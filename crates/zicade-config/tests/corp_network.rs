//! Acceptance tests for the `routing.corpNetwork` gate schema + validation.
//! Pure logic, runs on any OS.

use zicade_config::{ConfigError, ValidationCtx, from_json_str, to_json_string};

/// A minimal `pac` config carrying the given `corpNetwork` JSON fragment (or an
/// empty string to omit the section entirely).
fn pac_with_corp(corp_fragment: &str) -> String {
    format!(
        r#"{{ "listen": {{ "host": "127.0.0.1", "port": 3129 }},
              "routing": {{ "mode": "pac",
                            "pac": {{ "source": "auto", "failPolicy": "error" }}{corp_fragment} }} }}"#
    )
}

#[test]
fn corp_network_absent_is_none_and_valid() {
    // Older configs without a corpNetwork section load with the gate off.
    let cfg = from_json_str(&pac_with_corp("")).expect("parses without corpNetwork");
    assert!(cfg.routing.corp_network.is_none());
    cfg.validate(&ValidationCtx::windows())
        .expect("no corpNetwork section is valid");
}

#[test]
fn corp_network_parses_and_defaults_poll_seconds() {
    let cfg = from_json_str(&pac_with_corp(
        r#", "corpNetwork": { "enabled": true, "dnsSuffixes": ["droot.org"] }"#,
    ))
    .expect("corpNetwork parses");
    let corp = cfg
        .routing
        .corp_network
        .as_ref()
        .expect("corpNetwork present");
    assert!(corp.enabled);
    assert_eq!(corp.dns_suffixes, vec!["droot.org".to_owned()]);
    assert_eq!(
        corp.poll_seconds, 30,
        "pollSeconds defaults to 30 when omitted"
    );
    cfg.validate(&ValidationCtx::windows())
        .expect("enabled with a suffix is valid");
}

#[test]
fn corp_network_enabled_requires_a_suffix() {
    let cfg = from_json_str(&pac_with_corp(
        r#", "corpNetwork": { "enabled": true, "dnsSuffixes": [] }"#,
    ))
    .unwrap();
    let err = cfg
        .validate(&ValidationCtx::windows())
        .expect_err("enabled without a suffix is invalid");
    assert!(
        matches!(err, ConfigError::CorpNetwork { .. }),
        "got {err:?}"
    );
}

#[test]
fn corp_network_rejects_zero_poll_seconds() {
    let cfg = from_json_str(&pac_with_corp(
        r#", "corpNetwork": { "enabled": true, "dnsSuffixes": ["droot.org"], "pollSeconds": 0 }"#,
    ))
    .unwrap();
    let err = cfg
        .validate(&ValidationCtx::windows())
        .expect_err("pollSeconds 0 is invalid");
    assert!(
        matches!(err, ConfigError::CorpNetwork { .. }),
        "got {err:?}"
    );
}

#[test]
fn corp_network_disabled_needs_no_suffix() {
    // A disabled gate carries no requirements.
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "direct",
                                 "corpNetwork": { "enabled": false, "dnsSuffixes": [] } } }"#;
    let cfg = from_json_str(json).unwrap();
    cfg.validate(&ValidationCtx::windows())
        .expect("disabled gate is valid without suffixes");
}

#[test]
fn corp_network_round_trips() {
    let cfg = from_json_str(&pac_with_corp(
        r#", "corpNetwork": { "enabled": true, "dnsSuffixes": ["droot.org", "corp.example"], "pollSeconds": 15 }"#,
    ))
    .unwrap();
    let reparsed = from_json_str(&to_json_string(&cfg).unwrap()).unwrap();
    assert_eq!(cfg, reparsed, "save then load must be lossless");
    assert_eq!(
        reparsed.routing.corp_network.as_ref().unwrap().poll_seconds,
        15
    );
}
