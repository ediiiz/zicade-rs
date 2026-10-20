//! Drop-downs show readable labels but store exactly the spellings the config
//! file uses, and every option maps back to the enum it names.

use zicade_config::{AuthMode, FailPolicy, PacSource, RoutingMode, SspiPackage};
use zicade_ui::choices::{
    AUTH_MODES, FAIL_POLICIES, Item, LOG_FORMATS, LOG_LEVELS, PAC_SOURCES, ROUTING_MODES,
    SSPI_PACKAGES, index_of, labels, name_of, parse,
};

fn round_trips<T: serde::Serialize + serde::de::DeserializeOwned>(options: &[Item]) {
    for option in options {
        let value: T = parse(option.name).unwrap_or_else(|| panic!("{} parses", option.name));
        assert_eq!(name_of(&value), option.name);
    }
}

#[test]
fn every_option_parses_and_names_itself() {
    round_trips::<RoutingMode>(&ROUTING_MODES);
    round_trips::<AuthMode>(&AUTH_MODES);
    round_trips::<SspiPackage>(&SSPI_PACKAGES);
    round_trips::<PacSource>(&PAC_SOURCES);
    round_trips::<FailPolicy>(&FAIL_POLICIES);
}

#[test]
fn an_unknown_name_does_not_parse() {
    assert_eq!(parse::<RoutingMode>("socks"), None);
}

#[test]
fn a_value_missing_from_the_list_selects_nothing() {
    // A hand-edited config may hold a level the drop-down does not offer; the
    // select then shows empty rather than silently picking another level.
    assert_eq!(index_of(&LOG_LEVELS, "info"), Some(2));
    assert_eq!(index_of(&LOG_LEVELS, "verbose"), None);
    assert_eq!(index_of(&LOG_FORMATS, "json"), Some(0));
}

#[test]
fn labels_are_readable_and_distinct_from_config_names() {
    assert_eq!(
        labels(&ROUTING_MODES),
        ["Direct", "Upstream proxy", "PAC script"]
    );
    for options in [&AUTH_MODES[..], &PAC_SOURCES[..], &FAIL_POLICIES[..]] {
        let shown = labels(options);
        let mut unique = shown.clone();
        unique.dedup();
        assert_eq!(shown, unique, "labels must tell options apart");
    }
}
