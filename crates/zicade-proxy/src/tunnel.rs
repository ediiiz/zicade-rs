//! Raw bidirectional splice for `CONNECT` tunnels (direct mode).

use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

/// Connect to `target` and splice bytes between the upgraded client connection
/// and the origin until either side closes.
pub(crate) async fn tunnel(upgraded: Upgraded, target: &str) -> std::io::Result<()> {
    let mut client = TokioIo::new(upgraded);
    let mut origin = TcpStream::connect(target).await?;
    tokio::io::copy_bidirectional(&mut client, &mut origin).await?;
    Ok(())
}
