//! The Events tab: the live WMI notification-query monitor.

use eframe::egui;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{BAD, OK};
use crate::util::save_file;
use crate::widgets::button::{btn_ghost, btn_primary, btn_secondary};
use crate::widgets::chip::dot_chip;
use crate::widgets::field::mono_input;
use crate::widgets::rule::hrule;

use vmiscope_core::EventMonitor;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: live event monitor
    // ------------------------------------------------------------------

    pub(crate) fn ui_events(&mut self, ui: &mut egui::Ui) {
        // The kit's inputs take their width from `spacing.text_edit_width`,
        // which egui defaults to 280. A notification query does not fit in 280,
        // and this one used to be explicitly full-bleed, so the default is
        // widened for the whole view rather than restated at the field.
        ui.spacing_mut().text_edit_width = ui.available_width();
        ui.horizontal(|ui| {
            ui.strong("Live WMI events");
            if self.monitor.is_some() {
                // Stop is the primary here for the same reason Pause is on the
                // Network tab: once the monitor is running, ending it is the
                // only decision left to make.
                if btn_primary(ui, icons::labelled(ui, icons::STOP, "Stop")).clicked() {
                    self.monitor = None;
                    // The error belongs to the subscription being torn down;
                    // leaving it up outlives the thing it describes.
                    self.monitor_error = None;
                }
                dot_chip(ui, OK, "monitoring");
            } else if btn_primary(ui, icons::labelled(ui, icons::PLAY, "Start")).clicked() {
                self.monitor_error = None;
                self.monitor = Some(EventMonitor::start(
                    self.active_ns.clone(),
                    self.monitor_wql.clone(),
                ));
            }
            if btn_ghost(ui, icons::labelled(ui, icons::X, "clear")).clicked() {
                self.events_log.clear();
            }
            if !self.events_log.is_empty()
                && btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "JSON"))
                    .on_hover_text("Export events")
                    .clicked()
            {
                save_file(
                    "wmi_events.json",
                    &vmiscope_core::export::events_to_json(&self.events_log),
                );
            }
            ui.weak(format!("{} events", self.events_log.len()));
        });
        // No hint: the field ships with a default query in it and is never
        // empty, so a hint would only ever be dead weight in the layout.
        mono_input(ui, &mut self.monitor_wql, "");
        if let Some(e) = &self.monitor_error {
            ui.colored_label(BAD, e);
        }
        ui.weak("A WMI notification query (WITHIN n). Default watches process creation.");
        hrule(ui);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.events_log.is_empty() {
                    ui.weak("No events yet \u{2014} click Start.");
                }
                for ev in &self.events_log {
                    let find = |suffix: &str| {
                        ev.iter()
                            .find(|(k, _)| k.ends_with(suffix))
                            .map(|(_, v)| v.as_str())
                            .unwrap_or("")
                    };
                    let name = find(".Name");
                    let summary = if name.is_empty() {
                        ev.iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join("   ")
                    } else {
                        format!(
                            "{name}   pid {}   {}",
                            find(".ProcessId"),
                            find(".CommandLine")
                        )
                    };
                    let all = ev
                        .iter()
                        .map(|(k, v)| format!("{k} = {v}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.label(summary).on_hover_text(all);
                }
            });
    }
}
