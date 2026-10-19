//! The configuration form: edit, then save to validate, persist, and apply
//! to the running proxy in one step.

mod fields;
mod render;

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use zicade_control::Control;

use self::fields::{Builder, Fields};
use crate::form_model::ConfigForm;

/// The outcome of the last save, shown under the buttons.
enum Outcome {
    Saved,
    Failed(SharedString),
}

pub(crate) struct ConfigView {
    control: Control,
    fields: Fields,
    /// A switch is stateless, so the view holds its value.
    corp_enabled: bool,
    saving: bool,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl ConfigView {
    pub(crate) fn new(control: Control, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = ConfigForm::from_config(&control.config_redacted());
        let mut builder = Builder {
            window,
            cx,
            subscriptions: Vec::new(),
        };
        let fields = builder.build(&form);
        Self {
            control,
            fields,
            corp_enabled: form.corp_enabled,
            saving: false,
            outcome: None,
            _subscriptions: builder.subscriptions,
        }
    }

    /// Throw away edits and show the config the proxy is running with.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = ConfigForm::from_config(&self.control.config_redacted());
        self.fields.load(&form, window, cx);
        self.corp_enabled = form.corp_enabled;
        self.outcome = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = match self.fields.read(self.corp_enabled, cx).to_config() {
            Ok(config) => config,
            Err(err) => {
                self.outcome = Some(Outcome::Failed(err.to_string().into()));
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.outcome = None;
        cx.notify();
        let control = self.control.clone();
        // Applying writes the file and may rebuild PAC routing; keep it off
        // the UI thread.
        let applying = cx.background_spawn(async move { control.apply(config) });
        cx.spawn_in(window, async move |this, cx| {
            let result = applying.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.reload(window, cx);
                        this.outcome = Some(Outcome::Saved);
                        window.push_notification(
                            Notification::success("Saved and applied to the running proxy."),
                            cx,
                        );
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "configuration was not applied");
                        this.outcome = Some(Outcome::Failed(err.to_string().into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
