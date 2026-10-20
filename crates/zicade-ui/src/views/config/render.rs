//! How the settings are laid out: one titled section per concern, labels
//! beside their fields with help underneath, and only the sections the
//! selected routing mode uses.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::fields::{AuthFields, Choice, Text};
use super::{ConfigView, Outcome};

const SAVED: &str = "Saved and applied.";

impl Render for ConfigView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.fields.mode.name(cx);
        let mode = mode.as_str();
        let gated = mode == "upstream" || mode == "pac";
        let sections = v_flex()
            .px_6()
            .pb_6()
            .gap_8()
            .max_w(rems(44.))
            .child(self.listen())
            .child(self.routing(mode))
            .when(mode == "upstream", |this| this.child(self.upstream(cx)))
            .when(mode == "pac", |this| this.child(self.pac(cx)))
            .when(gated, |this| this.child(self.corp_network(cx)))
            .child(self.logging());
        v_flex()
            .size_full()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(sections),
            )
            .child(self.actions(cx))
    }
}

impl ConfigView {
    fn listen(&self) -> impl IntoElement {
        let f = &self.fields;
        section(
            "Proxy address",
            "Where browsers and apps reach Zicade. Takes effect after a restart.",
        )
        .child(
            form()
                .child(field("Host").child(text("listen-host", "Host", &f.listen_host)))
                .child(field("Port").child(short("listen-port", "Port", &f.listen_port))),
        )
    }

    fn routing(&self, mode: &str) -> impl IntoElement {
        let help = match mode {
            "upstream" => "Every request goes through one upstream proxy.",
            "pac" => "A PAC script picks the route for each request.",
            _ => "Requests go straight to the sites they ask for.",
        };
        section("Routing", "How requests leave this machine.").child(
            form().child(
                field("Mode")
                    .description(help)
                    .child(choice("routing-mode", &self.fields.mode)),
            ),
        )
    }

    fn upstream(&self, cx: &App) -> impl IntoElement {
        let f = &self.fields;
        section("Upstream proxy", "The proxy every request is sent through.").child(
            form()
                .child(field("Host").child(text("upstream-host", "Host", &f.upstream_host)))
                .child(field("Port").child(short("upstream-port", "Port", &f.upstream_port)))
                .children(auth_fields("upstream", &f.upstream_auth, cx)),
        )
    }

    fn pac(&self, cx: &App) -> impl IntoElement {
        let f = &self.fields;
        let source = f.pac_source.name(cx);
        let source = source.as_str();
        let source_help = match source {
            "file" => "Read the script from a file on this machine.",
            "url" => "Download the script from a web address.",
            _ => "Find the script the way Windows does (WPAD and system settings).",
        };
        section(
            "PAC script",
            "Authentication applies to the proxies the script picks.",
        )
        .child(
            form()
                .child(
                    field("Source")
                        .description(source_help)
                        .child(choice("pac-source", &f.pac_source)),
                )
                .child(field("File").visible(source == "file").child(text(
                    "pac-path",
                    "File",
                    &f.pac_path,
                )))
                .child(
                    field("Address")
                        .visible(source == "url")
                        .child(text("pac-url", "Address", &f.pac_url)),
                )
                .child(
                    field("If the script fails")
                        .child(choice("pac-fail-policy", &f.pac_fail_policy)),
                )
                .children(auth_fields("pac", &f.pac_auth, cx)),
        )
    }

    fn corp_network(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.fields;
        let switch = Switch::new("corp-enabled")
            .checked(self.corp_enabled)
            .label("Only on the corporate network")
            .on_click(cx.listener(|this, on: &bool, _, cx| {
                this.corp_enabled = *on;
                cx.notify();
            }));
        section(
            "Corporate network",
            "Off the corporate network, requests go directly instead.",
        )
        .child(
            form()
                .child(Field::new().label_indent(true).child(switch))
                .child(
                    field("DNS suffixes")
                        .visible(self.corp_enabled)
                        .description("Separate several with commas.")
                        .child(text("corp-suffixes", "DNS suffixes", &f.corp_suffixes)),
                )
                .child(
                    field("Check every")
                        .visible(self.corp_enabled)
                        .description("Seconds between checks, besides network-change events.")
                        .child(short(
                            "corp-poll-seconds",
                            "Check every",
                            &f.corp_poll_seconds,
                        )),
                ),
        )
    }

