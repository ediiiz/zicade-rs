//! The form's widget state, and the copy to and from a [`ConfigForm`].

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::choices::{
    AUTH_MODES, FAIL_POLICIES, LOG_FORMATS, LOG_LEVELS, PAC_SOURCES, ROUTING_MODES, SSPI_PACKAGES,
    index_of, name_of, parse,
};
use crate::form_model::{AuthForm, ConfigForm};

pub(super) type Choice = Entity<SelectState<Vec<&'static str>>>;
pub(super) type Text = Entity<InputState>;

/// One auth slot's widgets.
pub(super) struct AuthFields {
    pub mode: Choice,
    pub package: Choice,
    pub username: Text,
    pub password: Text,
}

/// Every widget on the form.
pub(super) struct Fields {
    pub listen_host: Text,
    pub listen_port: Text,
    pub mode: Choice,
    pub upstream_host: Text,
    pub upstream_port: Text,
    pub upstream_auth: AuthFields,
    pub pac_source: Choice,
    pub pac_path: Text,
    pub pac_url: Text,
    pub pac_fail_policy: Choice,
    pub pac_auth: AuthFields,
    pub corp_suffixes: Text,
    pub corp_poll_seconds: Text,
    pub log_level: Choice,
    pub log_format: Choice,
}

/// Builds widgets on a view and re-renders it when a choice changes, since
/// choices decide which sections show.
pub(super) struct Builder<'a, 'b, V: 'static> {
    pub window: &'a mut Window,
    pub cx: &'a mut Context<'b, V>,
    pub subscriptions: Vec<Subscription>,
}

impl<V: 'static> Builder<'_, '_, V> {
    fn text(&mut self, value: &str, placeholder: &str) -> Text {
        let (value, placeholder) = (value.to_owned(), placeholder.to_owned());
        self.cx.new(|cx| {
            InputState::new(self.window, cx)
                .placeholder(placeholder)
                .default_value(value)
        })
    }

    fn secret(&mut self) -> Text {
        self.cx.new(|cx| {
            InputState::new(self.window, cx)
                .placeholder("Leave blank to keep the saved password")
                .masked(true)
        })
    }

    fn choice(&mut self, options: &'static [&'static str], selected: &str) -> Choice {
        let index = index_of(options, selected).map(IndexPath::new);
        let select = self
            .cx
            .new(|cx| SelectState::new(options.to_vec(), index, self.window, cx));
        let sub = self.cx.subscribe_in(
            &select,
            self.window,
            |_, _, _: &SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        );
        self.subscriptions.push(sub);
        select
    }

    fn auth(&mut self, auth: &AuthForm) -> AuthFields {
        AuthFields {
            mode: self.choice(&AUTH_MODES, &name_of(&auth.mode)),
            package: self.choice(&SSPI_PACKAGES, &name_of(&auth.package)),
            username: self.text(&auth.username, "DOMAIN\\user"),
            password: self.secret(),
        }
    }

    pub fn build(&mut self, form: &ConfigForm) -> Fields {
        Fields {
            listen_host: self.text(&form.listen_host, "127.0.0.1"),
            listen_port: self.text(&form.listen_port, "3129"),
            mode: self.choice(&ROUTING_MODES, &name_of(&form.mode)),
            upstream_host: self.text(&form.upstream_host, "proxy.example.com"),
            upstream_port: self.text(&form.upstream_port, "8080"),
            upstream_auth: self.auth(&form.upstream_auth),
            pac_source: self.choice(&PAC_SOURCES, &name_of(&form.pac_source)),
            pac_path: self.text(&form.pac_path, "C:\\path\\to\\proxy.pac"),
            pac_url: self.text(&form.pac_url, "http://wpad.example.com/wpad.dat"),
            pac_fail_policy: self.choice(&FAIL_POLICIES, &name_of(&form.pac_fail_policy)),
            pac_auth: self.auth(&form.pac_auth),
            corp_suffixes: self.text(&form.corp_suffixes, "corp.example.com, lab.example.com"),
            corp_poll_seconds: self.text(&form.corp_poll_seconds, "30"),
            log_level: self.choice(&LOG_LEVELS, &form.log_level),
            log_format: self.choice(&LOG_FORMATS, &form.log_format),
        }
    }
}

