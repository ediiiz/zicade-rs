//! Corporate-network detection gate.
//!
//! When `routing.corpNetwork` is enabled, a background monitor decides whether
//! the machine is on the corporate network by matching the active adapters' DNS
//! suffixes against the configured list. On-corp the configured routing
//! (`pac`/`upstream`) applies; off-corp the proxy routes `Direct`.
//!
//! This module keeps the *decision* ([`on_corp`]) pure and OS-independent so it
//! is unit-testable everywhere; the platform suffix enumeration and network
//! change events live in `zicade-win` and are injected into the supervisor.

/// Decide whether any `active` DNS suffix indicates the corporate network,
/// given the `configured` corporate suffixes.
///
/// Matching is case-insensitive and dot-boundary aware: an active suffix matches
/// a configured suffix `c` when it equals `c` or ends with `.c` (so
/// `dy.droot.org` matches `droot.org`, but `notdroot.org` does not). Leading and
/// trailing dots and surrounding whitespace are ignored on both sides; empty
/// entries never match.
// The supervisor that consumes this lands in M4; until then only the unit tests
// exercise it. TODO(M4): remove this allow once `NetworkGate` calls `on_corp`.
#[allow(dead_code)]
pub(crate) fn on_corp(active: &[String], configured: &[String]) -> bool {
    let wanted: Vec<String> = configured.iter().filter_map(|c| normalize(c)).collect();
    if wanted.is_empty() {
        return false;
    }
    active
        .iter()
        .filter_map(|a| normalize(a))
        .any(|a| wanted.iter().any(|c| suffix_matches(&a, c)))
}

/// Lower-case, trim whitespace, and strip leading/trailing dots. Returns `None`
/// for entries that are empty once normalized.
fn normalize(s: &str) -> Option<String> {
    let t = s.trim().trim_matches('.').to_ascii_lowercase();
    if t.is_empty() { None } else { Some(t) }
}

/// True when normalized suffix `a` equals `c` or sits below it on a dot boundary.
fn suffix_matches(a: &str, c: &str) -> bool {
    a == c || a.ends_with(&format!(".{c}"))
}

#[cfg(test)]
mod tests {
    use super::on_corp;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn exact_suffix_matches() {
        assert!(on_corp(&v(&["droot.org"]), &v(&["droot.org"])));
    }

    #[test]
    fn subdomain_matches_on_dot_boundary() {
        assert!(on_corp(&v(&["dy.droot.org"]), &v(&["droot.org"])));
    }

    #[test]
    fn is_case_insensitive() {
        assert!(on_corp(&v(&["DY.DRoot.Org"]), &v(&["droot.org"])));
    }

    #[test]
    fn ignores_leading_dots_and_whitespace() {
        assert!(on_corp(&v(&["  .dy.droot.org. "]), &v(&[" .droot.org "])));
    }

    #[test]
    fn non_dot_boundary_does_not_match() {
        // "notdroot.org" must NOT be treated as inside "droot.org".
        assert!(!on_corp(&v(&["notdroot.org"]), &v(&["droot.org"])));
    }

    #[test]
    fn no_active_suffix_is_off_corp() {
        assert!(!on_corp(&[], &v(&["droot.org"])));
    }

    #[test]
    fn empty_configured_is_off_corp() {
        // A misconfigured/empty suffix list never reports on-corp.
        assert!(!on_corp(&v(&["dy.droot.org"]), &[]));
        assert!(!on_corp(&v(&["dy.droot.org"]), &v(&["", "  "])));
    }

    #[test]
    fn matches_any_of_several_configured() {
        let configured = v(&["corp.example", "droot.org"]);
        assert!(on_corp(&v(&["home.lan", "dy.droot.org"]), &configured));
        assert!(!on_corp(&v(&["home.lan", "guest.wifi"]), &configured));
    }
}
