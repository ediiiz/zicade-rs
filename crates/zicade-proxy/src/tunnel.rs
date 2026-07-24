//! Instrumented bidirectional splice for `CONNECT` tunnels.
//!
//! Unlike a bare `copy_bidirectional`, this keeps a live per-direction byte
//! count, propagates half-close, and emits rich lifecycle logs (`tunnel
//! established` / `tunnel closed[ with error]`) carrying the exact errno, the
//! failing direction, and how many bytes each direction had moved before the
//! close. That turns an opaque "connection reset" into an actionable record:
//! which side broke, and after how much data.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::metrics::ProxyMetrics;

/// Per-direction copy buffer. Larger than tokio's 8 KiB default so large
/// uploads/downloads move in fewer syscalls.
const BUFFER_SIZE: usize = 256 * 1024;

/// Once one direction *errors*, how long to let the other drain before giving
/// up. A clean EOF on one side never triggers this — the peer direction is
/// allowed to run to its own completion (a half-closed upload can still receive
/// a slow response).
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// If a single write makes no progress for this long, the sink has stopped
/// consuming (e.g. a gateway silently stalling an upload) — fail the pump
/// instead of parking the tunnel forever. The implied minimum drain rate is
/// BUFFER_SIZE / this ≈ 4 KiB/s, far below any live peer here (loopback
/// client, LAN gateway). Reads are deliberately unbounded: an idle-but-open
/// tunnel is legitimate.
const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Which half of the tunnel an event refers to.
#[derive(Clone, Copy)]
enum Direction {
    ClientToUpstream,
    UpstreamToClient,
}

/// Describes a tunnel for its lifecycle logs. Addresses are `Option` because the
/// peer/local address lookups can fail on a torn-down socket; they render as `-`.
pub(crate) struct TunnelInfo {
    pub client_remote: Option<SocketAddr>,
    pub client_local: Option<SocketAddr>,
    /// The CONNECT target authority (`host:port`).
    pub target: String,
    /// The upstream proxy `host:port` when routed through one; `None` = direct.
    pub upstream: Option<String>,
}

/// Direct mode: connect straight to `info.target`, then splice.
pub(crate) async fn tunnel(
    upgraded: Upgraded,
    metrics: &ProxyMetrics,
    info: TunnelInfo,
) -> io::Result<()> {
    let peer = match TcpStream::connect(&info.target).await {
        Ok(peer) => {
            if let Err(err) = peer.set_nodelay(true) {
                tracing::debug!(error = %err, "failed to set TCP_NODELAY on origin socket");
            }
            peer
        }
        Err(err) => {
            tracing::warn!(
                target = %info.target,
                client_remote = %addr(info.client_remote),
                error = %err,
                "tunnel origin connect failed"
            );
            return Err(err);
        }
    };
    splice(upgraded, peer, metrics, info).await
}

