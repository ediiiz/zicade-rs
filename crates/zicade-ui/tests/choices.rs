//! Drop-downs offer exactly the spellings the config file uses, and every
//! option maps back to the enum it names.

use zicade_config::{AuthMode, FailPolicy, PacSource, RoutingMode, SspiPackage};
use zicade_ui::choices::{
    AUTH_MODES, FAIL_POLICIES, LOG_FORMATS, LOG_LEVELS, PAC_SOURCES, ROUTING_MODES, SSPI_PACKAGES,
    index_of, name_of, parse,
};

#[test]
fn every_option_parses_and_names_itself() {
    for name in ROUTING_MODES {
        let mode: RoutingMode = parse(name).unwrap();
        assert_eq!(name_of(&mode), name);
    }
    for name in AUTH_MODES {
        let mode: AuthMode = parse(name).unwrap();
        assert_eq!(name_of(&mode), name);
    }
    for name in SSPI_PACKAGES {
        let package: SspiPackage = parse(name).unwrap();
        assert_eq!(name_of(&package), name);
    }
    for name in PAC_SOURCES {
        let source: PacSource = parse(name).unwrap();
        assert_eq!(name_of(&source), name);
    }
    for name in FAIL_POLICIES {
        let policy: FailPolicy = parse(name).unwrap();
        assert_eq!(name_of(&policy), name);
    }
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
