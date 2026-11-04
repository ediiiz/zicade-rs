//! `routing.pool` schema + validation: off unless configured, bounded when on,
//! and invisible in saved configs that never set it.

use zicade_config::{ConfigError, PoolConfig, ValidationCtx, from_json_str, to_json_string};

/// A minimal `upstream` config carrying the given `pool` JSON fragment (or an
/// empty string to omit the section entirely).
fn upstream_with_pool(pool_fragment: &str) -> String {
    format!(
        r#"{{ "routing": {{ "mode": "upstream",
                           "upstream": {{ "host": "proxy.example.com", "port": 8080 }}{pool_fragment} }} }}"#
    )
}

#[test]
fn pool_is_off_when_absent() {
    let cfg = from_json_str(&upstream_with_pool("")).expect("parses without pool");
    assert_eq!(cfg.routing.pool, PoolConfig::default());
    assert!(!cfg.routing.pool.enabled, "holding sockets open is opt-in");
    cfg.validate(&ValidationCtx::windows())
        .expect("no pool section is valid");
}

#[test]
fn enabling_the_pool_takes_modest_defaults() {
    let cfg = from_json_str(&upstream_with_pool(r#", "pool": { "enabled": true }"#)).unwrap();
    let pool = cfg.routing.pool;
    assert!(pool.enabled);
    assert_eq!(pool.min_idle, 2);
    assert_eq!(pool.idle_timeout_seconds, 30);
    cfg.validate(&ValidationCtx::windows())
        .expect("defaults are valid");
}

#[test]
fn pool_fields_parse_in_camel_case() {
    let cfg = from_json_str(&upstream_with_pool(
        r#", "pool": { "enabled": true, "minIdle": 4, "idleTimeoutSeconds": 15 }"#,
    ))
    .unwrap();
    assert_eq!(
        cfg.routing.pool,
        PoolConfig {
            enabled: true,
            min_idle: 4,
            idle_timeout_seconds: 15,
        }
    );
}

#[test]
fn enabled_pool_rejects_out_of_range_limits() {
    for fragment in [
        r#", "pool": { "enabled": true, "minIdle": 0 }"#,
        r#", "pool": { "enabled": true, "minIdle": 17 }"#,
        r#", "pool": { "enabled": true, "idleTimeoutSeconds": 0 }"#,
    ] {
        let cfg = from_json_str(&upstream_with_pool(fragment)).unwrap();
        let err = cfg.validate(&ValidationCtx::windows()).expect_err(fragment);
        assert!(matches!(err, ConfigError::Pool { .. }), "got {err:?}");
    }
}

#[test]
fn disabled_pool_carries_no_requirements() {
    let cfg = from_json_str(&upstream_with_pool(
        r#", "pool": { "enabled": false, "minIdle": 0 }"#,
    ))
    .unwrap();
    cfg.validate(&ValidationCtx::windows())
        .expect("a disabled pool is not checked");
}

#[test]
fn default_pool_is_not_written_and_a_set_one_round_trips() {
    let plain = from_json_str(&upstream_with_pool("")).unwrap();
    let json = to_json_string(&plain).unwrap();
    assert!(
        !json.contains("pool"),
        "an unset pool stays out of the file"
    );

    let set = from_json_str(&upstream_with_pool(
        r#", "pool": { "enabled": true, "minIdle": 3 }"#,
    ))
    .unwrap();
    let reparsed = from_json_str(&to_json_string(&set).unwrap()).unwrap();
    assert_eq!(reparsed.routing.pool, set.routing.pool);
}
