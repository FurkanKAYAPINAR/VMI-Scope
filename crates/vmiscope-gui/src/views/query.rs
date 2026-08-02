//! The Query view: a WQL editor, the result grid, and the run history.
//!
//! The editor used to be three lines inside the Explorer's central pane. It is
//! its own destination now, laid out the way the design has it: a header row
//! carrying the namespace and the Run / Save / Export actions, a line-numbered
//! editor, a status strip built from the run's *measured* numbers, the result
//! table, and a 262px history rail down the right.
//!
//! Two things here are deliberately derived rather than stated:
//!
//! * The status strip's duration is `QueryResult::elapsed_ms`, which the core
//!   measures around the enumeration. It is never a constant, and the namespace
//!   bind is reported apart from it (in the strip's tooltip) because folding the
//!   two together would turn a 3 ms query on a 40 ms connection into "43 ms".
//! * The `ORDER BY` note appears only when the WQL really contains the clause
//!   outside a string literal. A permanent banner would teach people to stop
//!   reading it.
//!
//! What that note *says* is not what `docs/REDESIGN.md` planned. The plan called
//! "ORDER BY is evaluated locally by the client" a true statement about WQL.
//! **It is not true here.** Measured against this machine's WMI: every `ORDER BY`
//! form is rejected by the query parser with `WBEM_E_INVALID_QUERY`
//! (`0x80041017`, "Invalid query") before a single row is produced --
//! `Win32_Process`, `Win32_Service` and `Win32_OperatingSystem` alike, whether
//! the clause is on its own line or not, with or without `ASC`. Nothing sorts it
//! locally either: the result table sorts on a *header click*, which is a
//! different thing that the user asks for explicitly. So the note says the query
//! will be refused and points at the header, which is both true and more useful
//! than the plan's version -- that one implies the query runs.

use eframe::egui::{
    self, Align2, Frame, Key, Label, Margin, Modifiers, Pos2, RichText, Sense, TextEdit, TextStyle,
    Vec2,
};

use vmiscope_core::export::{query_to_csv, query_to_json};

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, BG, DIVIDER, OK, R_MD, S2, S3, S4, SURFACE, WARN};
use crate::util::save_file;
use crate::widgets::button::{accent, btn_icon, btn_primary, btn_secondary, focus_ring};
use crate::widgets::codeview::{tint_line, Lang, Role};
use crate::widgets::kv::kv_grid_sized;
use crate::widgets::loading::{empty_state, format_ms, spinner, SLOW_MS};
use crate::widgets::rule::{hrule, solid_hline, HAIRLINE};
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

/// History rail width. Exact, per task 4.5.
const HISTORY_W: f32 = 262.0;

/// The row-detail reveal's width. Wider than the history rail because it carries
/// WMI property names, which are long.
const DETAIL_W: f32 = 320.0;

/// Editor gutter width. Four monospace digits plus air -- a WQL query long
/// enough to need five is a query that wants a file.
const GUTTER_DIGITS: usize = 3;

/// How many rows the editor opens at. The mock's textarea is 117px at a 1.85
/// line height, which is a shade under five lines of the monospace face.
const EDITOR_ROWS: usize = 5;

/// Starting width of a result column. Every column of a `SELECT *` is an unknown
/// shape, so they all start equal and the user drags from there.
const COL_W: f32 = 160.0;
/// Never shrink a column past the point its header stops being identifiable.
const COL_MIN: f32 = 48.0;

/// A history row's query text: one line, ellipsized.
const HISTORY_TEXT_SIZE: f32 = 11.5;
/// A history row's meta line.
const HISTORY_META_SIZE: f32 = 10.5;

// ---------------------------------------------------------------------------
// Derivations
// ---------------------------------------------------------------------------

