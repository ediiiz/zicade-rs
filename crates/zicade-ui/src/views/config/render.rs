//! How the form is laid out: one group per config section, showing only the
//! sections the selected routing mode uses.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::group_box::GroupBox;
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::fields::{AuthFields, Choice, Text, picked};
use super::{ConfigView, Outcome};

const LABEL_WIDTH: f32 = 150.;
const SAVED: &str = "Saved and applied.";

impl Render for ConfigView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = picked(&self.fields.mode, cx);
        let gated = mode == "upstream" || mode == "pac";
        let form = v_flex()
            .gap_4()
            .p_4()
            .child(self.listen())
            .child(self.routing())
            .when(mode == "upstream", |this| this.child(self.upstream(cx)))
            .when(mode == "pac", |this| this.child(self.pac(cx)))
            .when(gated, |this| this.child(self.corp_network(cx)))
            .child(self.logging());
        v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(form))
            .child(self.actions(cx))
    }
}

impl ConfigView {
    fn listen(&self) -> impl IntoElement {
        let f = &self.fields;
        section("Listen")
            .child(text_row("Host", "listen-host", &f.listen_host))
            .child(short_row("Port", "listen-port", &f.listen_port))
            .child(note("A listen change takes effect after a restart."))
    }

    fn routing(&self) -> impl IntoElement {
        section("Routing").child(choice_row("Mode", "routing-mode", &self.fields.mode))
    }

    fn upstream(&self, cx: &App) -> impl IntoElement {
        let f = &self.fields;
        section("Upstream proxy")
            .child(text_row("Host", "upstream-host", &f.upstream_host))
            .child(short_row("Port", "upstream-port", &f.upstream_port))
            .children(auth_rows("upstream", &f.upstream_auth, cx))
    }

    fn pac(&self, cx: &App) -> impl IntoElement {
        let f = &self.fields;
        let source = picked(&f.pac_source, cx);
        section("PAC")
            .child(choice_row("Source", "pac-source", &f.pac_source))
            .when(source == "file", |this| {
                this.child(text_row("File path", "pac-path", &f.pac_path))
            })
            .when(source == "url", |this| {
                this.child(text_row("URL", "pac-url", &f.pac_url))
            })
            .child(choice_row(
                "If PAC fails",
                "pac-fail-policy",
                &f.pac_fail_policy,
            ))
            .child(note("Auth for the proxies the PAC script picks:"))
            .children(auth_rows("pac", &f.pac_auth, cx))
    }

    fn corp_network(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.fields;
        let switch = Switch::new("corp-enabled")
            .checked(self.corp_enabled)
            .label("Use the routing above only on the corporate network")
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.corp_enabled = *on;
                cx.notify();
            }));
        section("Corporate network")
            .child(switch)
            .when(self.corp_enabled, |this| {
                this.child(text_row("DNS suffixes", "corp-suffixes", &f.corp_suffixes))
                    .child(short_row(
                        "Re-check every (s)",
                        "corp-poll-seconds",
                        &f.corp_poll_seconds,
                    ))
                    .child(note(
                        "Off the corporate network, requests go direct. Separate suffixes with commas.",
                    ))
            })
    }

    fn logging(&self) -> impl IntoElement {
        let f = &self.fields;
        section("Logging")
            .child(choice_row("Level", "log-level", &f.log_level))
            .child(choice_row("Format", "log-format", &f.log_format))
    }

    fn actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let outcome = match &self.outcome {
            Some(Outcome::Saved) => Some((SharedString::from(SAVED), theme.success)),
            Some(Outcome::Failed(reason)) => Some((reason.clone(), theme.danger)),
            None => None,
        };
        let outcome = outcome.map(|(text, color)| {
            div()
                .id("save-outcome")
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .text_color(color)
                .child(text)
        });
        h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new("save")
                    .primary()
                    .label("Save")
                    .loading(self.saving)
                    .disabled(self.saving)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.save(window, cx)),
                    ),
            )
            .child(
                Button::new("reload")
                    .ghost()
                    .label("Discard changes")
                    .disabled(self.saving)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.reload(window, cx)),
                    ),
            )
            .child(div().flex_1().text_sm().children(outcome))
    }
}

/// The username/password or SSPI-package rows the picked auth mode needs.
/// `slot` keeps the upstream and PAC copies of these rows apart.
fn auth_rows(slot: &'static str, auth: &AuthFields, cx: &App) -> Vec<AnyElement> {
    let id = |name: &str| SharedString::from(format!("{slot}-{name}"));
    let mut rows = vec![choice_row("Authentication", id("auth"), &auth.mode).into_any_element()];
    match picked(&auth.mode, cx) {
        "basic" => {
            rows.push(text_row("Username", id("username"), &auth.username).into_any_element());
            let password = Input::new(&auth.password)
                .id(id("password"))
                .aria_label("Password")
                .mask_toggle();
            rows.push(row("Password", password).into_any_element());
        }
        "negotiate" => {
            rows.push(choice_row("SSPI package", id("package"), &auth.package).into_any_element());
            rows.push(
                note("Uses the signed-in Windows account; no password is stored.")
                    .into_any_element(),
            );
        }
        _ => {}
    }
    rows
}

fn section(title: &'static str) -> GroupBox {
    GroupBox::new().title(div().font_weight(FontWeight::SEMIBOLD).child(title))
}

fn row(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    v_form()
        .label_width(px(LABEL_WIDTH))
        .child(field().label(label).child(input))
}

fn text_row(label: &'static str, id: impl Into<ElementId>, input: &Text) -> AnyElement {
    row(label, Input::new(input).id(id).aria_label(label)).into_any_element()
}

/// A narrow text field, for ports and seconds.
fn short_row(label: &'static str, id: impl Into<ElementId>, input: &Text) -> AnyElement {
    row(
        label,
        Input::new(input).id(id).aria_label(label).w(px(120.)),
    )
    .into_any_element()
}

fn choice_row(label: &'static str, id: impl Into<ElementId>, select: &Choice) -> AnyElement {
    row(label, Select::new(select).id(id).w(px(220.))).into_any_element()
}

fn note(text: &'static str) -> impl IntoElement {
    div().text_xs().opacity(0.7).child(text)
}
