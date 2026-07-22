//! The corporate-network gate supervisor.
//!
//! [`NetworkGate`] owns the proxy's live routing when the gate is enabled,
//! swapping between the configured routing (on-corp) and `Direct` (off-corp) as
//! the network changes. Detection and network-change waiting are injected so the
//! decision logic is unit-testable off-Windows.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::watch;
use zicade_config::Config;
use zicade_proxy::{Routing, RoutingHandle};

use super::on_corp;

/// A closure that rebuilds the on-corp routing from a config (`build_routing`).
/// Infallible from the gate's view: build errors are mapped to `Direct` by the
/// caller so a transient rebuild failure never wedges the gate.
type Rebuild = Box<dyn Fn(&Config) -> Routing + Send>;
/// A closure yielding the active adapter DNS suffixes (real one is
/// `zicade_win::active_dns_suffixes`; tests inject a fake).
type Detect = Box<dyn Fn() -> Vec<String> + Send>;
/// A closure invoked with the current on-corp state whenever it changes:
/// `Some(true)`/`Some(false)` on an on-/off-corp transition, and `None` when the
/// gate is disabled (so the web status stops reporting a stale `onCorp`).
type Report = Box<dyn Fn(Option<bool>) + Send>;

/// Whether the corporate-network gate is enabled in `config`.
pub(crate) fn gate_enabled(config: &Config) -> bool {
    matches!(&config.routing.corp_network, Some(c) if c.enabled)
}

/// Extract the effective gate parameters from a config: `(enabled, suffixes,
/// poll)`. A disabled/absent gate yields a harmless default poll.
fn gate_params(config: &Config) -> (bool, Vec<String>, Duration) {
    match &config.routing.corp_network {
        Some(c) if c.enabled => (
            true,
            c.dns_suffixes.clone(),
            Duration::from_secs(c.poll_seconds.max(1)),
        ),
        _ => (false, Vec::new(), Duration::from_secs(30)),
    }
}

/// The mutable state the gate applies decisions from. Guarded by a `Mutex` so
/// the monitor thread and the config-apply hook share one source of truth.
struct GateInner {
    routing: RoutingHandle,
    enabled: bool,
    suffixes: Vec<String>,
    poll: Duration,
    config: Config,
    rebuild: Rebuild,
    detect: Detect,
    report: Report,
    /// Last applied on-corp decision; `None` forces the next evaluation to apply
    /// (used at startup and after a reconfigure).
    last: Option<bool>,
}

impl GateInner {
    /// Detect the current network and, if the on-corp decision changed, swap the
    /// live routing (fresh on-corp routing, or `Direct` off-corp). No-op while
    /// the gate is disabled.
    fn evaluate(&mut self) {
        if !self.enabled {
            return;
        }
        let corp = on_corp(&(self.detect)(), &self.suffixes);
        if self.last == Some(corp) {
            return;
        }
        self.apply(corp);
        self.last = Some(corp);
    }

    /// Swap the live routing for the given on-corp state and notify `report`.
    fn apply(&mut self, corp: bool) {
        let routing = if corp {
            (self.rebuild)(&self.config)
        } else {
            Routing::Direct
        };
        tracing::info!(
            on_corp = corp,
            routing = ?routing,
            "corp-network gate: applying routing"
        );
        self.routing.set(routing);
        (self.report)(Some(corp));
    }

    /// Adopt a new config: refresh suffixes/poll and re-evaluate. If the gate was
    /// turned off, apply the plain configured routing once and go dormant.
    fn reconfigure(&mut self, config: Config) {
        let (enabled, suffixes, poll) = gate_params(&config);
        self.enabled = enabled;
        self.suffixes = suffixes;
        self.poll = poll;
        self.config = config;
        self.last = None;
        if self.enabled {
            self.evaluate();
        } else {
            let routing = (self.rebuild)(&self.config);
            tracing::info!("corp-network gate disabled; applying configured routing");
            self.routing.set(routing);
            // Clear any stale on-corp state so the web status stops reporting it.
            (self.report)(None);
        }
    }
}

/// Corporate-network gate: owns the live routing when enabled, swapping between
/// the configured routing (on-corp) and `Direct` (off-corp) as the network
/// changes. Cheaply cloneable (shared `Arc`) so the monitor thread and the
/// config-apply hook operate on the same state.
#[derive(Clone)]
pub(crate) struct NetworkGate {
    inner: Arc<Mutex<GateInner>>,
}