/// Splice bytes between the upgraded client connection and an already-connected
/// `peer` (a direct origin, or an authenticated upstream tunnel), tallying the
/// bytes into `metrics` and logging the tunnel's establishment and close.
// The complexity is the two structured lifecycle-log arms plus the select/drain
// close policy; splitting it would scatter the tunnel's teardown logic.
#[allow(clippy::cognitive_complexity)]
pub(crate) async fn splice(
    upgraded: Upgraded,
    peer: TcpStream,
    metrics: &ProxyMetrics,
    info: TunnelInfo,
) -> io::Result<()> {
    let upstream_remote = addr(peer.peer_addr().ok());
    let upstream_local = addr(peer.local_addr().ok());
    let route = info.upstream.as_deref().unwrap_or("direct");

    tracing::info!(
        client_remote = %addr(info.client_remote),
        client_local = %addr(info.client_local),
        upstream_remote = %upstream_remote,
        upstream_local = %upstream_local,
        target = %info.target,
        upstream = %route,
        buffer_size = BUFFER_SIZE,
        close_grace_secs = CLOSE_GRACE.as_secs(),
        "tunnel established"
    );

    let client = TokioIo::new(upgraded);
    let (client_r, client_w) = tokio::io::split(client);
    let (peer_r, peer_w) = tokio::io::split(peer);

    // Live counters: readable even if a pump is aborted mid-flight by the grace.
    let c2u = AtomicU64::new(0); // client -> upstream (upload)
    let u2c = AtomicU64::new(0); // upstream -> client (download)
    let start = Instant::now();

    let up = pump(client_r, peer_w, &c2u, Direction::ClientToUpstream);
    let down = pump(peer_r, client_w, &u2c, Direction::UpstreamToClient);
    tokio::pin!(up);
    tokio::pin!(down);

    // Run both directions. Whichever settles first decides how long to wait on
    // the other (see [`wait_for`]): a clean EOF lets the peer run to completion
    // (a half-closed upload can still receive a slow response); a client-side
    // reset abandons the peer immediately; only an upstream-side reset grants a
    // brief grace to flush any remaining bytes to the still-live client.
    let (c2u_out, u2c_out) = tokio::select! {
        r = &mut up => {
            let wait = wait_for(&r, Direction::ClientToUpstream);
            let other = drain_other(wait, down).await;
            (Outcome::from_res(r), other)
        }
        r = &mut down => {
            let wait = wait_for(&r, Direction::UpstreamToClient);
            let other = drain_other(wait, up).await;
            (other, Outcome::from_res(r))
        }
    };

    let bytes_c2u = c2u.load(Ordering::Relaxed);
    let bytes_u2c = u2c.load(Ordering::Relaxed);
    metrics.add_bytes(bytes_u2c, bytes_c2u);

    let duration_ms = start.elapsed().as_millis() as u64;
    let bytes_total = bytes_c2u + bytes_u2c;

    // Classify the close from BOTH directions' terminal state. The decisive
    // question is whether the download (upstream -> client) reached a clean EOF:
    // if it did, the response was fully delivered and any client-side reset that
    // follows is just abortive teardown (benign, and common on Windows), NOT a
    // failed transfer. A real failure is an unfinished transfer — the upstream
    // resetting the download, or the client resetting before the download ended.
    let real_failure = match (&c2u_out, &u2c_out) {
        // A *write* failure on the upload path means the upstream closed/refused
        // the body mid-relay — a real failure even if the download side then
        // EOF'd, because the client's request body never fully reached the server.
        // (`who_reset` attributes a c2u write failure to the upstream.)
        (Outcome::Failed(e), _) if e.op == "write" => Some((e, Direction::ClientToUpstream)),
        // Download reached a clean EOF: the response was fully delivered, so a
        // client-side (read) reset afterwards is just abortive teardown — benign.
        (_, Outcome::Eof) => None,
        // Upstream reset the still-open download.
        (_, Outcome::Failed(e)) => Some((e, Direction::UpstreamToClient)),
        // Client reset before the download finished.
        (Outcome::Failed(e), _) => Some((e, Direction::ClientToUpstream)),
        _ => None,
    };

    match real_failure {
        Some((pe, dir)) => {
            tracing::info!(
                client_remote = %addr(info.client_remote),
                client_local = %addr(info.client_local),
                upstream_remote = %upstream_remote,
                upstream_local = %upstream_local,
                target = %info.target,
                upstream = %route,
                bytes_total,
                bytes_client_to_upstream = bytes_c2u,
                bytes_upstream_to_client = bytes_u2c,
                duration_ms,
                close_reason = "error",
                failed_direction = dir.as_str(),
                failed_op = pe.op,
                reset_by = who_reset(dir, pe.op),
                c2u_outcome = c2u_out.as_str(),
                u2c_outcome = u2c_out.as_str(),
                error = %pe.err,
                "tunnel closed with error"
            );
            Err(io::Error::new(pe.err.kind(), pe.err.to_string()))
        }
        None => {
            // A completed transfer whose peer reset only during teardown is
            // still a success; report the teardown as a flag, not an error.
            let teardown_reset =
                matches!(c2u_out, Outcome::Failed(_)) || matches!(u2c_out, Outcome::Failed(_));
            tracing::info!(
                client_remote = %addr(info.client_remote),
                client_local = %addr(info.client_local),
                upstream_remote = %upstream_remote,
                upstream_local = %upstream_local,
                target = %info.target,
                upstream = %route,
                bytes_total,
                bytes_client_to_upstream = bytes_c2u,
                bytes_upstream_to_client = bytes_u2c,
                duration_ms,
                close_reason = "completed",
                c2u_outcome = c2u_out.as_str(),
                u2c_outcome = u2c_out.as_str(),
                teardown_reset,
                "tunnel closed"
            );
            Ok(())
        }
    }
}

