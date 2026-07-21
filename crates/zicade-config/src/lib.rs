#![forbid(unsafe_code)]

//! Configuration schema, load/save, validation, and migration for Zicade.
//!
//! The serde structs in this crate are the config schema (spec §5.3). Real
//! behavior lands in M1; M0 only proves the crate compiles and tests run.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_scaffold_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