/// Does `wql` contain an `ORDER BY` clause outside a string literal?
///
/// Reuses [`tint_line`] rather than scanning for the substring, because
/// `WHERE Name = 'order by'` is not an `ORDER BY` clause and a `contains`
/// would say it was. The lexer already knows where a literal starts and ends
/// (including doubled-quote escapes), so the only thing left to do here is read
/// its output.
///
/// The words are flattened across lines before the scan, so a clause broken over
/// two lines still counts. An unterminated literal ends at its line, which is the
/// lexer's rule and the right one: WQL has no multi-line string.
pub(crate) fn has_order_by(wql: &str) -> bool {
    let words: Vec<String> = wql
        .lines()
        .flat_map(|line| tint_line(line, Lang::Wql))
        .filter(|span| !matches!(span.role, Role::Str | Role::Comment))
        .map(|span| span.text)
        .filter(|text| !text.trim().is_empty())
        .collect();

    words
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case("order") && pair[1].eq_ignore_ascii_case("by"))
}

/// Which logical line number belongs beside each visual row of a wrapped galley.
///
/// `ends_with_newline[i]` is [`epaint::text::PlacedRow::ends_with_newline`] for
/// visual row `i`. A row that continues the previous one gets `None` -- the
/// gutter has to number *lines*, not rows, or a wrapped query is silently
/// renumbered every time the pane is resized.
fn gutter_labels(ends_with_newline: &[bool]) -> Vec<Option<usize>> {
    let mut labels = Vec::with_capacity(ends_with_newline.len());
    let mut line = 0usize;
    for row in 0..ends_with_newline.len() {
        let starts_line = row == 0 || ends_with_newline[row - 1];
        if starts_line {
            line += 1;
            labels.push(Some(line));
        } else {
            labels.push(None);
        }
    }
    labels
}

/// "1 row" / "N rows".
///
/// Shared with the Saved view, which prints the same figure on its cards. It is
/// a one-word difference and it is the difference between a tool that reads as
/// written and one that reads as generated.
pub(crate) fn rows_label(rows: usize) -> String {
    if rows == 1 {
        "1 row".to_string()
    } else {
        format!("{rows} rows")
    }
}

