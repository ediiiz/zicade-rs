#![forbid(unsafe_code)]

//! Routing: `RoutingMode`, the `RouteResolver` and `PacBackend` traits, and
//! PAC-result parsing. Traits live here so tests inject fakes and the real
//! WinHTTP backend (in `zicade-win`) is one impl among several. Behavior lands
//! in M4; M0 only proves the crate compiles.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
