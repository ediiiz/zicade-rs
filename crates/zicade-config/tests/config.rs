//! M1 acceptance tests for `zicade-config` (spec §5.3 + validation rules).
//! Pure logic, no network, runs on any OS: platform-dependent rules take an
//! injected [`ValidationCtx`] rather than `cfg!(windows)`.

use zicade_config::{
    AuthMode, Config, ConfigError, FailPolicy, PacSource, RoutingMode, ValidationCtx,
    from_json_str, to_json_string,
};

/// The full spec §5.3 example, as valid JSON (comments stripped).
const FULL_EXAMPLE: &str = r#"{
  "listen":  { "host": "127.0.0.1", "port": 3129 },
  "routing": {
    "mode": "pac",
    "upstream": { "host": "wp8080", "port": 8080,
                  "auth": { "mode": "negotiate" } },
    "pac":      { "source": "auto",
                  "path": null, "url": null,
                  "failPolicy": "error",
                  "auth": { "mode": "negotiate" } }
  },
  "logging": { "level": "info", "format": "json" }
}"#;

#[test]
fn defaults_are_loopback_3129() {
    let cfg = Config::default();
    assert_eq!(cfg.listen.host, "127.0.0.1");
    assert_eq!(cfg.listen.port, 3129);
}

#[test]
fn parses_full_example_schema() {
    let cfg = from_json_str(FULL_EXAMPLE).expect("full example should parse");
    assert_eq!(cfg.listen.port, 3129);
    assert_eq!(cfg.routing.mode, RoutingMode::Pac);

    let up = cfg.routing.upstream.as_ref().expect("upstream present");
    assert_eq!(up.host, "wp8080");
    assert_eq!(up.port, 8080);
    assert_eq!(up.auth.mode, AuthMode::Negotiate);

    let pac = cfg.routing.pac.as_ref().expect("pac present");
    assert_eq!(pac.source, PacSource::Auto);
    assert_eq!(pac.fail_policy, FailPolicy::Error);
    assert_eq!(pac.auth.mode, AuthMode::Negotiate);
    assert_eq!(cfg.logging.level, "info");
    assert_eq!(cfg.logging.format, "json");
}

#[test]
fn round_trips_through_json() {
    let cfg = from_json_str(FULL_EXAMPLE).unwrap();
    let json = to_json_string(&cfg).unwrap();
    let reparsed = from_json_str(&json).unwrap();
    assert_eq!(cfg, reparsed, "save then load must be lossless");
}

#[test]
fn rejects_unknown_routing_mode() {
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "sideways" } }"#;
    let err = from_json_str(json).expect_err("unknown mode must be rejected");
    assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
}

#[test]
fn rejects_unknown_fail_policy() {
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "pac",
                                 "pac": { "source": "auto", "failPolicy": "yolo" } } }"#;
    let err = from_json_str(json).expect_err("unknown failPolicy must be rejected");
    assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
}

#[test]
fn rejects_zero_listen_port() {
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 0 },
                    "routing": { "mode": "direct" } }"#;
    let cfg = from_json_str(json).unwrap();
    let err = cfg
        .validate(&ValidationCtx::windows())
        .expect_err("port 0 is invalid");
    assert!(
        matches!(err, ConfigError::InvalidPort { .. }),
        "got {err:?}"
    );
}

#[test]
fn upstream_mode_requires_upstream_section() {
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "upstream" } }"#;
    let cfg = from_json_str(json).unwrap();
    let err = cfg
        .validate(&ValidationCtx::windows())
        .expect_err("upstream mode without upstream section is invalid");
    assert!(
        matches!(err, ConfigError::MissingSection { .. }),
        "got {err:?}"
    );
}

#[test]
fn pac_mode_requires_pac_section() {
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "pac" } }"#;
    let cfg = from_json_str(json).unwrap();
    let err = cfg
        .validate(&ValidationCtx::windows())
        .expect_err("pac mode without pac section is invalid");
    assert!(
        matches!(err, ConfigError::MissingSection { .. }),
        "got {err:?}"
    );
}

#[test]
fn negotiate_rejected_when_platform_lacks_support() {
    // Same config, two platforms: negotiate must fail off-Windows and pass on it.
    let cfg = from_json_str(FULL_EXAMPLE).unwrap();

    let off = ValidationCtx {
        negotiate_supported: false,
    };
    let err = cfg
        .validate(&off)
        .expect_err("negotiate must be rejected without platform support");
    assert!(
        matches!(err, ConfigError::NegotiateUnsupported { .. }),
        "got {err:?}"
    );

    let on = ValidationCtx {
        negotiate_supported: true,
    };
    cfg.validate(&on)
        .expect("negotiate is valid with platform support");
}

#[test]
fn negotiate_in_pac_auth_also_checked() {
    // Only the PAC auth uses negotiate; still rejected off-Windows.
    let json = r#"{ "listen": { "host": "127.0.0.1", "port": 3129 },
                    "routing": { "mode": "pac",
                                 "pac": { "source": "auto", "failPolicy": "direct",
                                          "auth": { "mode": "negotiate" } } } }"#;
    let cfg = from_json_str(json).unwrap();
    let err = cfg
        .validate(&ValidationCtx {
            negotiate_supported: false,
        })
        .expect_err("pac.auth negotiate must be checked too");
    match err {
        ConfigError::NegotiateUnsupported { field } => {
            assert!(field.contains("pac"), "field was {field}");
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn pac_selected_upstream_inherits_pac_auth() {
    // LESSON-6: a PAC result pointing at a gateway must carry routing.pac.auth
    // so the handshake can run. Internal (DIRECT) hosts carry no upstream.
    let cfg = from_json_str(FULL_EXAMPLE).unwrap();
    let pac_auth = cfg.routing.pac.as_ref().unwrap().auth.clone();

    let effective = cfg
        .routing
        .pac_upstream("gateway.example.com", 8080)
        .expect("pac_upstream builds an upstream when pac config is present");

    assert_eq!(effective.host, "gateway.example.com");
    assert_eq!(effective.port, 8080);
    assert_eq!(
        effective.auth, pac_auth,
        "PAC-selected upstream must inherit pac.auth"
    );
    assert_eq!(effective.auth.mode, AuthMode::Negotiate);
}