/// A coarse "how long ago", for the history rail.
///
/// Coarse on purpose: the rail is 262px wide and the question it answers is
/// "was this this morning or last week", never "was this 94 seconds ago".
fn relative_time(then: u64, now: u64) -> String {
    if then == 0 || then > now {
        // A timestamp from the future is a clock change, not a time; saying
        // "just now" would be a guess.
        return "\u{2014}".to_string();
    }
    let secs = now - then;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
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

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    pub(crate) fn ui_query(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("vs_history")
            .exact_size(HISTORY_W)
            // `Panel::right` is constructed resizable; a fixed rail needs both of
            // these, and the parent still draws the separator, so the rail paints
            // its own edge.
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| self.ui_query_history(ui));

        // The row-detail reveal: a second right panel, present only while a row
        // is selected. It replaces the Explorer-era `detail` pane -- a panel that
        // was always there and empty most of the time.
        if self.selected_row.is_some() {
            egui::Panel::right("vs_row_detail")
                .exact_size(DETAIL_W)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::NONE.fill(BG))
                .show(ui, |ui| self.ui_row_detail(ui));
        }

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| {
                self.ui_query_editor(ui);
                self.ui_query_status(ui);
                self.ui_query_results(ui);
            });
    }

    // -- editor ------------------------------------------------------------

    fn ui_query_editor(&mut self, ui: &mut egui::Ui) {
        // Consumed before the field is added: a focused multiline `TextEdit`
        // claims Enter as its own, so a shortcut read after it would never fire.
        let run_shortcut = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));

        let mut run = run_shortcut;
        let mut save = false;
        let mut export_csv = false;
        let mut export_json = false;
        let has_rows = self.result.as_ref().is_some_and(|r| !r.rows.is_empty());

        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S3 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(icons::labelled_styled(
                        ui,
                        icons::TERMINAL_WINDOW,
                        "WQL query",
                        TextStyle::Body,
                        accent(ui),
                    ));
                    ui.label(
                        RichText::new(&self.active_ns)
                            .text_style(TextStyle::Monospace)
                            .size(11.0)
                            .color(muted(45)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Right-to-left, so the buttons read Run · Save · Export
                        // left to right once laid out.
                        ui.menu_button(icons::labelled(ui, icons::EXPORT, "Export"), |ui| {
                            if has_rows {
                                if ui
                                    .button(icons::labelled(ui, icons::FILE_CSV, "Results as CSV"))
                                    .clicked()
                                {
                                    export_csv = true;
                                    ui.close();
                                }
                                if ui
                                    .button(icons::labelled(
                                        ui,
                                        icons::BRACKETS_CURLY,
                                        "Results as JSON",
                                    ))
                                    .clicked()
                                {
                                    export_json = true;
                                    ui.close();
                                }
                            } else {
                                ui.label(RichText::new("Run a query first").color(muted(40)));
                            }
                        });
                        if btn_secondary(ui, icons::labelled(ui, icons::BOOKMARK_SIMPLE, "Save"))
                            .on_hover_text("Save this query to the library")
                            .clicked()
                        {
                            save = true;
                        }
                        let run_label = if self.query_loading { "Running" } else { "Run" };
                        if btn_primary(ui, icons::labelled(ui, icons::PLAY, run_label))
                            .on_hover_text("Run the query (Ctrl+Enter)")
                            .clicked()
                        {
                            run = true;
                        }
                    });
                });
            });

        editor_edge(ui);
        self.ui_wql_field(ui);
        editor_edge(ui);

        if save {
            self.save_query_open = true;
            self.save_query_name.clear();
        }
        if run {
            self.run_query();
        }
        if let Some(result) = self.result.as_ref() {
            if export_csv {
                save_file("query.csv", &query_to_csv(result));
            }
            if export_json {
                save_file("query.json", &query_to_json(result));
            }
        }
    }

    /// The editor proper: a reserved gutter column, then the field, then the
    /// line numbers painted into the gutter at the galley's own row positions.
    ///
    /// The numbers cannot be laid out before the field, because until the field
    /// has been laid out nobody knows how many *visual* rows the text occupies.
    /// Reserving the width first and painting into it afterwards is what makes
    /// the gutter track wrapping instead of guessing at it.
    fn ui_wql_field(&mut self, ui: &mut egui::Ui) {
        let font = TextStyle::Monospace.resolve(ui.style());
        let digit_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_w = digit_w * GUTTER_DIGITS as f32;

        Frame::NONE
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // Zero height: the column only claims horizontal space. Its
                    // height comes from the field beside it, which is exactly
                    // what the numbers are aligned to.
                    let (gutter, _) =
                        ui.allocate_exact_size(Vec2::new(gutter_w, 0.0), Sense::hover());

                    let output = TextEdit::multiline(&mut self.query_text)
                        .font(TextStyle::Monospace)
                        .desired_rows(EDITOR_ROWS)
                        .desired_width(f32::INFINITY)
                        // No frame of its own: the surface panel above already
                        // supplies the ground and the padding, and a second
                        // inset box inside it reads as two nested fields.
                        .frame(Frame::NONE)
                        .hint_text(RichText::new("SELECT * FROM Win32_Process").color(muted(30)))
                        .show(ui);
                    focus_ring(ui, &output.response);

                    let ends: Vec<bool> = output
                        .galley
                        .rows
                        .iter()
                        .map(|row| row.ends_with_newline)
                        .collect();
                    let painter = ui.painter();
                    for (row, label) in output.galley.rows.iter().zip(gutter_labels(&ends)) {
                        let Some(n) = label else { continue };
                        painter.text(
                            Pos2::new(gutter.right(), output.galley_pos.y + row.pos.y),
                            Align2::RIGHT_TOP,
                            n.to_string(),
                            font.clone(),
                            muted(25),
                        );
                    }
                });
            });
    }

    // -- status strip ------------------------------------------------------

    /// "Completed in N ms · M rows · SELECT is projected server-side", plus the
    /// ORDER BY note when it applies.
    fn ui_query_status(&mut self, ui: &mut egui::Ui) {
        // Derived from the editor buffer rather than from the query that
        // produced the rows on screen: it is advice about the query you are
        // about to pay for, and arriving after the sort has already happened
        // would make it a post-mortem.
        let warn_order_by = has_order_by(&self.query_text);
        let result = self.result.as_ref();

        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.query_loading {
                        spinner(ui, "running\u{2026}");
                    } else if let Some(result) = result {
                        let text = format!("Completed in {}", format_ms(result.elapsed_ms));
                        ui.label(icons::labelled_styled(
                            ui,
                            icons::CHECK_CIRCLE,
                            &text,
                            TextStyle::Small,
                            OK,
                        ))
                        .on_hover_text(format!(
                            "Enumeration time, measured on the worker thread.\n\
                             Namespace bind: {} (not included).",
                            format_ms(result.connect_ms),
                        ));
                        dot(ui);
                        ui.label(
                            RichText::new(rows_label(result.rows.len()))
                                .text_style(TextStyle::Small)
                                .color(muted(50)),
                        );
                        if let Some(note) = result.completion.note() {
                            dot(ui);
                            ui.label(RichText::new(note).text_style(TextStyle::Small).color(WARN));
                        }
                    } else {
                        ui.label(
                            RichText::new("No query run yet")
                                .text_style(TextStyle::Small)
                                .color(muted(45)),
                        );
                    }
                    dot(ui);
                    ui.label(
                        RichText::new("SELECT is projected server-side")
                            .text_style(TextStyle::Small)
                            .color(muted(50)),
                    );

                    if warn_order_by {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(icons::labelled_styled(
                                ui,
                                icons::WARNING_CIRCLE,
                                "ORDER BY is not valid WQL \u{2014} sort by clicking a column",
                                TextStyle::Small,
                                WARN,
                            ))
                            .on_hover_text(
                                "Measured: WMI's query parser refuses every ORDER BY form \
                                 with WBEM_E_INVALID_QUERY (0x80041017) before returning a \
                                 row. Sorting here is a column-header click, over the rows \
                                 the query did return.",
                            );
                        });
                    }
                });
            });
        editor_edge(ui);
    }

    // -- results -----------------------------------------------------------

    fn ui_query_results(&mut self, ui: &mut egui::Ui) {
        Frame::NONE
            .inner_margin(Margin::symmetric(S4 as i8, S2 as i8))
            .show(ui, |ui| {
                let Some(result) = self.result.as_ref() else {
                    if !self.query_loading {
                        empty_state(
                            ui,
                            icons::TERMINAL_WINDOW,
                            "No results",
                            "Write a WQL query above and run it.",
                        );
                    }
                    return;
                };

                if result.columns.is_empty() {
                    empty_state(
                        ui,
                        icons::PROHIBIT,
                        "No columns",
                        "The query returned no properties. A projection that names only \
                         system properties comes back like this.",
                    );
                    return;
                }
                if result.rows.is_empty() {
                    empty_state(
                        ui,
                        icons::LIST_BULLETS,
                        "No rows",
                        "The query is valid and matched nothing.",
                    );
                    return;
                }

                let ncols = result.columns.len();
                // A column is numeric only if every non-empty cell parses as a
                // number -- measured from the data, not guessed from the name.
                let numeric: Vec<bool> = (0..ncols)
                    .map(|c| column_is_numeric(&result.rows, c))
                    .collect();
                let rows = &result.rows;

                let mut table = DataTableState {
                    sort: self.result_sort,
                    selected: self.selected_row,
                };
                DataTable::new("query-results")
                    .columns(result.columns.iter().enumerate().map(|(c, name)| {
                        TableColumn::initial(name.as_str(), COL_W)
                            .at_least(COL_MIN)
                            .numeric(numeric[c])
                    }))
                    .selectable(true)
                    .sort_key(|row, col| rows[row].get(col).cloned().unwrap_or_default())
                    .show(ui, &mut table, rows.len(), |row| {
                        let cells = &rows[row.data_index()];
                        // Driven by the per-column typing rather than by the
                        // row's own length: a row shorter than the column union
                        // still has to fill every cell, or the table's columns
                        // and its contents drift apart.
                        for (col, is_numeric) in numeric.iter().enumerate() {
                            let value = cells.get(col).map(String::as_str).unwrap_or("");
                            if *is_numeric {
                                row.text(value);
                            } else {
                                row.path(value);
                            }
                        }
                    });

                self.result_sort = table.sort;
                self.selected_row = table.selected;
            });
    }

    /// The selected row's full property set, revealed beside the table.
    fn ui_row_detail(&mut self, ui: &mut egui::Ui) {
        column_edge_left(ui);
        let mut close = false;
        Frame::NONE
            .inner_margin(Margin::symmetric(S3 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ROW")
                            .text_style(TextStyle::Small)
                            .color(muted(55)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if btn_icon(ui, icons::X).on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                hrule(ui);

                let Some((index, result)) = self.selected_row.zip(self.result.as_ref()) else {
                    return;
                };
                let Some(cells) = result.rows.get(index) else {
                    // The selection outlived the result it pointed into.
                    ui.label(RichText::new("Row no longer in the result.").color(muted(45)));
                    return;
                };

                ui.label(
                    RichText::new(format!("{} of {}", index + 1, result.rows.len()))
                        .text_style(TextStyle::Small)
                        .color(muted(45)),
                );
                ui.add_space(S2);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let pairs: Vec<(&str, &str)> = result
                            .columns
                            .iter()
                            .enumerate()
                            .map(|(c, name)| {
                                (
                                    name.as_str(),
                                    cells.get(c).map(String::as_str).unwrap_or(""),
                                )
                            })
                            .collect();
                        kv_grid_sized(ui, "vs_query_row_detail", DETAIL_KEY_W, pairs);
                    });
            });
        if close {
            self.selected_row = None;
        }
    }

    // -- history -----------------------------------------------------------

    fn ui_query_history(&mut self, ui: &mut egui::Ui) {
        column_edge_left(ui);

        let mut load: Option<(String, String)> = None;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        Frame::NONE
            .inner_margin(Margin::symmetric(S2 as i8, S2 as i8))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("HISTORY")
                        .text_style(TextStyle::Small)
                        .color(muted(55)),
                );
                hrule(ui);

                if self.config.history.is_empty() {
                    ui.label(
                        RichText::new("Queries you run appear here.")
                            .text_style(TextStyle::Small)
                            .color(muted(40)),
                    );
                    return;
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for entry in &self.config.history {
                            let response = Frame::NONE
                                .corner_radius(R_MD)
                                .inner_margin(Margin::symmetric(S2 as i8, S2 as i8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add(
                                        Label::new(
                                            RichText::new(&entry.wql)
                                                .text_style(TextStyle::Monospace)
                                                .size(HISTORY_TEXT_SIZE)
                                                .color(muted(82)),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                    ui.horizontal(|ui| {
                                        // An entry whose reply has not landed (or
                                        // never will, because the query failed)
                                        // shows an em dash, never a zero.
                                        let (ms_text, ms_color) = match entry.elapsed_ms {
                                            Some(ms) if ms >= SLOW_MS => (format_ms(ms), WARN),
                                            Some(ms) => (format_ms(ms), muted(42)),
                                            None => ("\u{2014}".to_string(), muted(30)),
                                        };
                                        ui.label(icons::labelled_styled(
                                            ui,
                                            icons::TIMER,
                                            &ms_text,
                                            TextStyle::Small,
                                            ms_color,
                                        ));
                                        let rows = match entry.rows {
                                            Some(n) => rows_label(n),
                                            None => "\u{2014}".to_string(),
                                        };
                                        meta(ui, &rows);
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let when = entry
                                                    .at
                                                    .map(|at| relative_time(at, now))
                                                    .unwrap_or_else(|| "\u{2014}".to_string());
                                                meta(ui, &when);
                                            },
                                        );
                                    });
                                })
                                .response
                                .interact(Sense::click());

                            if response.hovered() {
                                ui.painter().rect_filled(
                                    response.rect,
                                    R_MD,
                                    muted(HISTORY_HOVER_TINT),
                                );
                            }
                            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if response.clicked() {
                                load = Some((entry.wql.clone(), entry.namespace.clone()));
                            }
                        }
                    });
            });

        if let Some((wql, namespace)) = load {
            // Namespace first: `run_query` reads `active_ns`, so restoring the
            // text without the namespace would run the right query against the
            // wrong repository -- the same class of bug as task 4.16.
            //
            // An entry migrated from a v1 config has no namespace; leave the
            // active one alone rather than guessing.
            if !namespace.is_empty() {
                self.select_namespace(namespace);
            }
            self.query_text = wql;
            self.run_query();
        }
    }
}