impl NetworkGate {
    /// Build a gate from the current `config`, injecting the routing `rebuild`,
    /// the suffix `detect`, and the transition `report`.
    pub(crate) fn new(
        routing: RoutingHandle,
        config: Config,
        rebuild: Rebuild,
        detect: Detect,
        report: Report,
    ) -> Self {
        let (enabled, suffixes, poll) = gate_params(&config);
        let inner = GateInner {
            routing,
            enabled,
            suffixes,
            poll,
            config,
            rebuild,
            detect,
            report,
            last: None,
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Evaluate once, synchronously — used at startup to set the correct initial
    /// routing before the proxy begins serving.
    pub(crate) fn evaluate_now(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.evaluate();
        }
    }

    /// Adopt a new config (from the web config-apply hook), re-evaluating now.
    pub(crate) fn reconfigure(&self, config: Config) {
        if let Ok(mut g) = self.inner.lock() {
            g.reconfigure(config);
        }
    }

    /// Spawn the monitor thread: evaluate, then block until a network change or
    /// the poll interval elapses, until `shutdown` flips to `true`. Detached —
    /// the process tears it down on exit, so it is never joined.
    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) {
        thread::Builder::new()
            .name("zicade-netmon".to_owned())
            .spawn(move || {
                while !*shutdown.borrow() {
                    let poll = self
                        .inner
                        .lock()
                        .map(|mut g| {
                            g.evaluate();
                            g.poll
                        })
                        .unwrap_or(Duration::from_secs(30));
                    zicade_win::wait_for_network_change(poll);
                }
                tracing::debug!("corp-network gate thread exiting on shutdown");
            })
            .expect("spawn zicade-netmon thread");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use zicade_config::CorpNetworkConfig;
    use zicade_proxy::{UpstreamAuth, UpstreamTarget};

    /// An enabled gate config for the given corporate suffixes.
    fn corp_config(suffixes: &[&str]) -> Config {
        let mut cfg = Config::default();
        cfg.routing.corp_network = Some(CorpNetworkConfig {
            enabled: true,
            dns_suffixes: suffixes.iter().map(|s| (*s).to_owned()).collect(),
            poll_seconds: 30,
        });
        cfg
    }

    /// The sentinel "on-corp" routing the fake `rebuild` returns, distinguishable
    /// from `Direct` by its Debug label ("Upstream").
    fn intended() -> Routing {
        Routing::Upstream(UpstreamTarget {
            addr: "wp8080:8080".to_owned(),
            auth: UpstreamAuth::None,
        })
    }

    fn label(handle: &RoutingHandle) -> String {
        format!("{:?}", handle.current())
    }

    /// Build a gate over a shared, mutable active-suffix list and a transition
    /// counter, so tests drive detection deterministically off-Windows.
    fn test_gate(
        config: Config,
        active: Arc<Mutex<Vec<String>>>,
        transitions: Arc<AtomicUsize>,
    ) -> (NetworkGate, RoutingHandle) {
        let handle = RoutingHandle::new(Routing::Direct);
        let gate = NetworkGate::new(
            handle.clone(),
            config,
            Box::new(|_cfg| intended()),
            Box::new(move || active.lock().unwrap().clone()),
            Box::new(move |_state: Option<bool>| {
                transitions.fetch_add(1, Ordering::SeqCst);
            }),
        );
        (gate, handle)
    }

    #[test]
    fn on_corp_applies_intended_routing() {
        let active = Arc::new(Mutex::new(vec!["dy.droot.org".to_owned()]));
        let (gate, handle) = test_gate(
            corp_config(&["droot.org"]),
            active,
            Arc::new(AtomicUsize::new(0)),
        );
        gate.evaluate_now();
        assert_eq!(
            label(&handle),
            "Upstream",
            "on-corp uses configured routing"
        );
    }

    #[test]
    fn off_corp_falls_back_to_direct() {
        let active = Arc::new(Mutex::new(vec!["home.lan".to_owned()]));
        let (gate, handle) = test_gate(
            corp_config(&["droot.org"]),
            active,
            Arc::new(AtomicUsize::new(0)),
        );
        gate.evaluate_now();
        assert_eq!(label(&handle), "Direct", "off-corp routes Direct");
    }

    #[test]
    fn only_applies_on_change() {
        let active = Arc::new(Mutex::new(vec!["home.lan".to_owned()]));
        let transitions = Arc::new(AtomicUsize::new(0));
        let (gate, handle) = test_gate(
            corp_config(&["droot.org"]),
            Arc::clone(&active),
            Arc::clone(&transitions),
        );

        gate.evaluate_now(); // off-corp -> Direct (transition 1)
        gate.evaluate_now(); // unchanged -> no transition
        assert_eq!(transitions.load(Ordering::SeqCst), 1);
        assert_eq!(label(&handle), "Direct");

        *active.lock().unwrap() = vec!["dy.droot.org".to_owned()];
        gate.evaluate_now(); // on-corp -> Upstream (transition 2)
        gate.evaluate_now(); // unchanged
        assert_eq!(transitions.load(Ordering::SeqCst), 2);
        assert_eq!(label(&handle), "Upstream");
    }

    #[test]
    fn reconfigure_disable_applies_configured_routing_and_stops_gating() {
        let active = Arc::new(Mutex::new(vec!["home.lan".to_owned()])); // off-corp
        let (gate, handle) = test_gate(
            corp_config(&["droot.org"]),
            Arc::clone(&active),
            Arc::new(AtomicUsize::new(0)),
        );
        gate.evaluate_now();
        assert_eq!(label(&handle), "Direct");

        // Disable the gate: the plain configured routing applies immediately, and
        // further evaluations do nothing even though we are still "off-corp".
        gate.reconfigure(Config::default());
        assert_eq!(
            label(&handle),
            "Upstream",
            "disabled gate uses configured routing"
        );
        gate.evaluate_now();
        assert_eq!(label(&handle), "Upstream", "disabled gate no longer gates");
    }
}