/// How long to keep the peer direction alive once the first direction settled.
enum Wait {
    /// Clean EOF: let the peer finish on its own (supports legit half-close).
    Full,
    /// Upstream reset: brief window to flush buffered bytes to the live client.
    Grace,
    /// Client reset/gone: nothing left to deliver — drop the peer now.
    None,
}

/// Decide the peer's wait policy from the first-settled pump's outcome.
fn wait_for(res: &Result<(), PumpError>, dir: Direction) -> Wait {
    match res {
        Ok(()) => Wait::Full,
        Err(PumpError { op, .. }) => match who_reset(dir, op) {
            "client" => Wait::None,
            _ => Wait::Grace,
        },
    }
}

/// The terminal state of one direction's pump — richer than `Result` so the
/// close classifier can tell a peer that finished cleanly from one that was
/// abandoned by the [`Wait`] policy (status unknown) rather than confirmed done.
enum Outcome {
    /// Clean EOF — the source half-closed and we relayed everything.
    Eof,
    /// The pump failed (a read or write errored); carries the tagged error.
    Failed(PumpError),
    /// Abandoned by the close policy (the peer already settled); never observed
    /// reaching EOF, so it must not be counted as a completed transfer.
    Dropped,
}

impl Outcome {
    fn from_res(res: Result<(), PumpError>) -> Self {
        match res {
            Ok(()) => Outcome::Eof,
            Err(e) => Outcome::Failed(e),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Outcome::Eof => "eof",
            Outcome::Failed(_) => "failed",
            Outcome::Dropped => "dropped",
        }
    }
}

/// Resolve the peer direction under the chosen [`Wait`] policy into an [`Outcome`].
async fn drain_other<F>(wait: Wait, other: F) -> Outcome
where
    F: Future<Output = Result<(), PumpError>>,
{
    match wait {
        Wait::Full => Outcome::from_res(other.await),
        Wait::Grace => match tokio::time::timeout(CLOSE_GRACE, other).await {
            Ok(res) => Outcome::from_res(res),
            Err(_) => Outcome::Dropped, // grace expired; stop waiting on the dead peer
        },
        Wait::None => Outcome::Dropped, // drop the peer future without awaiting it
    }
}

/// A pump failure, tagged with which socket operation failed. `Read` means the
/// *source* endpoint died; `Write` means the *sink* endpoint died. Combined with
/// the pump's [`Direction`] this pins the reset to a concrete endpoint (see
/// [`who_reset`]).
struct PumpError {
    op: &'static str,
    err: io::Error,
}

/// Copy one direction until EOF or error, keeping `counter` current as bytes are
/// forwarded, and half-closing the write side on EOF so the peer sees the FIN.
async fn pump<R, W>(
    mut r: R,
    mut w: W,
    counter: &AtomicU64,
    dir: Direction,
) -> Result<(), PumpError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; BUFFER_SIZE];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(n) => n,
            Err(err) => return Err(PumpError { op: "read", err }),
        };
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(());
        }
        match tokio::time::timeout(WRITE_STALL_TIMEOUT, w.write_all(&buf[..n])).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(PumpError { op: "write", err }),
            Err(_) => {
                return Err(PumpError {
                    op: "write",
                    err: io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "write stalled for {}s (peer stopped consuming)",
                            WRITE_STALL_TIMEOUT.as_secs()
                        ),
                    ),
                });
            }
        }
        let total = counter.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        tracing::trace!(
            direction = dir.as_str(),
            chunk = n,
            total,
            "tunnel chunk forwarded"
        );
    }
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::ClientToUpstream => "client_to_upstream",
            Direction::UpstreamToClient => "upstream_to_client",
        }
    }
}

/// Resolve which endpoint actually reset, from the failing pump's direction and
/// the failed op. A read failure blames the source; a write failure blames the
/// sink. This is the field that separates a benign client teardown from a real
/// upstream/gateway reset.
fn who_reset(dir: Direction, op: &str) -> &'static str {
    match (dir, op) {
        (Direction::ClientToUpstream, "read") => "client",
        (Direction::ClientToUpstream, _) => "upstream",
        (Direction::UpstreamToClient, "read") => "upstream",
        (Direction::UpstreamToClient, _) => "client",
    }
}

/// Render an optional socket address for logging (`-` when unknown).
fn addr(a: Option<SocketAddr>) -> String {
    a.map(|a| a.to_string()).unwrap_or_else(|| "-".to_owned())
}