/// The option currently picked in `choice`, as a config-file name.
pub(super) fn picked(choice: &Choice, cx: &App) -> &'static str {
    choice.read(cx).selected_value().copied().unwrap_or("")
}

/// The enum the picked option names, or its default when nothing is picked.
fn picked_enum<T: DeserializeOwned + Default>(choice: &Choice, cx: &App) -> T {
    parse(picked(choice, cx)).unwrap_or_default()
}

fn text(input: &Text, cx: &App) -> String {
    input.read(cx).value().to_string()
}

impl AuthFields {
    fn read(&self, cx: &App) -> AuthForm {
        AuthForm {
            mode: picked_enum(&self.mode, cx),
            package: picked_enum(&self.package, cx),
            username: text(&self.username, cx),
            password: text(&self.password, cx),
        }
    }

    fn load(&self, auth: &AuthForm, window: &mut Window, cx: &mut App) {
        select(&self.mode, &AUTH_MODES, &auth.mode, window, cx);
        select(&self.package, &SSPI_PACKAGES, &auth.package, window, cx);
        set(&self.username, &auth.username, window, cx);
        set(&self.password, "", window, cx);
    }
}

impl Fields {
    /// Everything on screen, as the form model. `corp_enabled` lives on the
    /// view (a switch has no state entity).
    pub fn read(&self, corp_enabled: bool, cx: &App) -> ConfigForm {
        ConfigForm {
            listen_host: text(&self.listen_host, cx),
            listen_port: text(&self.listen_port, cx),
            mode: picked_enum(&self.mode, cx),
            upstream_host: text(&self.upstream_host, cx),
            upstream_port: text(&self.upstream_port, cx),
            upstream_auth: self.upstream_auth.read(cx),
            pac_source: picked_enum(&self.pac_source, cx),
            pac_path: text(&self.pac_path, cx),
            pac_url: text(&self.pac_url, cx),
            pac_fail_policy: picked_enum(&self.pac_fail_policy, cx),
            pac_auth: self.pac_auth.read(cx),
            corp_enabled,
            corp_suffixes: text(&self.corp_suffixes, cx),
            corp_poll_seconds: text(&self.corp_poll_seconds, cx),
            log_level: picked(&self.log_level, cx).to_owned(),
            log_format: picked(&self.log_format, cx).to_owned(),
        }
    }

    /// Put `form` on screen; password fields are always cleared.
    pub fn load(&self, form: &ConfigForm, window: &mut Window, cx: &mut App) {
        set(&self.listen_host, &form.listen_host, window, cx);
        set(&self.listen_port, &form.listen_port, window, cx);
        select(&self.mode, &ROUTING_MODES, &form.mode, window, cx);
        set(&self.upstream_host, &form.upstream_host, window, cx);
        set(&self.upstream_port, &form.upstream_port, window, cx);
        self.upstream_auth.load(&form.upstream_auth, window, cx);
        select(&self.pac_source, &PAC_SOURCES, &form.pac_source, window, cx);
        set(&self.pac_path, &form.pac_path, window, cx);
        set(&self.pac_url, &form.pac_url, window, cx);
        select(
            &self.pac_fail_policy,
            &FAIL_POLICIES,
            &form.pac_fail_policy,
            window,
            cx,
        );
        self.pac_auth.load(&form.pac_auth, window, cx);
        set(&self.corp_suffixes, &form.corp_suffixes, window, cx);
        set(&self.corp_poll_seconds, &form.corp_poll_seconds, window, cx);
        select_name(&self.log_level, &LOG_LEVELS, &form.log_level, window, cx);
        select_name(&self.log_format, &LOG_FORMATS, &form.log_format, window, cx);
    }
}

fn set(input: &Text, value: &str, window: &mut Window, cx: &mut App) {
    let value = value.to_owned();
    input.update(cx, |state, cx| state.set_value(value, window, cx));
}

fn select<T: Serialize>(
    choice: &Choice,
    options: &[&str],
    value: &T,
    window: &mut Window,
    cx: &mut App,
) {
    select_name(choice, options, &name_of(value), window, cx);
}

fn select_name(choice: &Choice, options: &[&str], name: &str, window: &mut Window, cx: &mut App) {
    let index = index_of(options, name).map(IndexPath::new);
    choice.update(cx, |state, cx| state.set_selected_index(index, window, cx));
}
