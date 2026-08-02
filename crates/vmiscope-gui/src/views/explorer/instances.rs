//! The Instances sub-tab: a dense, sortable table of `SELECT * FROM <class>`.

use eframe::egui;
use eframe::egui::TextStyle;

use vmiscope_core::Tally;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::muted;
use crate::widgets::loading::{partial_note, spinner};
use crate::widgets::table::{numeric_threshold_color, DataTable, DataTableState, TableColumn};

/// Starting width of a result column. Every column of a `SELECT *` is an unknown
/// shape, so they all start equal and the user drags from there.
const COL_W: f32 = 150.0;
/// Never shrink a column past the point its header stops being identifiable.
const COL_MIN: f32 = 48.0;

/// Percent thresholds. Above `WARN_PCT` a percent cell reads warn, above
/// `BAD_PCT` bad -- the design's "colour a CPU cell by how alarming it is".
const WARN_PCT: f64 = 75.0;
const BAD_PCT: f64 = 90.0;

impl VmiScopeApp {
    pub(crate) fn ui_instances_tab(&mut self, ui: &mut egui::Ui) {
        // A skipped class has no instances to list -- its "instances" are the
        // wrong question (an association is a relationship, an event a message
        // shape). Say which, rather than showing an empty grid that reads as
        // "none found".
        if let Some(Tally::Skipped(reason)) = self.selected_tally() {
            let note = reason.note();
            ui.add_space(crate::theme::tokens::S3);
            ui.label(icons::labelled_styled(
                ui,
                icons::INFO,
                note,
                TextStyle::Body,
                muted(55),
            ));
            return;
        }

        if self.query_loading {
            spinner(ui, "querying instances\u{2026}");
        }

        let mut table = DataTableState {
            sort: self.result_sort,
            selected: self.selected_row,
        };
        let mut rendered = false;

        if let Some(result) = self.result.as_ref() {
            rendered = true;
            // A cut-short result says so above the table rather than passing a
            // partial population off as the whole.
            partial_note(ui, result.completion.note());

            if result.columns.is_empty() {
                ui.label(egui::RichText::new("Query returned no columns.").color(muted(50)));
            } else if result.rows.is_empty() && !self.query_loading {
                ui.label(egui::RichText::new("No instances.").color(muted(50)));
            } else if !result.rows.is_empty() {
                let ncols = result.columns.len();
                // A column is numeric only if every non-empty cell parses as a
                // number -- measured from the data, not guessed from the name,
                // so a right-aligned column is never one full of text.
                let numeric: Vec<bool> = (0..ncols)
                    .map(|c| column_is_numeric(&result.rows, c))
                    .collect();
                let percent: Vec<bool> = result
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(c, name)| numeric[c] && name.to_lowercase().contains("percent"))
                    .collect();
                let rows = &result.rows;

                DataTable::new("explorer-instances")
                    .columns(result.columns.iter().enumerate().map(|(c, name)| {
                        TableColumn::initial(name.as_str(), COL_W)
                            .at_least(COL_MIN)
                            .numeric(numeric[c])
                    }))
                    .selectable(true)
                    .sort_key(|row, col| rows[row].get(col).cloned().unwrap_or_default())
                    .show(ui, &mut table, rows.len(), |row| {
                        let cells = &rows[row.data_index()];
                        for col in 0..ncols {
                            let val = cells.get(col).map(String::as_str).unwrap_or("");
                            if percent[col] {
                                let v: f64 = val.parse().unwrap_or(0.0);
                                row.colored(val, numeric_threshold_color(v, WARN_PCT, BAD_PCT));
                            } else if numeric[col] {
                                row.text(val);
                            } else {
                                // Ellipsis-plus-tooltip for long paths and names.
                                row.path(val);
                            }
                        }
                    });
            }
        }

        if !rendered && !self.query_loading {
            ui.label(egui::RichText::new("No instances loaded.").color(muted(50)));
        }

        self.result_sort = table.sort;
        self.selected_row = table.selected;
    }
}

/// Is column `col` numeric across the rows? Samples the first chunk so a huge
/// result is not scanned in full every frame; a column with no non-empty cell in
/// the sample is treated as text (nothing to right-align).
fn column_is_numeric(rows: &[Vec<String>], col: usize) -> bool {
    let mut any = false;
    for row in rows.iter().take(128) {
        match row.get(col) {
            Some(v) if v.is_empty() => {}
            Some(v) => {
                any = true;
                if v.parse::<f64>().is_err() {
                    return false;
                }
            }
            None => {}
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(cells: &[&[&str]]) -> Vec<Vec<String>> {
        cells
            .iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    /// A column of numbers is numeric; one with any non-numeric cell is not; a
    /// column that is empty in the sample is text (there is nothing to align).
    #[test]
    fn numeric_detection_needs_every_nonempty_cell_to_parse() {
        let data = rows(&[
            &["42", "root", "", "3.5"],
            &["17", "12", "", "x"],
            &["8", "9", "", "1"],
        ]);
        assert!(column_is_numeric(&data, 0), "all numbers");
        assert!(!column_is_numeric(&data, 1), "'root' is not a number");
        assert!(!column_is_numeric(&data, 2), "all empty -> not numeric");
        assert!(!column_is_numeric(&data, 3), "'x' breaks it");
    }
}
