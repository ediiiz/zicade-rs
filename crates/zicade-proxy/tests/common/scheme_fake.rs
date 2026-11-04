//! Fake upstream proxy whose 407s advertise a scripted list of
//! `Proxy-Authenticate` schemes, plus an authenticator that checks it is fed
//! the gateway's challenge. Lets tests pin which keyword the proxy answers
//! with when a gateway speaks `NTLM`, `Negotiate`, or neither.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use zicade_auth::{AuthError, UpstreamAuthenticator};
use zicade_proxy::{AuthFactory, UpstreamAuth};

use super::upstream_fake::{echo_tunnel, read_request};

/// The continuation token the fake hands back on the first authed leg.
const SERVER_CHALLENGE: &[u8] = b"srv-challenge";

/// Placeholder in a scripted offer, replaced by the base64 server challenge.
const CHALLENGE: &str = "{challenge}";

/// What the fake gateway advertises and accepts.
pub struct SchemeScript {
    /// `Proxy-Authenticate` values, in order, on the 407 to an unauthed leg.
    pub bare: Vec<&'static str>,
    /// `Proxy-Authenticate` values on the 407 to the first authed leg;
    /// `{challenge}` is replaced by the server's continuation token.
    pub challenge: Vec<&'static str>,
    /// The keyword every authed leg must use. A leg with any other keyword gets
    /// a 403 and the connection is closed. `None` scripts a gateway that
    /// answers every leg with the bare 407, whatever it is sent.
    pub accept: Option<&'static str>,
}

/// A running fake gateway and every `Proxy-Authorization` value it received.
pub struct SchemeUpstream {
    pub addr: SocketAddr,
    sent: Arc<Mutex<Vec<String>>>,
}

impl SchemeUpstream {
    /// The `Proxy-Authorization` values received so far, across connections.
    pub fn authorizations(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

/// `Proxy-Authorization` value the fake authenticator's `n`th token produces
/// under `keyword`.
pub fn expected_header(keyword: &str, n: usize) -> String {
    format!("{keyword} {}", STANDARD.encode(format!("tok{n}")))
}

/// Spawn the fake gateway: bare 407 -> challenge 407 -> 200. On 200 a CONNECT
/// becomes an echo tunnel and a plain request echoes its body.
pub async fn spawn_scheme_upstream(script: SchemeScript) -> SchemeUpstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(script);
    let recorder = Arc::clone(&sent);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream, Arc::clone(&script), Arc::clone(&recorder)));
        }
    });
    SchemeUpstream { addr, sent }
}

async fn serve(mut stream: TcpStream, script: Arc<SchemeScript>, sent: Arc<Mutex<Vec<String>>>) {
    let mut authed_legs = 0u32;
    while let Some((head, body)) = read_request(&mut stream).await {
        let auth = authorization(&head);
        if let Some(value) = &auth {
            sent.lock().unwrap().push(value.clone());
        }
        let Some(accept) = script.accept else {
            if write_407(&mut stream, &script.bare).await.is_err() {
                return;
            }
            continue;
        };
        let Some(value) = auth else {
            if write_407(&mut stream, &script.bare).await.is_err() {
                return;
            }
            continue;
        };
        if !value.starts_with(&format!("{accept} ")) {
            let _ = stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
        authed_legs += 1;
        if authed_legs == 1 {
            if write_407(&mut stream, &script.challenge).await.is_err() {
                return;
            }
            continue;
        }
        grant(stream, head.starts_with("CONNECT"), body).await;
        return;
    }
}

async fn write_407(stream: &mut TcpStream, offers: &[&str]) -> std::io::Result<()> {
    let challenge = STANDARD.encode(SERVER_CHALLENGE);
    let mut resp = String::from("HTTP/1.1 407 Proxy Authentication Required\r\n");
    for offer in offers {
        let offer = offer.replace(CHALLENGE, &challenge);
        resp.push_str(&format!("Proxy-Authenticate: {offer}\r\n"));
    }
    resp.push_str("Content-Length: 0\r\nConnection: keep-alive\r\n\r\n");
    stream.write_all(resp.as_bytes()).await
}

async fn grant(mut stream: TcpStream, is_connect: bool, body: Vec<u8>) {
    if is_connect {
        let _ = stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await;
        echo_tunnel(stream).await;
        return;
    }
    let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.write_all(&body).await;
}

/// The `Proxy-Authorization` value in a request head, if any.
fn authorization(head: &str) -> Option<String> {
    head.split("\r\n").find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("proxy-authorization")
            .then(|| v.trim().to_owned())
    })
}

/// A two-leg authenticator that fails unless the proxy hands it no challenge
/// on the first leg and exactly the gateway's challenge on the second, so a
/// test also proves the challenge was found among several offers.
struct ChallengeCheckingAuth {
    leg: usize,
}

impl UpstreamAuthenticator for ChallengeCheckingAuth {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        let expected = match self.leg {
            0 => None,
            1 => Some(SERVER_CHALLENGE),
            _ => return Err(AuthError::Authenticator("fake ran out of tokens".into())),
        };
        if challenge != expected {
            return Err(AuthError::Authenticator(format!(
                "leg {} got challenge {challenge:?}, expected {expected:?}",
                self.leg
            )));
        }
        self.leg += 1;
        Ok(format!("tok{}", self.leg).into_bytes())
    }
}

/// An [`UpstreamAuth::Negotiate`] building a fresh challenge-checking fake per
/// upstream connection.
pub fn challenge_checking_factory() -> UpstreamAuth {
    let factory: AuthFactory = Arc::new(|| {
        Box::new(ChallengeCheckingAuth { leg: 0 }) as Box<dyn UpstreamAuthenticator + Send>
    });
    UpstreamAuth::Negotiate(factory)
}
