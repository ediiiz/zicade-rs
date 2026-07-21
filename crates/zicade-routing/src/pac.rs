//! PAC result parsing.
//!
//! Parses a proxy string into a structured [`PacResult`]. Two closely-related
//! wire formats are accepted so the same parser serves both seams:
//!
//! * the classic PAC `FindProxyForURL` return value —
//!   `"DIRECT"`, `"PROXY host:port"`, `"PROXY a:1; DIRECT"`, `"SOCKS h:1080"`;
//! * the bare proxy list `WinHttpGetProxyForUrl` hands back in
//!   `WINHTTP_PROXY_INFO.lpszProxy` — `"host:port"`, optionally scheme-prefixed
//!   (`"https=host:port"`) and separated by `;` or whitespace.
//!
//! Rule: return the FIRST usable entry. `DIRECT` wins if seen first; the first
//! usable `PROXY`/bare `host:port` wins; `SOCKS` and malformed entries are
//! skipped. Empty/whitespace input, or nothing usable, yields [`PacResult::Direct`].

/// A parsed PAC/WinHTTP proxy decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacResult {
    /// Connect straight to the origin; no upstream proxy.
    Direct,
    /// Route through the given upstream proxy.
    Proxy {
        /// Proxy host (no scheme, no port).
        host: String,
        /// Proxy port.
        port: u16,
    },
}

impl PacResult {
    /// Parse a PAC/WinHTTP proxy string. See the module docs for the rules.
    pub fn parse(raw: &str) -> Self {
        for entry in raw.split([';', '\n']) {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let mut tokens = entry.split_whitespace();
            let keyword = tokens.next().unwrap_or_default();
            let upper = keyword.to_ascii_uppercase();

            if upper == "DIRECT" {
                return PacResult::Direct;
            }
            if upper == "PROXY" {
                // "PROXY host:port": the address is the next token.
                if let Some(result) = tokens.next().and_then(parse_addr) {
                    return result;
                }
                continue; // malformed PROXY entry -> skip to next
            }
            if upper.starts_with("SOCKS") {
                continue; // SOCKS is ignored for now
            }
            // Bare entry (WinHTTP proxy-list form), possibly "scheme=host:port".
            if let Some(result) = parse_addr(keyword) {
                return result;
            }
        }
        PacResult::Direct
    }
}

/// Parse a single `host:port` token, tolerating a leading `scheme=` prefix.
fn parse_addr(token: &str) -> Option<PacResult> {
    // "https=host:port" -> "host:port"; "host:port" is unchanged.
    let addr = token.rsplit('=').next().unwrap_or(token);
    let (host, port) = addr.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    let port: u16 = port.trim().parse().ok()?;
    Some(PacResult::Proxy {
        host: host.to_owned(),
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_proxy_without_port_is_direct() {
        assert_eq!(PacResult::parse("PROXY wp8080"), PacResult::Direct);
    }

    #[test]
    fn scheme_prefixed_bare_entry() {
        assert_eq!(
            PacResult::parse("https=wp8080:8080"),
            PacResult::Proxy {
                host: "wp8080".to_owned(),
                port: 8080
            }
        );
    }

    #[test]
    fn semicolon_separated_bare_list_takes_first() {
        assert_eq!(
            PacResult::parse("a:80;b:81"),
            PacResult::Proxy {
                host: "a".to_owned(),
                port: 80
            }
        );
    }
}
