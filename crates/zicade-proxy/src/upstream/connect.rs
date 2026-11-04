//! The authenticated `CONNECT` handshake against the upstream proxy, run on a
//! prewarmed socket when the target has a pool with one ready.

use std::io;

use tokio::net::TcpStream;

use super::auth::AuthState;
use super::wire::{self, Head};
use super::{HANDSHAKE_TIMEOUT, MAX_LEGS, UpstreamTarget};

/// Connect to the upstream proxy and establish an authenticated `CONNECT`
/// tunnel to `dst`, returning the raw stream ready to splice. Taking or dialing
/// the socket and the whole handshake, retry included, are bounded by
/// [`HANDSHAKE_TIMEOUT`].
pub(crate) async fn connect_via_upstream(
    target: &UpstreamTarget,
    dst: &str,
) -> io::Result<TcpStream> {
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, connect(target, dst)).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "upstream CONNECT handshake with {} timed out after {}s",
                target.addr,
                HANDSHAKE_TIMEOUT.as_secs()
            ),
        )),
    }
}

/// Try a pooled socket first; fall back to one fresh dial only if that socket
/// turned out to be dead before the upstream answered anything.
async fn connect(target: &UpstreamTarget, dst: &str) -> io::Result<TcpStream> {
    if let Some(stream) = target.pool.as_ref().and_then(|pool| pool.take()) {
        match handshake(stream, true, target, dst).await {
            Err(Failure::Stale(err)) => tracing::debug!(
                upstream = %target.addr,
                error = %err,
                "pooled upstream socket closed before answering; dialing fresh"
            ),
            result => return result.map_err(Failure::into_io),
        }
    }
    let stream = TcpStream::connect(&target.addr).await?;
    handshake(stream, false, target, dst)
        .await
        .map_err(Failure::into_io)
}

/// Why a handshake attempt failed.
enum Failure {
    /// A reused socket failed at the transport level before any response byte
    /// arrived: the upstream refused nothing, so one fresh dial is safe.
    /// Anything later, any HTTP status, and any auth error is [`Self::Failed`]:
    /// retrying those would resend credentials the gateway already rejected.
    Stale(io::Error),
    Failed(io::Error),
}

impl Failure {
    fn into_io(self) -> io::Error {
        match self {
            Self::Stale(err) | Self::Failed(err) => err,
        }
    }
}

/// Run the 407 handshake on `stream` until the upstream grants the tunnel.
async fn handshake(
    mut stream: TcpStream,
    reused: bool,
    target: &UpstreamTarget,
    dst: &str,
) -> Result<TcpStream, Failure> {
    let upstream = target.addr.as_str();
    if let Err(err) = stream.set_nodelay(true) {
        tracing::debug!(error = %err, "failed to set TCP_NODELAY on upstream socket");
    }
    tracing::debug!(
        upstream,
        target = dst,
        reused,
        "upstream CONNECT handshake begin"
    );
    let mut auth_state = AuthState::new(&target.auth);
    let mut header = auth_state.initial_header();

    for leg in 0..MAX_LEGS {
        let first_reused = reused && leg == 0;
        let (status, headers) = send_leg(&mut stream, dst, header.as_deref(), first_reused).await?;
        tracing::trace!(
            leg,
            status,
            target = dst,
            authed = header.is_some(),
            "upstream CONNECT leg response"
        );
        if status == 200 {
            tracing::debug!(
                upstream,
                target = dst,
                legs = leg + 1,
                "upstream CONNECT tunnel authorized"
            );
            return Ok(stream);
        }
        // Drain any body so the connection is clean for the next leg.
        wire::drain_body(&mut stream, &headers)
            .await
            .map_err(Failure::Failed)?;
        if status != 407 {
            return Err(Failure::Failed(io::Error::other(format!(
                "upstream refused CONNECT with status {status}"
            ))));
        }
        header = Some(auth_state.on_challenge(&headers).map_err(Failure::Failed)?);
    }
    Err(Failure::Failed(io::Error::other(
        "upstream handshake exhausted legs",
    )))
}

/// Send one `CONNECT` leg and read the response head. On the first leg of a
/// reused socket, a write failure or a close before any response byte is
/// [`Failure::Stale`].
async fn send_leg(
    stream: &mut TcpStream,
    dst: &str,
    auth_header: Option<&str>,
    first_reused: bool,
) -> Result<Head, Failure> {
    let transport = |err| {
        if first_reused {
            Failure::Stale(err)
        } else {
            Failure::Failed(err)
        }
    };
    let head = connect_request(dst, auth_header);
    wire::write_all(stream, head.as_bytes())
        .await
        .map_err(transport)?;
    if first_reused {
        wire::await_response(stream).await.map_err(transport)?;
    }
    wire::read_head(stream).await.map_err(Failure::Failed)
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