    fn logging(&self) -> impl IntoElement {
        let f = &self.fields;
        section("Logging", "Applies to the Logs page and the console.").child(
            form()
                .child(field("Level").child(choice("log-level", &f.log_level)))
                .child(field("Format").child(choice("log-format", &f.log_format))),
        )
    }

    fn actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let changed = self.has_changes(cx);
        let theme = cx.theme();
        let (status, color) = match (&self.outcome, self.saving, changed) {
            (_, true, _) => (SharedString::from("Saving…"), theme.muted_foreground),
            (Some(Outcome::Failed(reason)), _, _) => (reason.clone(), theme.danger),
            (_, _, true) => ("Unsaved changes".into(), theme.muted_foreground),
            (Some(Outcome::Saved), _, false) => (SAVED.into(), theme.success),
            (None, _, false) => (SharedString::default(), theme.muted_foreground),
        };
        h_flex()
            .w_full()
            .px_6()
            .py_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("save-outcome")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(color)
                    .child(status),
            )
            .child(
                Button::new("reload")
                    .outline()
                    .label("Discard changes")
                    .disabled(self.saving || !changed)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.reload(window, cx)),
                    ),
            )
            .child(
                Button::new("save")
                    .primary()
                    .label("Save")
                    .tooltip("Save (Ctrl+S)")
                    .loading(self.saving)
                    .disabled(self.saving || !changed)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.save(window, cx)),
                    ),
            )
    }
}

/// The username/password or SSPI-package fields the picked auth mode needs.
/// `slot` keeps the upstream and PAC copies of these fields apart.
fn auth_fields(slot: &'static str, auth: &AuthFields, cx: &App) -> Vec<Field> {
    let id = |name: &str| SharedString::from(format!("{slot}-{name}"));
    let mode = auth.mode.name(cx);
    let mode = mode.as_str();
    vec![
        field("Authentication").child(choice(id("auth"), &auth.mode)),
        field("SSPI package")
            .visible(mode == "negotiate")
            .description("Uses the signed-in Windows account; no password is stored.")
            .child(choice(id("package"), &auth.package)),
        field("Username").visible(mode == "basic").child(text(
            id("username"),
            "Username",
            &auth.username,
        )),
        field("Password")
            .visible(mode == "basic")
            .description("Saved passwords are never shown. Leave blank to keep it.")
            .child(
                Input::new(&auth.password)
                    .id(id("password"))
                    .aria_label("Password")
                    .mask_toggle(),
            ),
    ]
}

fn section(title: &'static str, description: &'static str) -> Div {
    v_flex().gap_3().child(
        v_flex()
            .gap_0p5()
            .child(div().text_base().font_semibold().child(title))
            .child(div().text_sm().opacity(0.7).child(description)),
    )
}

fn form() -> Form {
    Form::new().label_layout(Axis::Horizontal)
}

fn field(label: &'static str) -> Field {
    Field::new().label(label)
}

fn text(id: impl Into<ElementId>, label: &'static str, input: &Text) -> Input {
    Input::new(input).id(id).aria_label(label)
}

/// A narrow text field, for ports and seconds.
fn short(id: impl Into<ElementId>, label: &'static str, input: &Text) -> Input {
    Input::new(input).id(id).aria_label(label).w_32()
}

fn choice(id: impl Into<ElementId>, select: &Choice) -> Select<Vec<&'static str>> {
    Select::new(&select.state).id(id).w_56()
}
