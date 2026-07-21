#![forbid(unsafe_code)]

//! Zicade binary entrypoint: loads config, starts the proxy and web server,
//! and wires shutdown. Real wiring lands in M6; M0 is a placeholder that runs.

fn main() {
    println!("zicade (scaffold)");
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
