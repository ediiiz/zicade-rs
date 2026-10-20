//! The configuration form edits plain text fields; turning them back into a
//! [`Config`] is where typos, blank passwords, and hidden sections are decided.

use zicade_config::{
    AuthConfig, AuthMode, Config, CorpNetworkConfig, FailPolicy, PacConfig, PacSource, RoutingMode,
    SspiPackage, UpstreamConfig,
};
use zicade_ui::form_model::ConfigForm;

fn upstream_config() -> Config {
    let mut cfg = Config::default();
    cfg.routing.mode = RoutingMode::Upstream;
    cfg.routing.upstream = Some(UpstreamConfig {
        host: "proxy.example".to_owned(),
        port: 8080,
        auth: AuthConfig {
            mode: AuthMode::Negotiate,
            package: SspiPackage::Negotiate,
            username: None,
            password: None,
        },
    });
    cfg.routing.corp_network = Some(CorpNetworkConfig {
        enabled: true,
        dns_suffixes: vec!["corp.example".to_owned(), "lab.example".to_owned()],
        poll_seconds: 45,
    });
    cfg
}

fn pac_config() -> Config {
    let mut cfg = Config::default();
    cfg.routing.mode = RoutingMode::Pac;
    cfg.routing.pac = Some(PacConfig {
        source: PacSource::Url,
        path: None,
        url: Some("http://wpad.example/proxy.pac".to_owned()),
        fail_policy: FailPolicy::Direct,
        auth: AuthConfig {
            mode: AuthMode::Basic,
            username: Some("alice".to_owned()),
            ..AuthConfig::default()
        },
    });
    cfg
}

#[test]
fn an_unedited_form_gives_back_the_config_it_was_built_from() {
    for cfg in [Config::default(), upstream_config(), pac_config()] {
        let form = ConfigForm::from_config(&cfg);
        assert_eq!(form.to_config(), Ok(cfg));
    }
}

/// The form only ever sees redacted configs; a blank password must reach
/// `Control::apply` as `None` so the saved password is kept.
#[test]
fn a_blank_password_is_sent_as_none() {
    let mut cfg = upstream_config();
    let up = cfg.routing.upstream.as_mut().unwrap();
    up.auth.mode = AuthMode::Basic;
    up.auth.username = Some("alice".to_owned());
    let mut form = ConfigForm::from_config(&cfg);
    form.upstream_auth.password = "   ".to_owned();

    let out = form.to_config().unwrap();

    assert_eq!(out.routing.upstream.unwrap().auth.password, None);
}

#[test]
fn a_typed_password_is_sent() {
    let mut form = ConfigForm::from_config(&pac_config());
    form.pac_auth.password = "s3cret".to_owned();

    let out = form.to_config().unwrap();

    assert_eq!(
        out.routing.pac.unwrap().auth.password.as_deref(),
        Some("s3cret")
    );
}

#[test]
fn only_the_selected_mode_section_is_saved() {
    // Switching from upstream to direct must not keep validating (and failing
    // on) a half-filled upstream section the user can no longer see.
    let mut form = ConfigForm::from_config(&upstream_config());
    form.mode = RoutingMode::Direct;

    let out = form.to_config().unwrap();

    assert_eq!(out.routing.upstream, None);
    assert_eq!(out.routing.pac, None);
    assert_eq!(
        out.routing.corp_network, None,
        "the gate only applies to pac/upstream"
    );
}

#[test]
fn ports_that_are_not_numbers_are_reported_by_field() {
    let mut form = ConfigForm::from_config(&upstream_config());
    form.upstream_port = "80a".to_owned();
    let err = form.to_config().unwrap_err().to_string();
    assert!(err.contains("Upstream port"), "{err}");

    let mut form = ConfigForm::from_config(&Config::default());
    form.listen_port = "70000".to_owned();
    let err = form.to_config().unwrap_err().to_string();
    assert!(err.contains("Listen port"), "{err}");
}

#[test]
fn dns_suffixes_split_on_commas_and_newlines_and_drop_blanks() {
    let mut form = ConfigForm::from_config(&upstream_config());
    form.corp_suffixes = " corp.example,\n\nlab.example , ".to_owned();

    let corp = form.to_config().unwrap().routing.corp_network.unwrap();

    assert_eq!(corp.dns_suffixes, ["corp.example", "lab.example"]);
}

#[test]
fn text_fields_are_trimmed_and_blank_optionals_dropped() {
    let mut form = ConfigForm::from_config(&pac_config());
    form.listen_host = " 127.0.0.1 ".to_owned();
    form.pac_url = "  ".to_owned();
    form.pac_auth.username = String::new();

    let out = form.to_config().unwrap();

    assert_eq!(out.listen.host, "127.0.0.1");
    let pac = out.routing.pac.unwrap();
    assert_eq!(pac.url, None);
    assert_eq!(pac.auth.username, None);
}
