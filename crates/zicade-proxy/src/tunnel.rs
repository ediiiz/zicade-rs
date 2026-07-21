//! Raw bidirectional splice for `CONNECT` tunnels (direct mode).

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

/// Connect to `target` and splice bytes between the upgraded client connection
/// and the origin until either side closes.
pub(crate) async fn tunnel(upgraded: Upgraded, target: &str) -> std::io::Result<()> {
    let origin = TcpStream::connect(target).await?;
    splice(upgraded, origin).await
}

/// Splice bytes between the upgraded client connection and an already-connected
/// stream (e.g. a tunnel established through an upstream proxy).
pub(crate) async fn splice(upgraded: Upgraded, mut peer: TcpStream) -> std::io::Result<()> {
    let mut client = TokioIo::new(upgraded);
    tokio::io::copy_bidirectional(&mut client, &mut peer).await?;
    Ok(())
}