/// Minimum width of the row-detail key column. WMI property names run long
/// (`WorkingSetPrivate`, `ParentProcessId`), and a key column discovered from
/// the first row would reflow on every selection.
const DETAIL_KEY_W: f32 = 128.0;

/// Hover strength on a history row, as a percentage of the body colour. The same
/// 6% the palette's rows use, so "this is clickable" reads the same in both.
const HISTORY_HOVER_TINT: u8 = 6;

/// The middle dot between status-strip and meta-line items.
fn dot(ui: &mut egui::Ui) {
    ui.label(
        RichText::new("\u{00b7}")
            .text_style(TextStyle::Small)
            .color(muted(28)),
    );
}

/// One muted meta item in a history row.
fn meta(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(HISTORY_META_SIZE).color(muted(42)));
}

/// A full-bleed hairline under a band of the editor stack.
///
/// Solid rather than faded: these separate the parts of one control surface, and
/// the design's fade is for freestanding rules. `hrule` would also add its own
/// breathing room, which is exactly what an edge-to-edge band must not have.
fn editor_edge(ui: &mut egui::Ui) {
    let (_, rect) = ui.allocate_space(Vec2::new(ui.available_width(), HAIRLINE));
    solid_hline(ui.painter(), rect, DIVIDER);
}

/// Paint a right-hand panel's own left edge, flush with its outer rect.
fn column_edge_left(ui: &egui::Ui) {
    let r = ui.max_rect();
    crate::widgets::rule::solid_vline(
        ui.painter(),
        egui::Rect::from_min_max(r.left_top(), Pos2::new(r.left() + HAIRLINE, r.bottom())),
        DIVIDER,
    );
}

