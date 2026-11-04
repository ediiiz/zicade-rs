//! Corporate-network detection gate.
//!
//! When `routing.corpNetwork` is enabled, a background monitor decides whether
//! the machine is on the corporate network by matching the active adapters' DNS
//! suffixes against the configured list. On-corp the configured routing
//! (`pac`/`upstream`) applies; off-corp the proxy routes `Direct`.
//!
//! This module keeps the *decision* ([`on_corp`]) pure and OS-independent so it
//! is unit-testable everywhere; the [`gate`] submodule holds the supervisor,
//! into which the platform suffix enumeration and network change events (from
//! `zicade-win`) are injected.

mod gate;

pub(crate) use gate::{NetworkGate, gate_enabled};

/// Decide whether any `active` DNS suffix indicates the corporate network,
/// given the `configured` corporate suffixes.
///
/// Matching is case-insensitive and dot-boundary aware: an active suffix matches
/// a configured suffix `c` when it equals `c` or ends with `.c` (so
/// `site.ad.example.org` matches `ad.example.org`, but `notad.example.org` does not). Leading and
/// trailing dots and surrounding whitespace are ignored on both sides; empty
/// entries never match.
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
        assert!(on_corp(&v(&["ad.example.org"]), &v(&["ad.example.org"])));
    }

    #[test]
    fn subdomain_matches_on_dot_boundary() {
        assert!(on_corp(
            &v(&["site.ad.example.org"]),
            &v(&["ad.example.org"])
        ));
    }

    #[test]
    fn is_case_insensitive() {
        assert!(on_corp(
            &v(&["SITE.AD.Example.Org"]),
            &v(&["ad.example.org"])
        ));
    }

    #[test]
    fn ignores_leading_dots_and_whitespace() {
        assert!(on_corp(
            &v(&["  .site.ad.example.org. "]),
            &v(&[" .ad.example.org "])
        ));
    }

    #[test]
    fn non_dot_boundary_does_not_match() {
        // "notad.example.org" must NOT be treated as inside "ad.example.org".
        assert!(!on_corp(
            &v(&["notad.example.org"]),
            &v(&["ad.example.org"])
        ));
    }

    #[test]
    fn no_active_suffix_is_off_corp() {
        assert!(!on_corp(&[], &v(&["ad.example.org"])));
    }

    #[test]
    fn empty_configured_is_off_corp() {
        // A misconfigured/empty suffix list never reports on-corp.
        assert!(!on_corp(&v(&["site.ad.example.org"]), &[]));
        assert!(!on_corp(&v(&["site.ad.example.org"]), &v(&["", "  "])));
    }

    #[test]
    fn matches_any_of_several_configured() {
        let configured = v(&["corp.example", "ad.example.org"]);
        assert!(on_corp(
            &v(&["home.lan", "site.ad.example.org"]),
            &configured
        ));
        assert!(!on_corp(&v(&["home.lan", "guest.wifi"]), &configured));
    }
}
