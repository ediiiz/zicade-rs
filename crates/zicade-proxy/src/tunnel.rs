//! Raw bidirectional splice for `CONNECT` tunnels (direct mode).

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::metrics::ProxyMetrics;

/// Connect to `target` and splice bytes between the upgraded client connection
/// and the origin until either side closes, tallying the bytes into `metrics`.
pub(crate) async fn tunnel(
    upgraded: Upgraded,
    target: &str,
    metrics: &ProxyMetrics,
) -> std::io::Result<()> {
    let origin = TcpStream::connect(target).await?;
    splice(upgraded, origin, metrics).await
}

/// Splice bytes between the upgraded client connection and an already-connected
/// stream (e.g. a tunnel established through an upstream proxy).
///
/// `copy_bidirectional` returns `(client→peer, peer→client)`, i.e. `(out, in)`
/// from the proxy's point of view, which we fold into `metrics` on completion.
pub(crate) async fn splice(
    upgraded: Upgraded,
    mut peer: TcpStream,
    metrics: &ProxyMetrics,
) -> std::io::Result<()> {
    let mut client = TokioIo::new(upgraded);
    let (out, inbound) = tokio::io::copy_bidirectional(&mut client, &mut peer).await?;
    metrics.add_bytes(inbound, out);
    Ok(())
}