// The shared empty state -- a large muted glyph, a heading, and one line saying
// what would fill it -- was defined here and is now
// `widgets::loading::empty_state`: task 7.6's audit found two other views that
// needed exactly this and one that had reimplemented it, so it belongs in the
// kit rather than in whichever view happened to need it first.

#[cfg(test)]
mod tests {
    use super::*;

    // -- ORDER BY derivation ----------------------------------------------

    /// The whole point of deriving it: a query without the clause must show no
    /// note (task 4.3's acceptance).
    #[test]
    fn a_query_without_order_by_shows_no_note() {
        for wql in [
            "SELECT * FROM Win32_Process",
            "SELECT Name, ProcessId FROM Win32_Process WHERE ProcessId > 4",
            "",
            "ASSOCIATORS OF {Win32_Process.Handle=\"4\"}",
        ] {
            assert!(!has_order_by(wql), "{wql:?} was read as having ORDER BY");
        }
    }

    #[test]
    fn order_by_is_found_whatever_its_casing_or_spacing() {
        for wql in [
            "SELECT * FROM Win32_Process ORDER BY Name",
            "select name from win32_process order by name desc",
            "SELECT * FROM Win32_Process\n  ORDER BY WorkingSetSize DESC",
            // Broken over two lines: still one clause.
            "SELECT * FROM Win32_Process ORDER\nBY Name",
            "SELECT * FROM Win32_Process    ORDER     BY   Name",
        ] {
            assert!(has_order_by(wql), "{wql:?} was not recognised");
        }
    }

