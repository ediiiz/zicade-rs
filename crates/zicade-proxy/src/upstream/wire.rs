//! Raw HTTP/1.1 wire helpers for the upstream-proxy conversation.
//!
//! The upstream handshake is multi-leg on a single TCP connection, so we cannot
//! use a pooled hyper client here. These helpers read a response head strictly
//! up to the terminator (never over-reading into a following tunnel), then read
//! or drain the body by framing.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Guard against a hostile/unbounded response head.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// A parsed response head: status code plus lowercased header name/value pairs.
pub(crate) type Head = (u16, Vec<(String, String)>);

/// Read a response head up to and including `\r\n\r\n`, one byte at a time so no
/// body/tunnel bytes are consumed. Returns the status and parsed headers.
pub(crate) async fn read_head(stream: &mut TcpStream) -> io::Result<Head> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed before response head completed",
            ));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(io::Error::other("upstream response head too large"));
        }
    }
    Ok(parse_head(&buf))
}

fn parse_head(buf: &[u8]) -> Head {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    (status, headers)
}

/// The `Content-Length` value, if present and parseable.
pub(crate) fn content_length(headers: &[(String, String)]) -> Option<usize> {
    header(headers, "content-length").and_then(|v| v.parse().ok())
}

fn is_chunked(headers: &[(String, String)]) -> bool {
    header(headers, "transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Read the response body according to its framing (`Content-Length`, chunked,
/// or empty). Used to consume a non-final response so the connection stays clean
/// for the next handshake leg, and to buffer the final response.
pub(crate) async fn read_body(
    stream: &mut TcpStream,
    headers: &[(String, String)],
) -> io::Result<Vec<u8>> {
    if let Some(len) = content_length(headers) {
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;
        Ok(buf)
    } else if is_chunked(headers) {
        read_chunked(stream).await
    } else {
        Ok(Vec::new())
    }
}

async fn read_chunked(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = read_line(stream).await?;
        let size_field = line.trim().split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_field, 16)
            .map_err(|_| io::Error::other("invalid chunk size"))?;
        if size == 0 {
            let _ = read_line(stream).await?; // trailing CRLF
            break;
        }
        let mut chunk = vec![0u8; size];
        stream.read_exact(&mut chunk).await?;
        out.extend_from_slice(&chunk);
        let _ = read_line(stream).await?; // CRLF after the chunk data
    }
    Ok(out)
}

async fn read_line(stream: &mut TcpStream) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\n") {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(io::Error::other("chunk line too long"));
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// Write a full byte buffer and flush.
pub(crate) async fn write_all(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(bytes).await?;
    stream.flush().await
}
