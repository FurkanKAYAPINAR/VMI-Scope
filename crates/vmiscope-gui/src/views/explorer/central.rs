//! The Explorer's central panel: the WQL editor, the results table, and the
//! script generator that mirrors the current query.

use eframe::egui;

use crate::app::{CentralView, ScriptLang, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::DIVIDER;
use crate::util::{generate_script, save_file};
use crate::widgets::button::{btn_primary, btn_secondary, focus_ring, segmented};
use crate::widgets::codeview::{code_panel, Lang};
use crate::widgets::loading::spinner;
use crate::widgets::rule::{hrule, vrule};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

/// Starting width of a result column. Every column of a WQL result is the same
/// unknown shape, so they all start equal and the user drags from there.
const COL_W: f32 = 150.0;
/// Never shrink a result column below this -- past it the header text is gone
/// and the column stops being identifiable.
const COL_MIN: f32 = 48.0;
/// Height of the generated-script panel. The generator is a reference, not the
/// thing being worked on, so it gets a fixed slice rather than the pane.
const SCRIPT_H: f32 = 150.0;

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
                icons::labelled(ui, icons::FILE_TEXT, "Instances"),
            );
            ui.selectable_value(
                &mut self.central_view,
                CentralView::Schema,
                icons::labelled(ui, icons::CUBE, "Schema"),
            );
            if let Some(c) = self.selected_class.clone() {
                vrule(ui, DIVIDER);
                ui.weak(&c);
                if btn_secondary(ui, icons::labelled(ui, icons::FILE_TEXT, "MOF"))
                    .on_hover_text("Show MOF text")
                    .clicked()
                {
                    mof_click = Some(c.clone());
                }
                let was_open = self.actions_open;
                if ui
                    .selectable_label(
                        self.actions_open,
                        icons::labelled(ui, icons::GEAR_SIX, "Actions"),
                    )
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
        hrule(ui);

        if self.central_view == CentralView::Schema {
            self.ui_schema(ui);
            return;
        }

        // Query bar.
        ui.horizontal(|ui| {
            ui.strong("WQL");
            ui.weak(&self.active_ns);
            if self.query_loading {
                spinner(ui, "querying");
            }
        });
        let run_shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        // The WQL editor stays a hand-rolled `TextEdit`: it is the one multiline
        // input in the app, and `widgets::field` only covers the single-line
        // shapes. The focus ring has to be called explicitly for the same
        // reason -- nothing in egui paints one, and the kit only does it for the
        // controls it owns.
        let editor = ui.add(
            egui::TextEdit::multiline(&mut self.query_text)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .code_editor()
                .hint_text("SELECT * FROM Win32_Process"),
        );
        focus_ring(ui, &editor);
        ui.horizontal(|ui| {
            if btn_primary(ui, icons::labelled(ui, icons::PLAY, "Run  (Ctrl+Enter)")).clicked()
                || run_shortcut
            {
                self.run_query();
            }
            if let Some(result) = &self.result {
                ui.weak(format!(
                    "{} rows \u{00d7} {} cols",
                    result.rows.len(),
                    result.columns.len()
                ));
                if !result.rows.is_empty() {
                    vrule(ui, DIVIDER);
                    if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "CSV"))
                        .on_hover_text("Export results as CSV")
                        .clicked()
                    {
                        save_file("query.csv", &vmiscope_core::export::query_to_csv(result));
                    }
                    if btn_secondary(ui, icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "JSON"))
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
            // `widgets::field::combo` takes a fixed `&[(T, &str)]`; these two
            // are lists of strings that run a query when picked, so they stay
            // hand-rolled.
            egui::ComboBox::from_id_salt("query-history")
                .selected_text(icons::labelled(
                    ui,
                    icons::TIMER,
                    &format!("History ({})", history.len()),
                ))
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
                    .selected_text(icons::labelled(
                        ui,
                        icons::STAR,
                        &format!("Saved ({})", saved.len()),
                    ))
                    .show_ui(ui, |ui| {
                        for sq in &saved {
                            if ui.selectable_label(false, &sq.name).clicked() {
                                self.query_text = sq.wql.clone();
                                self.run_query();
                            }
                        }
                    });
            }
            if btn_secondary(ui, icons::labelled(ui, icons::STAR, "Save\u{2026}")).clicked() {
                self.save_query_name.clear();
                self.save_query_open = true;
            }
        });

        self.ui_script_gen(ui);
        hrule(ui);

        // Results table. The selection and the sort live on the app so they
        // survive a tab switch; the table gets them on loan for the frame.
        let mut table = DataTableState {
            sort: self.result_sort,
            selected: self.selected_row,
        };
        if let Some(result) = self.result.as_ref() {
            if result.columns.is_empty() {
                ui.weak("Query returned no columns.");
            } else {
                let rows = &result.rows;
                let ncols = result.columns.len();
                DataTable::new("results-table")
                    .columns(
                        result
                            .columns
                            .iter()
                            .map(|c| TableColumn::initial(c.as_str(), COL_W).at_least(COL_MIN)),
                    )
                    .selectable(true)
                    .sort_key(|row, col| rows[row].get(col).cloned().unwrap_or_default())
                    .show(ui, &mut table, rows.len(), |row| {
                        let cells = &rows[row.data_index()];
                        // Driven by the column count, not by the row's own
                        // length: a short row leaves blanks rather than
                        // shifting every cell after it one column left.
                        for col in 0..ncols {
                            row.text(cells.get(col).map(String::as_str).unwrap_or(""));
                        }
                    });
            }
        } else {
            ui.weak("Run a query to see results.");
        }
        self.result_sort = table.sort;
        self.selected_row = table.selected;
    }

    /// Collapsible PowerShell / VBScript generator for the current query.
    pub(crate) fn ui_script_gen(&mut self, ui: &mut egui::Ui) {
        let script_header =
            icons::labelled(ui, icons::CODE, "Generate script (PowerShell / VBScript)");
        ui.collapsing(script_header, |ui| {
            segmented(
                ui,
                &mut self.script_lang,
                &[
                    (ScriptLang::PowerShell, "PowerShell"),
                    (ScriptLang::VbScript, "VBScript"),
                ],
            );
            let script = generate_script(self.script_lang, &self.active_ns, &self.query_text);
            ui.horizontal(|ui| {
                if btn_secondary(ui, icons::labelled(ui, icons::COPY, "Copy")).clicked() {
                    ui.ctx().copy_text(script.clone());
                }
                ui.weak("PowerShell: paste & run \u{00b7} VBScript: cscript file.vbs");
            });
            let lang = match self.script_lang {
                ScriptLang::PowerShell => Lang::PowerShell,
                ScriptLang::VbScript => Lang::VbScript,
            };
            // `code_panel` scrolls itself and grows to its content, so the cap
            // goes on the space it is handed rather than on a second, nested
            // `ScrollArea`.
            ui.scope(|ui| {
                ui.set_max_height(SCRIPT_H);
                code_panel(ui, &script, lang);
            });
        });
    }
}
