//! In-process fake origin servers.

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::HeaderValue;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Spawn an HTTP/1.1 origin that echoes the request body back in the response
/// (with `Content-Length`) and sets `x-origin-echo: 1`. Returns its address.
pub async fn spawn_http_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service_fn(echo))
                    .await;
            });
        }
    });
    addr
}

async fn echo(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let (_parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    let mut resp = Response::new(Full::new(bytes));
    resp.headers_mut()
        .insert("x-origin-echo", HeaderValue::from_static("1"));
    Ok(resp)
}

/// Spawn a raw TCP server that echoes every byte it receives. Used as the
/// target of a `CONNECT` tunnel. Returns its address.
pub async fn spawn_tcp_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}
