//! The Explorer's central panel: the WQL editor, the results table, and the
//! script generator that mirrors the current query.

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::app::{CentralView, ScriptLang, VmiScopeApp};
use crate::util::{generate_script, save_file, smart_cmp, toggle_sort};
use crate::widgets::table::sortable_header;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: query editor + results table
    // ------------------------------------------------------------------

    pub(crate) fn ui_central(&mut self, ui: &mut egui::Ui) {
        // View switch for the selected class.
        let prev_view = self.central_view;
        let mut mof_click: Option<String> = None;
        let mut open_actions_for: Option<String> = None;
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.central_view,
                CentralView::Instances,
                "\u{1f4c4} Instances",
            );
            ui.selectable_value(
                &mut self.central_view,
                CentralView::Schema,
                "\u{1f9ec} Schema",
            );
            if let Some(c) = self.selected_class.clone() {
                ui.separator();
                ui.weak(&c);
                if ui
                    .button("\u{1f4c4} MOF")
                    .on_hover_text("Show MOF text")
                    .clicked()
                {
                    mof_click = Some(c.clone());
                }
                let was_open = self.actions_open;
                if ui
                    .selectable_label(self.actions_open, "\u{2699} Actions")
                    .on_hover_text("Invoke methods (mutating)")
                    .clicked()
                {
                    self.actions_open = !self.actions_open;
                }
                if self.actions_open && !was_open {
                    open_actions_for = Some(c);
                }
            }
        });
        if let Some(c) = mof_click {
            self.request_mof(c.clone(), c);
        }
        if let Some(c) = open_actions_for {
            self.act_method = None;
            self.act_outcome = None;
            self.act_instances = None;
            self.request_schema(c);
        }
        // Fetch schema the moment the user flips to the Schema view.
        if self.central_view == CentralView::Schema && prev_view != CentralView::Schema {
            if let Some(c) = self.selected_class.clone() {
                self.request_schema(c);
            }
        }
        ui.separator();

        if self.central_view == CentralView::Schema {
            self.ui_schema(ui);
            return;
        }

        // Query bar.
        ui.horizontal(|ui| {
            ui.strong("WQL");
            ui.weak(&self.active_ns);
            if self.query_loading {
                ui.spinner();
            }
        });
        let run_shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        ui.add(
            egui::TextEdit::multiline(&mut self.query_text)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .code_editor()
                .hint_text("SELECT * FROM Win32_Process"),
        );
        ui.horizontal(|ui| {
            if ui.button("\u{25b6} Run  (Ctrl+Enter)").clicked() || run_shortcut {
                self.run_query();
            }
            if let Some(result) = &self.result {
                ui.weak(format!(
                    "{} rows \u{00d7} {} cols",
                    result.rows.len(),
                    result.columns.len()
                ));
                if !result.rows.is_empty() {
                    ui.separator();
                    if ui
                        .button("\u{2b73} CSV")
                        .on_hover_text("Export results as CSV")
                        .clicked()
                    {
                        save_file("query.csv", &vmiscope_core::export::query_to_csv(result));
                    }
                    if ui
                        .button("\u{2b73} JSON")
                        .on_hover_text("Export results as JSON")
                        .clicked()
                    {
                        save_file("query.json", &vmiscope_core::export::query_to_json(result));
                    }
                }
            }
        });

        // Query history + saved queries.
        ui.horizontal(|ui| {
            let history = self.config.history.clone();
            egui::ComboBox::from_id_salt("query-history")
                .selected_text(format!("\u{23f1} History ({})", history.len()))
                .show_ui(ui, |ui| {
                    for q in &history {
                        let short: String = q.chars().take(80).collect();
                        if ui.selectable_label(false, short).clicked() {
                            self.query_text = q.clone();
                            self.run_query();
                        }
                    }
                });
            let saved = self.config.saved.clone();
            if !saved.is_empty() {
                egui::ComboBox::from_id_salt("saved-queries")
                    .selected_text(format!("\u{2605} Saved ({})", saved.len()))
                    .show_ui(ui, |ui| {
                        for sq in &saved {
                            if ui.selectable_label(false, &sq.name).clicked() {
                                self.query_text = sq.wql.clone();
                                self.run_query();
                            }
                        }
                    });
            }
            if ui.button("\u{2605} Save\u{2026}").clicked() {
                self.save_query_name.clear();
                self.save_query_open = true;
            }
        });

        self.ui_script_gen(ui);
        ui.separator();

        // Results table (virtualized, sortable by clicking a header).
        let selected_row = self.selected_row;
        let sort = self.result_sort;
        let mut newly_clicked: Option<usize> = None;
        let mut header_clicked: Option<usize> = None;
        if let Some(result) = self.result.as_ref() {
            if result.columns.is_empty() {
                ui.weak("Query returned no columns.");
            } else {
                // Row display order for the active sort column.
                let mut order: Vec<usize> = (0..result.rows.len()).collect();
                if let Some((ci, asc)) = sort {
                    order.sort_by(|&a, &b| {
                        let av = result.rows[a].get(ci).map(String::as_str).unwrap_or("");
                        let bv = result.rows[b].get(ci).map(String::as_str).unwrap_or("");
                        let o = smart_cmp(av, bv);
                        if asc {
                            o
                        } else {
                            o.reverse()
                        }
                    });
                }

                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                let ncols = result.columns.len();
                let mut table = TableBuilder::new(ui)
                    .id_salt("results-table")
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0);
                for _ in 0..ncols {
                    table = table.column(
                        Column::initial(150.0)
                            .at_least(48.0)
                            .clip(true)
                            .resizable(true),
                    );
                }
                table
                    .header(22.0, |mut header| {
                        for (ci, col) in result.columns.iter().enumerate() {
                            header.col(|ui| {
                                if sortable_header(ui, col, ci, sort) {
                                    header_clicked = Some(ci);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, order.len(), |mut row| {
                            let actual = order[row.index()];
                            row.set_selected(selected_row == Some(actual));
                            for cell in &result.rows[actual] {
                                row.col(|ui| {
                                    ui.label(cell);
                                });
                            }
                            if row.response().clicked() {
                                newly_clicked = Some(actual);
                            }
                        });
                    });
            }
        } else {
            ui.weak("Run a query to see results.");
        }
        if let Some(ci) = header_clicked {
            toggle_sort(&mut self.result_sort, ci);
        }
        if let Some(ri) = newly_clicked {
            self.selected_row = Some(ri);
        }
    }

    /// Collapsible PowerShell / VBScript generator for the current query.
    pub(crate) fn ui_script_gen(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("\u{1f4dc} Generate script (PowerShell / VBScript)", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.script_lang, ScriptLang::PowerShell, "PowerShell");
                ui.selectable_value(&mut self.script_lang, ScriptLang::VbScript, "VBScript");
            });
            let script = generate_script(self.script_lang, &self.active_ns, &self.query_text);
            ui.horizontal(|ui| {
                if ui.button("\u{1f4cb} Copy").clicked() {
                    ui.ctx().copy_text(script.clone());
                }
                ui.weak("PowerShell: paste & run \u{00b7} VBScript: cscript file.vbs");
            });
            egui::ScrollArea::vertical()
                .id_salt("script-box")
                .max_height(150.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(script.as_str()).monospace())
                            .selectable(true),
                    );
                });
        });
    }
}
