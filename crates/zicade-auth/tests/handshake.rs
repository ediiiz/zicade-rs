//! M3 tests for the pure Negotiate handshake state machine, driven by a fake
//! authenticator. Covers single-leg (Kerberos-like) and multi-leg (NTLM 3-leg)
//! handshakes, the leg cap against infinite loops, rejection, and the
//! credential handle lifecycle (acquired once, released once).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use zicade_auth::{
    AuthError, HandshakeStep, NegotiateHandshake, UpstreamAuthenticator, UpstreamResponse,
};

#[derive(Clone, Default)]
struct Counters {
    acquired: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
}

enum Mode {
    Seq(VecDeque<Vec<u8>>),
    Infinite(Vec<u8>),
}

/// Fake SSPI authenticator: emits scripted tokens, records the challenges it
/// was fed, and counts acquire/release to assert handle lifecycle.
struct FakeAuth {
    tokens: Mode,
    challenges: Arc<Mutex<Vec<Option<Vec<u8>>>>>,
    counters: Counters,
}

impl FakeAuth {
    fn sequence(tokens: Vec<Vec<u8>>, counters: Counters) -> Self {
        counters.acquired.fetch_add(1, Ordering::SeqCst);
        Self {
            tokens: Mode::Seq(tokens.into()),
            challenges: Arc::default(),
            counters,
        }
    }

    fn infinite(token: Vec<u8>, counters: Counters) -> Self {
        counters.acquired.fetch_add(1, Ordering::SeqCst);
        Self {
            tokens: Mode::Infinite(token),
            challenges: Arc::default(),
            counters,
        }
    }

    fn challenges(&self) -> Vec<Option<Vec<u8>>> {
        self.challenges.lock().unwrap().clone()
    }
}

impl UpstreamAuthenticator for FakeAuth {
    fn step(&mut self, challenge: Option<&[u8]>) -> Result<Vec<u8>, AuthError> {
        self.challenges
            .lock()
            .unwrap()
            .push(challenge.map(<[u8]>::to_vec));
        match &mut self.tokens {
            Mode::Seq(q) => q
                .pop_front()
                .ok_or_else(|| AuthError::Authenticator("no more tokens".into())),
            Mode::Infinite(t) => Ok(t.clone()),
        }
    }
}

impl Drop for FakeAuth {
    fn drop(&mut self) {
        self.counters.released.fetch_add(1, Ordering::SeqCst);
    }
}

fn challenge(bytes: &[u8]) -> UpstreamResponse {
    UpstreamResponse::ProxyAuthRequired {
        challenge: Some(bytes.to_vec()),
    }
}

fn offer() -> UpstreamResponse {
    UpstreamResponse::ProxyAuthRequired { challenge: None }
}

#[test]
fn single_leg_handshake_completes() {
    let c = Counters::default();
    let mut hs = NegotiateHandshake::new(FakeAuth::sequence(vec![b"token1".to_vec()], c.clone()));

    match hs.on_response(offer()).unwrap() {
        HandshakeStep::SendToken(t) => assert_eq!(t, b"token1"),
        HandshakeStep::Complete => panic!("expected a token first"),
    }
    assert!(matches!(
        hs.on_response(UpstreamResponse::Success).unwrap(),
        HandshakeStep::Complete
    ));
    assert_eq!(hs.legs(), 1);
}

#[test]
fn ntlm_three_leg_handshake_completes() {
    let c = Counters::default();
    let mut hs = NegotiateHandshake::new(FakeAuth::sequence(
        vec![b"type1".to_vec(), b"type3".to_vec()],
        c.clone(),
    ));

    assert!(
        matches!(hs.on_response(offer()).unwrap(), HandshakeStep::SendToken(t) if t == b"type1")
    );
    assert!(
        matches!(hs.on_response(challenge(b"type2")).unwrap(), HandshakeStep::SendToken(t) if t == b"type3")
    );
    assert!(matches!(
        hs.on_response(UpstreamResponse::Success).unwrap(),
        HandshakeStep::Complete
    ));

    assert_eq!(
        hs.authenticator().challenges(),
        vec![None, Some(b"type2".to_vec())],
        "authenticator must be fed the server challenge on the second leg"
    );
    assert_eq!(hs.legs(), 2);
}

#[test]
fn leg_cap_rejects_infinite_loop() {
    let c = Counters::default();
    let mut hs =
        NegotiateHandshake::new(FakeAuth::infinite(b"tok".to_vec(), c.clone())).with_max_legs(3);

    let mut resp = offer();
    let mut result = Ok(());
    for _ in 0..20 {
        match hs.on_response(resp) {
            Ok(HandshakeStep::SendToken(_)) => resp = challenge(b"again"),
            Ok(HandshakeStep::Complete) => panic!("must not complete against a hostile server"),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    assert!(
        matches!(result, Err(AuthError::TooManyLegs { max: 3 })),
        "got {result:?}"
    );
}

#[test]
fn rejection_after_token_is_error() {
    let c = Counters::default();
    let mut hs = NegotiateHandshake::new(FakeAuth::sequence(vec![b"t1".to_vec()], c.clone()));

    assert!(matches!(
        hs.on_response(offer()).unwrap(),
        HandshakeStep::SendToken(_)
    ));
    // A 407 with no continuation token after we already sent one = rejected.
    let err = hs.on_response(offer()).unwrap_err();
    assert!(matches!(err, AuthError::Rejected), "got {err:?}");
}

#[test]
fn success_without_auth_completes_immediately() {
    let c = Counters::default();
    let mut hs = NegotiateHandshake::new(FakeAuth::sequence(vec![], c.clone()));
    assert!(matches!(
        hs.on_response(UpstreamResponse::Success).unwrap(),
        HandshakeStep::Complete
    ));
    assert_eq!(hs.legs(), 0);
}

#[test]
fn credential_handle_acquired_once_released_once() {
    let c = Counters::default();
    {
        let mut hs = NegotiateHandshake::new(FakeAuth::sequence(vec![b"t".to_vec()], c.clone()));
        hs.on_response(offer()).unwrap();
        hs.on_response(UpstreamResponse::Success).unwrap();
    } // handshake (and the authenticator it owns) dropped here

    assert_eq!(
        c.acquired.load(Ordering::SeqCst),
        1,
        "credential acquired exactly once"
    );
    assert_eq!(
        c.released.load(Ordering::SeqCst),
        1,
        "credential released exactly once"
    );
}