    /// The reason this is a lexer scan and not a `contains`: the words inside a
    /// literal are data, not syntax.
    #[test]
    fn order_by_inside_a_string_literal_is_not_a_clause() {
        assert!(!has_order_by(
            "SELECT * FROM Win32_Service WHERE Name = 'order by'"
        ));
        assert!(!has_order_by(
            "SELECT * FROM Win32_Service WHERE Caption = \"ORDER BY\""
        ));
        // A doubled quote escapes rather than closing, so the clause-looking
        // text after it is still inside the literal.
        assert!(!has_order_by(
            "SELECT * FROM Win32_Service WHERE Name = 'say ''order by'' now'"
        ));
        // But a real clause after a literal that mentions it is still a clause.
        assert!(has_order_by(
            "SELECT * FROM Win32_Service WHERE Name = 'order by' ORDER BY Name"
        ));
    }

    /// Identifiers that merely start with the word are not the clause.
    #[test]
    fn order_alone_is_not_order_by() {
        assert!(!has_order_by("SELECT OrderBy FROM Widgets"));
        assert!(!has_order_by("SELECT * FROM Orders WHERE Order = 1"));
        assert!(!has_order_by("SELECT * FROM X GROUP BY Y"));
    }

    // -- gutter ------------------------------------------------------------

