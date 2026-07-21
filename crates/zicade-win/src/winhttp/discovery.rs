//! Pure (FFI-free) proxy-discovery decision logic for `source = "auto"`.
//!
//! Windows / browsers resolve the effective proxy for the current user with a
//! fixed precedence over the per-user WinINET/IE settings:
//!
//! 1. an explicit "Use setup script" address (`AutoConfigUrl`) — a PAC URL;
//! 2. WPAD network auto-detect (`fAutoDetect`);
//! 3. a static manual proxy (`Proxy`), honouring its bypass list;
//! 4. otherwise a direct connection.
//!
//! [`select_strategy`] encodes that precedence as a pure function of an
//! [`IeProxyConfig`], so it is unit-testable without touching WinHTTP. The
//! actual IE-config read (FFI) lives in [`super::win`]; it feeds the values
//! parsed here. [`StaticResolver`] implements the static-proxy leg, including a
//! minimal bypass match.

use zicade_routing::PacResult;

/// The per-user WinINET/IE proxy configuration, copied out of
/// `WINHTTP_CURRENT_USER_IE_PROXY_CONFIG` into owned Rust values.
#[derive(Debug, Default, Clone)]
pub(crate) struct IeProxyConfig {
    /// "Use setup script" address (a PAC URL) if configured.
    pub auto_config_url: Option<String>,
    /// Whether WPAD network auto-detect is enabled.
    pub auto_detect: bool,
    /// Static manual proxy string (e.g. `host:port` or `http=h:80;https=h:443`).
    pub proxy: Option<String>,
    /// Static proxy bypass list (`;`/whitespace separated, may contain `<local>`).
    pub bypass: Option<String>,
}

impl IeProxyConfig {
    /// An empty configuration (nothing set) — resolves to [`Strategy::Direct`].
    pub(crate) fn empty() -> Self {
        Self::default()
    }
}

/// The discovery outcome: how `source = "auto"` should resolve proxies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Strategy {
    /// Resolve via an explicit PAC config URL (the AutoConfigUrl).
    ConfigUrl(String),
    /// Resolve via WPAD network auto-detect.
    Wpad,
    /// Honour a static manual proxy with an optional bypass list.
    StaticProxy {
        /// The manual proxy string.
        proxy: String,
        /// The bypass list, if any.
        bypass: Option<String>,
    },
    /// Nothing configured — connect directly.
    Direct,
}

/// Choose the discovery [`Strategy`] from an [`IeProxyConfig`], applying the
/// MSDN precedence: AutoConfigUrl > WPAD auto-detect > static proxy > direct.
pub(crate) fn select_strategy(cfg: &IeProxyConfig) -> Strategy {
    if let Some(url) = cfg.auto_config_url.as_deref() {
        if !url.trim().is_empty() {
            return Strategy::ConfigUrl(url.to_owned());
        }
    }
    if cfg.auto_detect {
        return Strategy::Wpad;
    }
    if let Some(proxy) = cfg.proxy.as_deref() {
        if !proxy.trim().is_empty() {
            return Strategy::StaticProxy {
                proxy: proxy.to_owned(),
                bypass: cfg.bypass.clone(),
            };
        }
    }
    Strategy::Direct
}

/// A static-proxy resolver: returns the configured proxy for every host except
/// those matching the bypass list, which resolve DIRECT.
#[derive(Debug, Clone)]
pub(crate) struct StaticResolver {
    proxy: PacResult,
    bypass: Vec<String>,
}

impl StaticResolver {
    /// Build a resolver from a manual proxy string and optional bypass list.
    ///
    /// The proxy string is parsed with the shared [`PacResult`] parser (it
    /// accepts `host:port`, scheme-prefixed `http=host:port`, and `;`/space
    /// separated lists, taking the first usable entry). The bypass list is split
    /// on `;`/whitespace and lower-cased.
    pub(crate) fn new(proxy: &str, bypass: Option<&str>) -> Self {
        let bypass = bypass
            .map(|list| {
                list.split([';', ' ', '\t', '\n', '\r'])
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            proxy: PacResult::parse(proxy),
            bypass,
        }
    }

    /// Resolve `url`: the static proxy, or DIRECT if the host is bypassed.
    pub(crate) fn resolve(&self, url: &str) -> PacResult {
        let host = host_of(url);
        if host_bypassed(&host, &self.bypass) {
            return PacResult::Direct;
        }
        self.proxy.clone()
    }
}

/// Extract the lower-cased host from a URL (scheme/userinfo/port/path stripped).
fn host_of(url: &str) -> String {
    // Drop the scheme, if present.
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    // Authority ends at the first path/query/fragment delimiter.
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Strip userinfo (everything up to and including the last '@').
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    // Strip the port. Bracketed IPv6 literals keep their contents.
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split_once(']').map_or(host_port, |(h, _)| h)
    } else {
        host_port.split_once(':').map_or(host_port, |(h, _)| h)
    };
    host.to_ascii_lowercase()
}

