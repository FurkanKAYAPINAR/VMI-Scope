//! The close gate.
//!
//! Closing the window while a method invocation is in flight is the one exit
//! from this application that can leave something half-done on the far side.
//! Every other pending operation is a read: abandoning a class enumeration
//! costs the enumeration. `InvokeMethod` is a write -- `Win32_Process::Create`,
//! `Win32_Service::StopService`, `Win32_OperatingSystem::Win32Shutdown` -- and
//! it has already been dispatched to the worker by the time the window can be
//! closed, so quitting neither cancels it nor tells you what it did.
//!
//! So the gate does not pretend to cancel anything. It says what is running,
//! and offers to wait for the answer or to leave without it. Those are the two
//! true options.
//!
//! # How a close is refused
//!
//! eframe 0.35 has no `on_close_event` hook. A close arrives as
//! `ViewportInfo::close_requested()` on the input for one frame, and the *only*
//! way to refuse it is to answer `ViewportCommand::CancelClose` during that
//! same frame -- so this has to run every frame, before anything that might
//! return early, and it has to consult the flag rather than a modal that has
//! not been drawn yet.

use eframe::egui::{self, Align, Layout, RichText, TextStyle, ViewportCommand};

use crate::app::VmiScopeApp;
use crate::overlays::btn_danger;
use crate::theme::icons;
use crate::theme::tokens::{muted, S2, S4, WARN};
use crate::widgets::button::btn_secondary;
use crate::widgets::rule::hrule;

impl VmiScopeApp {
    /// Answer this frame's close request, if there is one.
    ///
    /// Call once per frame from `eframe::App::ui`, before the shell.
    pub(crate) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // Already asked and answered: let it through. Without this the second
        // close -- the one the confirm dialog itself sends -- would be refused
        // by the same condition that refused the first, and the window would be
        // unclosable for as long as the invocation ran.
        if self.closing_confirmed || !self.act_invoking {
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        self.closing_open = true;
    }

    /// The confirmation, drawn above everything else.
    pub(crate) fn ui_closing_modal(&mut self, ctx: &egui::Context) {
        if !self.closing_open {
            return;
        }
        // The invocation landed while the dialog was up: there is nothing left
        // to warn about, so the dialog withdraws rather than asking the user to
        // dismiss a question that has answered itself.
        if !self.act_invoking {
            self.closing_open = false;
            return;
        }

        let method = self
            .act_method
            .clone()
            .unwrap_or_else(|| "a method".to_string());
        let elapsed = self
            .act_invoke_started
            .map(|start| ctx.input(|i| i.time) - start);

        let mut decision = None;
        egui::Modal::new(egui::Id::new("vs_closing")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.label(icons::labelled_styled(
                ui,
                icons::WARNING,
                "A method invocation is still running",
                TextStyle::Body,
                WARN,
            ));
            hrule(ui);
            ui.add_space(S2);

            ui.label(
                RichText::new(match elapsed {
                    Some(secs) => format!("{method} \u{00b7} {secs:.1}s so far"),
                    None => method.clone(),
                })
                .text_style(TextStyle::Monospace),
            );
            ui.add_space(S2);
            ui.label(RichText::new(EXPLANATION).color(muted(60)));
            ui.add_space(S4);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Leaving is the destructive option, so it wears the one filled
                // button in the app -- the same shape the invoke gate uses for
                // the call this dialog is about.
                if btn_danger(ui, icons::labelled(ui, icons::X, "Close anyway")).clicked() {
                    decision = Some(true);
                }
                if btn_secondary(ui, icons::labelled(ui, icons::TIMER, "Wait")).clicked() {
                    decision = Some(false);
                }
            });
        });

        match decision {
            Some(true) => {
                self.closing_confirmed = true;
                self.closing_open = false;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Some(false) => self.closing_open = false,
            None => {}
        }
    }
}

/// Fixed width: a modal that grows with the window puts three short lines
/// across 1900px.
const WIDTH: f32 = 420.0;

/// Said plainly, because the honest answer is not "are you sure".
const EXPLANATION: &str = "The call has already been sent to WMI. Closing now does not cancel \
                           it -- the method still runs on the target, and its result, including \
                           any error, is lost with this window. The audit line for it was written \
                           when it was sent.";

#[cfg(test)]
mod tests {
    /// The audit trail is what makes "the result is lost" survivable, and the
    /// explanation says so. Pinned because it is a factual claim about another
    /// module: `state::requests::request_invoke` appends the audit line at
    /// dispatch, *before* the reply, so it exists whether or not this window
    /// lives long enough to see the outcome.
    #[test]
    fn the_explanation_matches_when_the_audit_line_is_written() {
        assert!(super::EXPLANATION.contains("audit"));
        let src = include_str!("../state/requests.rs");
        let audit = src.find("append_audit").expect("request_invoke audits");
        let send = src
            .find("Request::InvokeMethod")
            .expect("and then dispatches");
        assert!(
            audit < send,
            "the audit line is no longer written before the call is sent, so the \
             close gate's explanation is now wrong"
        );
    }
}