    /// Unwrapped: every visual row is a logical line.
    #[test]
    fn the_gutter_numbers_every_row_when_nothing_wraps() {
        // Four lines: three end with a newline, the last does not.
        let labels = gutter_labels(&[true, true, true, false]);
        assert_eq!(labels, vec![Some(1), Some(2), Some(3), Some(4)]);
    }

    /// Wrapped: a continuation row gets no number, and the next real line
    /// carries on from where the numbering left off rather than from the row
    /// index (task 4.1's acceptance).
    #[test]
    fn a_wrapped_line_is_numbered_once() {
        // Line 1 wraps over three rows, then line 2 wraps over two.
        let labels = gutter_labels(&[false, false, true, false, false]);
        assert_eq!(labels, vec![Some(1), None, None, Some(2), None]);
    }

    /// A trailing newline produces one more (empty) row, which is a real line
    /// and must be numbered -- that empty line is where the caret sits.
    #[test]
    fn a_trailing_newline_gets_its_own_number() {
        let labels = gutter_labels(&[true, false]);
        assert_eq!(labels, vec![Some(1), Some(2)]);
    }

    #[test]
    fn an_empty_galley_has_no_numbers() {
        assert!(gutter_labels(&[]).is_empty());
    }

    // -- relative time -----------------------------------------------------

    #[test]
    fn relative_time_is_coarse_and_never_negative() {
        let now = 1_750_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now - 59, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 min ago");
        assert_eq!(relative_time(now - 3_599, now), "59 min ago");
        assert_eq!(relative_time(now - 3_600, now), "1 h ago");
        assert_eq!(relative_time(now - 86_399, now), "23 h ago");
        assert_eq!(relative_time(now - 86_400, now), "1 d ago");
        // A clock that went backwards, and a zero stamp, are both "we do not
        // know" rather than "just now".
        assert_eq!(relative_time(now + 10, now), "\u{2014}");
        assert_eq!(relative_time(0, now), "\u{2014}");
    }

    #[test]
    fn one_row_is_a_row() {
        assert_eq!(rows_label(0), "0 rows");
        assert_eq!(rows_label(1), "1 row");
        assert_eq!(rows_label(187), "187 rows");
    }

    // -- column typing -----------------------------------------------------

    #[test]
    fn numeric_detection_needs_every_nonempty_cell_to_parse() {
        let data: Vec<Vec<String>> = [
            ["42", "root", "", "3.5"],
            ["17", "12", "", "x"],
            ["8", "9", "", "1"],
        ]
        .iter()
        .map(|r| r.iter().map(|s| (*s).to_string()).collect())
        .collect();
        assert!(column_is_numeric(&data, 0));
        assert!(!column_is_numeric(&data, 1));
        assert!(!column_is_numeric(&data, 2), "all empty -> not numeric");
        assert!(!column_is_numeric(&data, 3));
    }
}