/// True if `host` matches any entry in the parsed (lower-cased) bypass list.
///
/// Supported forms: `<local>` (any dotless host, i.e. no `.`), an exact
/// (case-insensitive) match, and `*` wildcards matching any run of characters
/// (e.g. `*.corp.local`, `10.*`). No CIDR/IP-range support.
fn host_bypassed(host: &str, entries: &[String]) -> bool {
    entries.iter().any(|entry| {
        if entry == "<local>" {
            !host.contains('.')
        } else {
            wildcard_match(entry, host)
        }
    })
}

/// Glob match where `*` in `pattern` matches any (possibly empty) run of chars.
/// Inputs are expected already lower-cased.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let last = parts.len() - 1;
    // The segment before the first '*' must be a literal prefix.
    let Some(mut rest) = text.strip_prefix(parts[0]) else {
        return false;
    };
    for (i, part) in parts.iter().enumerate().skip(1) {
        if i == last {
            // The trailing segment (possibly empty) must be a suffix of what
            // remains, so no earlier match is consumed past it.
            return rest.ends_with(part);
        }
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(off) => rest = &rest[off + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        auto_config_url: Option<&str>,
        auto_detect: bool,
        proxy: Option<&str>,
        bypass: Option<&str>,
    ) -> IeProxyConfig {
        IeProxyConfig {
            auto_config_url: auto_config_url.map(str::to_owned),
            auto_detect,
            proxy: proxy.map(str::to_owned),
            bypass: bypass.map(str::to_owned),
        }
    }

    #[test]
    fn autoconfigurl_wins_over_everything() {
        let s = select_strategy(&cfg(
            Some("http://wp/proxy.pac"),
            true,
            Some("p:8080"),
            None,
        ));
        assert_eq!(s, Strategy::ConfigUrl("http://wp/proxy.pac".to_owned()));
    }

    #[test]
    fn empty_autoconfigurl_is_ignored() {
        // Whitespace-only AutoConfigUrl must not win; fall through to WPAD.
        let s = select_strategy(&cfg(Some("   "), true, None, None));
        assert_eq!(s, Strategy::Wpad);
    }

    #[test]
    fn autodetect_selects_wpad_when_no_url() {
        let s = select_strategy(&cfg(None, true, Some("p:8080"), None));
        assert_eq!(s, Strategy::Wpad);
    }

    #[test]
    fn only_static_proxy_selects_static() {
        let s = select_strategy(&cfg(None, false, Some("p:8080"), Some("<local>")));
        assert_eq!(
            s,
            Strategy::StaticProxy {
                proxy: "p:8080".to_owned(),
                bypass: Some("<local>".to_owned()),
            }
        );
    }

    #[test]
    fn nothing_configured_selects_direct() {
        assert_eq!(
            select_strategy(&cfg(None, false, None, None)),
            Strategy::Direct
        );
        assert_eq!(select_strategy(&IeProxyConfig::empty()), Strategy::Direct);
    }

    #[test]
    fn static_resolver_returns_proxy_for_external_host() {
        let r = StaticResolver::new("wp8080:8080", Some("<local>;*.corp.local"));
        assert_eq!(
            r.resolve("https://example.com/"),
            PacResult::Proxy {
                host: "wp8080".to_owned(),
                port: 8080,
            }
        );
    }

    #[test]
    fn static_resolver_bypasses_local_and_wildcard() {
        let r = StaticResolver::new("wp8080:8080", Some("<local>;*.corp.local"));
        // <local>: a dotless host bypasses.
        assert_eq!(r.resolve("http://intranet/"), PacResult::Direct);
        // *.corp.local: a suffix wildcard bypasses.
        assert_eq!(r.resolve("http://build.corp.local/x"), PacResult::Direct);
    }

    #[test]
    fn host_of_strips_scheme_userinfo_port_and_path() {
        assert_eq!(
            host_of("https://user:pw@Example.COM:8443/a?b"),
            "example.com"
        );
        assert_eq!(host_of("http://intranet/"), "intranet");
        assert_eq!(host_of("no-scheme.example/x"), "no-scheme.example");
    }

    #[test]
    fn wildcard_match_common_cases() {
        assert!(wildcard_match("example.com", "example.com"));
        assert!(!wildcard_match("example.com", "example.org"));
        assert!(wildcard_match("*.corp.local", "build.corp.local"));
        assert!(!wildcard_match("*.corp.local", "corp.local"));
        assert!(wildcard_match("10.*", "10.0.0.1"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*c", "abc"));
        assert!(!wildcard_match("a*c", "abd"));
    }

    #[test]
    fn host_bypassed_matches_local_for_dotless_only() {
        let entries = vec!["<local>".to_owned()];
        assert!(host_bypassed("intranet", &entries));
        assert!(!host_bypassed("example.com", &entries));
    }
}
