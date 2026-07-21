//! A fake [`PacBackend`] for tests: classifies "internal" hosts as DIRECT and
//! everything else as a fixed upstream proxy, plus a variant that always errors
//! (to exercise `failPolicy`). Kept in the public API so integration tests and
//! other crates can drive `RouteResolver` without WinHTTP.

use crate::error::RoutingError;
use crate::pac::PacResult;
use crate::resolver::PacBackend;

/// A deterministic PAC backend for tests.
#[derive(Debug, Clone)]
pub enum FakeBackend {
    /// Classify by URL: any URL containing `internal` resolves to `DIRECT`,
    /// everything else resolves to the configured proxy.
    Classify {
        /// Proxy host returned for external URLs.
        proxy_host: String,
        /// Proxy port returned for external URLs.
        proxy_port: u16,
    },
    /// Always fails, to exercise `failPolicy`.
    Failing,
}

impl FakeBackend {
    /// Build a classifying backend that sends external hosts to `host:port`.
    pub fn classify(proxy_host: impl Into<String>, proxy_port: u16) -> Self {
        FakeBackend::Classify {
            proxy_host: proxy_host.into(),
            proxy_port,
        }
    }

    /// Build a backend whose `resolve` always errors.
    pub fn failing() -> Self {
        FakeBackend::Failing
    }
}

impl PacBackend for FakeBackend {
    fn resolve(&self, url: &str) -> Result<PacResult, RoutingError> {
        match self {
            FakeBackend::Classify {
                proxy_host,
                proxy_port,
            } => {
                if url.contains("internal") {
                    Ok(PacResult::Direct)
                } else {
                    Ok(PacResult::Proxy {
                        host: proxy_host.clone(),
                        port: *proxy_port,
                    })
                }
            }
            FakeBackend::Failing => Err(RoutingError::Backend("fake backend failure".to_owned())),
        }
    }
}
