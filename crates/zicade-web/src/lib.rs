#![forbid(unsafe_code)]

//! Local web API + htmx UI (axum): config CRUD, status, and SSE log streaming.
//! Loopback-only bind with a local-token gate on mutations. Behavior lands in
//! M5; M0 only proves the crate compiles.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
