//! The form's widget state, and the copy to and from a [`ConfigForm`].

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::choices::{
    AUTH_MODES, FAIL_POLICIES, Item, LOG_FORMATS, LOG_LEVELS, PAC_SOURCES, ROUTING_MODES,
    SSPI_PACKAGES, index_of, labels, name_of, parse,
};
use crate::form_model::{AuthForm, ConfigForm};

pub(super) type Text = Entity<InputState>;

/// A drop-down: readable labels on screen, config-file names underneath.
pub(super) struct Choice {
    pub state: Entity<SelectState<Vec<&'static str>>>,
    options: &'static [Item],
    /// A value from the config the list does not offer (a hand edit). It is
    /// kept, not blanked, until the user picks something else.
    unlisted: Option<String>,
}

impl Choice {
    /// The picked option's config-file name.
    pub fn name(&self, cx: &App) -> String {
        let picked = self
            .state
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.options.get(index.row));
        match (picked, &self.unlisted) {
            (Some(item), _) => item.name.to_owned(),
            (None, Some(unlisted)) => unlisted.clone(),
            (None, None) => String::new(),
        }
    }

    fn pick(&mut self, name: &str, window: &mut Window, cx: &mut App) {
        let index = index_of(self.options, name);
        self.unlisted = index.is_none().then(|| name.to_owned());
        self.state.update(cx, |state, cx| {
            state.set_selected_index(index.map(IndexPath::new), window, cx);
        });
    }
}

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

/// Builds widgets on a view and re-renders it on every edit: choices decide
/// which sections show, and any edit can change whether there is anything to
/// save.
pub(super) struct Builder<'a, 'b, V: 'static> {
    pub window: &'a mut Window,
    pub cx: &'a mut Context<'b, V>,
    pub subscriptions: Vec<Subscription>,
}

impl<V: 'static> Builder<'_, '_, V> {
    fn text(&mut self, value: &str, placeholder: &str) -> Text {
        let (value, placeholder) = (value.to_owned(), placeholder.to_owned());
        let input = self.cx.new(|cx| {
            InputState::new(self.window, cx)
                .placeholder(placeholder)
                .default_value(value)
        });
        self.watch(&input);
        input
    }

    fn secret(&mut self) -> Text {
        let input = self.cx.new(|cx| {
            InputState::new(self.window, cx)
                .placeholder("Leave blank to keep the saved password")
                .masked(true)
        });
        self.watch(&input);
        input
    }

    fn watch(&mut self, input: &Text) {
        let sub = self
            .cx
            .subscribe_in(input, self.window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        self.subscriptions.push(sub);
    }

    fn choice(&mut self, options: &'static [Item], selected: &str) -> Choice {
        let index = index_of(options, selected);
        let state = self.cx.new(|cx| {
            SelectState::new(labels(options), index.map(IndexPath::new), self.window, cx)
        });
        let sub = self.cx.subscribe_in(
            &state,
            self.window,
            |_, _, _: &SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        );
        self.subscriptions.push(sub);
        Choice {
            state,
            options,
            unlisted: index.is_none().then(|| selected.to_owned()),
        }
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

/// The enum the picked option names, or its default when nothing is picked.
fn picked_enum<T: DeserializeOwned + Default>(choice: &Choice, cx: &App) -> T {
    parse(&choice.name(cx)).unwrap_or_default()
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

    fn load(&mut self, auth: &AuthForm, window: &mut Window, cx: &mut App) {
        select(&mut self.mode, &auth.mode, window, cx);
        select(&mut self.package, &auth.package, window, cx);
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
            log_level: self.log_level.name(cx),
            log_format: self.log_format.name(cx),
        }
    }

    /// Put `form` on screen; password fields are always cleared.
    pub fn load(&mut self, form: &ConfigForm, window: &mut Window, cx: &mut App) {
        set(&self.listen_host, &form.listen_host, window, cx);
        set(&self.listen_port, &form.listen_port, window, cx);
        select(&mut self.mode, &form.mode, window, cx);
        set(&self.upstream_host, &form.upstream_host, window, cx);
        set(&self.upstream_port, &form.upstream_port, window, cx);
        self.upstream_auth.load(&form.upstream_auth, window, cx);
        select(&mut self.pac_source, &form.pac_source, window, cx);
        set(&self.pac_path, &form.pac_path, window, cx);
        set(&self.pac_url, &form.pac_url, window, cx);
        select(&mut self.pac_fail_policy, &form.pac_fail_policy, window, cx);
        self.pac_auth.load(&form.pac_auth, window, cx);
        set(&self.corp_suffixes, &form.corp_suffixes, window, cx);
        set(&self.corp_poll_seconds, &form.corp_poll_seconds, window, cx);
        self.log_level.pick(&form.log_level, window, cx);
        self.log_format.pick(&form.log_format, window, cx);
    }
}

fn set(input: &Text, value: &str, window: &mut Window, cx: &mut App) {
    let value = value.to_owned();
    input.update(cx, |state, cx| state.set_value(value, window, cx));
}

fn select<T: Serialize>(choice: &mut Choice, value: &T, window: &mut Window, cx: &mut App) {
    choice.pick(&name_of(value), window, cx);
}
