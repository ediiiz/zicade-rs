//! A minimal raw-TCP HTTP client that speaks proxy (absolute-form) requests and
//! `CONNECT`, so tests control request framing precisely.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Request body framing to exercise.
pub enum ReqBody {
    None,
    ContentLength(Vec<u8>),
    Chunked(Vec<u8>),
}

/// Parsed HTTP response.
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// Send an absolute-form proxy request over a fresh connection and return the
/// parsed response. Always sends `Connection: close`.
pub async fn proxy_http(
    proxy: SocketAddr,
    method: &str,
    url: &str,
    extra: &[(&str, &str)],
    body: ReqBody,
) -> HttpResponse {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let host = authority_of(url);
    let mut head = format!("{method} {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    let mut payload = Vec::new();
    match &body {
        ReqBody::None => head.push_str("\r\n"),
        ReqBody::ContentLength(b) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", b.len()));
            payload = b.clone();
        }
        ReqBody::Chunked(b) => {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
            payload.extend_from_slice(format!("{:x}\r\n", b.len()).as_bytes());
            payload.extend_from_slice(b);
            payload.extend_from_slice(b"\r\n0\r\n\r\n");
        }
    }
    stream.write_all(head.as_bytes()).await.unwrap();
    if !payload.is_empty() {
        stream.write_all(&payload).await.unwrap();
    }
    stream.flush().await.unwrap();
    read_response(&mut stream).await
}

/// Open a `CONNECT` tunnel through the proxy, asserting a 200 reply, and return
/// the (now spliced) stream for raw byte exchange with the target.
pub async fn proxy_connect(proxy: SocketAddr, target: &str) -> TcpStream {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while find(&buf, b"\r\n\r\n").is_none() {
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let status = parse_status(text.lines().next().unwrap_or(""));
    assert_eq!(status, 200, "CONNECT should return 200, got:\n{text}");
    stream
}

async fn read_response(stream: &mut TcpStream) -> HttpResponse {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut tmp).await.unwrap();
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let status = parse_status(lines.next().unwrap_or(""));
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let mut body = buf[header_end..].to_vec();
    if let Some(cl) = headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
    {
        while body.len() < cl {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(cl);
    } else {
        loop {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
    }
    HttpResponse {
        status,
        headers,
        body,
    }
}

fn authority_of(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    rest.split('/').next().unwrap_or(rest).to_string()
}

fn parse_status(status_line: &str) -> u16 {
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
