#![forbid(unsafe_code)]

//! The proxy data path: accept loop, HTTP forwarding, and `CONNECT` tunneling,
//! with per-connection isolation and graceful shutdown. Behavior lands in M2;
//! M0 only proves the crate compiles.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
