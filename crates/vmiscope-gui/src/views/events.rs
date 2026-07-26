//! The Events tab: the live WMI notification-query monitor.

use eframe::egui;
use egui::Color32;

use crate::app::VmiScopeApp;
use crate::util::save_file;

use vmiscope_core::EventMonitor;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: live event monitor
    // ------------------------------------------------------------------

    pub(crate) fn ui_events(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Live WMI events");
            if self.monitor.is_some() {
                if ui.button("\u{23f9} Stop").clicked() {
                    self.monitor = None;
                    // The error belongs to the subscription being torn down;
                    // leaving it up outlives the thing it describes.
                    self.monitor_error = None;
                }
                ui.colored_label(Color32::from_rgb(120, 210, 140), "\u{25cf} monitoring");
            } else if ui.button("\u{25b6} Start").clicked() {
                self.monitor_error = None;
                self.monitor = Some(EventMonitor::start(
                    self.active_ns.clone(),
                    self.monitor_wql.clone(),
                ));
            }
            if ui.button("clear").clicked() {
                self.events_log.clear();
            }
            if !self.events_log.is_empty()
                && ui
                    .button("\u{2b73} JSON")
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
        ui.add(
            egui::TextEdit::singleline(&mut self.monitor_wql)
                .desired_width(f32::INFINITY)
                .code_editor(),
        );
        if let Some(e) = &self.monitor_error {
            ui.colored_label(Color32::from_rgb(240, 120, 120), e);
        }
        ui.weak("A WMI notification query (WITHIN n). Default watches process creation.");
        ui.separator();

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
