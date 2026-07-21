#![forbid(unsafe_code)]

//! Upstream authentication: the `UpstreamAuthenticator` trait and the pure,
//! testable Negotiate handshake state machine that drives the challenge/response
//! legs against an upstream 407. The real SSPI impl lives in `zicade-win`; tests
//! inject a fake. Behavior lands in M3; M0 only proves the crate compiles.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
