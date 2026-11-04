//! The authenticated `CONNECT` handshake against the upstream proxy.
//!
//! Split out of [`super`] to keep each file inside the repo's file-size guard.
//! The conversation is raw HTTP/1.1 over one [`TcpStream`] because the
//! handshake must span several request/response legs on the *same* connection.

use std::io;

use tokio::net::TcpStream;

use super::auth::AuthState;
use super::{HANDSHAKE_TIMEOUT, MAX_LEGS, UpstreamTarget, wire};

/// Connect to the upstream proxy and establish an authenticated `CONNECT`
/// tunnel to `target`, returning the raw stream ready to splice. The dial and
/// the full handshake are bounded by [`HANDSHAKE_TIMEOUT`].
///
/// When `target` carries a pool, the dial is served from an idle socket where
/// possible, which removes one RTT per request. A reused socket the gateway has
/// already closed fails its first write; that is retried once with a fresh
/// dial so pooling never turns a stale entry into a user-visible error.
pub(crate) async fn connect_via_upstream(
    target: &UpstreamTarget,
    dst: &str,
) -> io::Result<TcpStream> {
    match connect_bounded(target, dst, true).await {
        Ok(stream) => Ok(stream),
        // Retry only when this attempt actually consumed a pooled socket: a
        // freshly dialled socket that failed would fail the same way a second
        // time, so retrying it would just double the cost of every real error.
        Err(err) if err.reused => connect_bounded(target, dst, false)
            .await
            .map_err(|e| e.error),
        Err(err) => Err(err.error),
    }
}

/// Outcome of one failed attempt: the error, plus whether it used a pooled
/// socket (and so deserves a single fresh-dial retry).
struct AttemptError {
    error: io::Error,
    reused: bool,
}

/// One dial + handshake attempt.
///
/// `allow_pool` is false on the retry, so a retry can never bite the same stale
/// socket twice — the second attempt always dials fresh.
async fn connect_bounded(
    target: &UpstreamTarget,
    dst: &str,
    allow_pool: bool,
) -> Result<TcpStream, AttemptError> {
    let upstream_addr = target.addr.as_str();
    let result = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        connect_handshake(target, dst, allow_pool),
    )
    .await;
    match result {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) => Err(AttemptError {
            reused: err.reused,
            error: err.error,
        }),
        Err(_) => Err(AttemptError {
            reused: false,
            error: io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "upstream CONNECT handshake with {upstream_addr} timed out after {}s",
                    HANDSHAKE_TIMEOUT.as_secs()
                ),
            ),
        }),
    }
}

#[allow(
    clippy::cognitive_complexity,
    reason = "the multi-leg 407 loop plus per-leg tracing reads as one handshake"
)]
async fn connect_handshake(
    target: &UpstreamTarget,
    dst: &str,
    allow_pool: bool,
) -> Result<TcpStream, AttemptError> {
    let upstream_addr = target.addr.as_str();
    let (mut stream, reused) = if allow_pool {
        target.dial().await
    } else {
        target.dial_fresh().await
    }
    .map_err(|err| AttemptError {
        error: err,
        reused: false,
    })?;
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on upstream socket");
    }
    tracing::debug!(
        upstream = upstream_addr,
        target = dst,
        reused,
        "upstream CONNECT handshake begin"
    );
    let mut auth_state = AuthState::new(&target.auth);
    let mut header = auth_state.initial_header();

    let fail = |error: io::Error| AttemptError { error, reused };

    for leg in 0..MAX_LEGS {
        let head = connect_request(dst, header.as_deref());
        if let Err(err) = wire::write_all(&mut stream, head.as_bytes()).await {
            // A pooled socket can be closed by the gateway between take and
            // write; recycle() will drop it (shrink sees EOF), so just report.
            return Err(fail(err));
        }
        let (status, headers) = match wire::read_head(&mut stream).await {
            Ok(pair) => pair,
            Err(err) => return Err(fail(err)),
        };
        tracing::trace!(
            leg,
            status,
            target = dst,
            authed = header.is_some(),
            "upstream CONNECT leg response"
        );

        match status {
            200 => {
                tracing::debug!(
                    upstream = upstream_addr,
                    target = dst,
                    legs = leg + 1,
                    "upstream CONNECT tunnel authorized"
                );
                return Ok(stream);
            }
            407 => {
                // Drain any body so the connection is clean for the next leg.
                if let Err(err) = wire::drain_body(&mut stream, &headers).await {
                    return Err(fail(err));
                }
                match auth_state.on_challenge(&headers) {
                    Ok(next) => header = Some(next),
                    Err(err) => return Err(fail(err)),
                }
            }
            other => {
                if let Err(err) = wire::drain_body(&mut stream, &headers).await {
                    return Err(fail(err));
                }
                // The socket itself is fine; the gateway refused this target.
                // Recycle so the next CONNECT skips the TCP dial.
                target.recycle(stream);
                return Err(fail(io::Error::other(format!(
                    "upstream refused CONNECT with status {other}"
                ))));
            }
        }
    }
    Err(fail(io::Error::other("upstream handshake exhausted legs")))
}

fn connect_request(target: &str, auth_header: Option<&str>) -> String {
    // Match what browsers and Winfoom send to the upstream proxy: an explicit
    // `Proxy-Connection: keep-alive` on the CONNECT. Some corporate gateways
    // otherwise apply a shorter/closing tunnel policy, which can drop long
    // uploads mid-stream.
    let mut head =
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n");
    if let Some(value) = auth_header {
        head.push_str(&format!("Proxy-Authorization: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}
