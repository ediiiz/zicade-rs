#![forbid(unsafe_code)]

//! Observability: a `tracing` layer that fans structured events into a bounded
//! in-memory ring buffer and a broadcast channel for the SSE log stream.
//! Behavior lands in M5; M0 only proves the crate compiles.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
