//! The Saved view: the local query library as a card grid.
//!
//! The mock calls this a "Shared library … synced from `\\fileserv\wmi\
//! library.json`". There is no sync engine here, so the header does not claim
//! one: it reads **Local library**, names the file the queries actually live in,
//! and offers Import…/Export… instead. A UNC path is a valid file path, so
//! importing from a share is real -- it just is not synchronisation, and calling
//! it that in a security tool would be the same class of lie as a fabricated row
//! count.
//!
//! The cards' metadata is real or absent. `ms` and `rows` come from the last time
//! that exact query ran in that exact namespace (`Config::note_query_run`); a
//! query that has never run shows an em dash rather than a plausible number. The
//! author is `USERDOMAIN\USERNAME` read at save time and stored, so an imported
//! query keeps the name of whoever saved it.

use eframe::egui::{self, Frame, Label, Margin, RichText, TextStyle};

use crate::app::VmiScopeApp;
use crate::config::SavedQuery;
use crate::theme::icons;
use crate::theme::tokens::{muted, S2, S3, S4, S6, WARN};
use crate::views::nav::View;
use crate::views::query::rows_label;
use crate::widgets::button::{accent, btn_icon, btn_primary, btn_secondary};
use crate::widgets::card::{card_grid, clickable_card};
use crate::widgets::chip::{tag_neutral, tag_outline};
use crate::widgets::field::{filter_box, mono_input};
use crate::widgets::loading::{format_ms, SLOW_MS};
use crate::widgets::rule::hrule;

/// Minimum card width. The mock's `minmax(310px, 1fr)`.
const CARD_MIN_W: f32 = 310.0;

/// The grid stops widening past this, so on a 2560px monitor the cards stay
/// cards instead of becoming rows.
const GRID_MAX_W: f32 = 1180.0;

/// How many lines of the query a card previews before eliding. Three is enough
/// to see the shape of a `SELECT … FROM … WHERE` without the card becoming the
/// editor.
const PREVIEW_LINES: usize = 3;

/// Em dash for a metric that was never measured.
const UNMEASURED: &str = "\u{2014}";

/// The label the ungrouped bucket shows in the folder filter.
const UNGROUPED: &str = "Ungrouped";

/// Everything the Saved view remembers between frames.
///
/// A struct rather than four fields on `VmiScopeApp`, for the same reason
/// `ProcessView` is one: these are the view's own filters and mean nothing
/// outside it.
#[derive(Default)]
pub(crate) struct SavedView {
    /// Free-text filter over name, folder and query text.
    pub(crate) filter: String,
    /// `None` = every folder. `Some("")` = the ungrouped bucket.
    pub(crate) folder: Option<String>,
    pub(crate) favs_only: bool,
    /// Result of the last import, shown until the next one. Kept so an import
    /// that matched nothing says so instead of looking like it did nothing.
    pub(crate) last_import: Option<String>,
    /// Name of the card whose folder tag is currently an input, if any.
    ///
    /// Folders have no registry -- naming one on a card is what creates it -- so
    /// the editor is the tag itself rather than a dialog listing folders that
    /// might not have any queries in them.
    pub(crate) editing_folder: Option<String>,
    /// The folder being typed, while `editing_folder` is set.
    pub(crate) folder_draft: String,
}

impl SavedView {
    /// Does `query` pass the active filters?
    ///
    /// Free of any `egui` type on purpose: this is the whole of the view's
    /// filtering logic and it is worth being able to test it directly.
    fn matches(&self, query: &SavedQuery) -> bool {
        if self.favs_only && !query.fav {
            return false;
        }
        if let Some(folder) = &self.folder {
            if &query.folder != folder {
                return false;
            }
        }
        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        query.name.to_lowercase().contains(&needle)
            || query.folder.to_lowercase().contains(&needle)
            || query.wql.to_lowercase().contains(&needle)
    }
}

/// The card's query preview: the first few lines, whitespace-normalised.
///
/// WQL saved from the editor can carry indentation and blank lines that make a
/// three-line preview show one word. Collapsing runs of whitespace inside each
/// line -- and dropping empty ones -- keeps the preview about the query.
fn preview(wql: &str) -> String {
    let mut lines: Vec<String> = wql
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect();
    let elided = lines.len() > PREVIEW_LINES;
    lines.truncate(PREVIEW_LINES);
    let mut out = lines.join("\n");
    if elided {
        out.push_str("\n\u{2026}");
    }
    out
}

/// The meta line's `ms` half: the real figure, or an em dash.
fn last_ms_label(ms: Option<u64>) -> String {
    ms.map_or_else(|| UNMEASURED.to_string(), format_ms)
}

/// The meta line's `rows` half.
fn last_rows_label(rows: Option<usize>) -> String {
    rows.map_or_else(|| UNMEASURED.to_string(), rows_label)
}

impl VmiScopeApp {
    pub(crate) fn ui_saved(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(S4 as i8, S3 as i8))
                    .show(ui, |ui| {
                        ui.set_max_width(GRID_MAX_W.min(ui.available_width()));
                        self.saved_header(ui);
                        self.saved_filters(ui);
                        hrule(ui);
                        self.saved_grid(ui);
                    });
            });
    }

    /// Title, the honest subtitle, and the three library-level actions.
    fn saved_header(&mut self, ui: &mut egui::Ui) {
        let count = self.config.saved.len();
        let path = crate::config::library_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no APPDATA \u{2014} nothing is persisted".to_string());

        let mut new_query = false;
        let mut do_import = false;
        let mut do_export = false;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(icons::labelled_styled(
                    ui,
                    icons::BOOKMARK_SIMPLE,
                    "Saved queries",
                    TextStyle::Heading,
                    ui.visuals().text_color(),
                ));
                // No "synced" claim: this file is the library, and nothing
                // reconciles it with anyone else's.
                ui.add(
                    Label::new(
                        RichText::new(format!(
                            "Local library \u{00b7} {count} {} \u{00b7} {path}",
                            if count == 1 { "query" } else { "queries" }
                        ))
                        .text_style(TextStyle::Small)
                        .color(muted(50)),
                    )
                    .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if btn_primary(ui, icons::labelled(ui, icons::PLUS, "New query")).clicked() {
                    new_query = true;
                }
                if btn_secondary(
                    ui,
                    icons::labelled(ui, icons::UPLOAD_SIMPLE, "Export\u{2026}"),
                )
                .on_hover_text("Write the whole library to a JSON file")
                .clicked()
                {
                    do_export = true;
                }
                if btn_secondary(
                    ui,
                    icons::labelled(ui, icons::DOWNLOAD_SIMPLE, "Import\u{2026}"),
                )
                .on_hover_text(
                    "Merge a library file in, replacing by name.\n\
                         A UNC path (\\\\server\\share\\library.json) works here \u{2014} \
                         it is read once, not synchronised.",
                )
                .clicked()
                {
                    do_import = true;
                }
            });
        });

        if let Some(note) = &self.saved_view.last_import {
            ui.label(
                RichText::new(note)
                    .text_style(TextStyle::Small)
                    .color(muted(55)),
            );
        }

        if new_query {
            // A new query is an empty editor, not a dialog: the library gains an
            // entry when it is saved, and offering a name box before there is
            // anything to name gets the order backwards.
            self.query_text.clear();
            self.view = View::Query;
        }
        if do_export {
            crate::util::save_file("vmiscope-library.json", &self.config.library_to_json());
        }
        if do_import {
            self.import_library();
        }
    }

    /// Filter row: text, folder chips, favourites.
    fn saved_filters(&mut self, ui: &mut egui::Ui) {
        let folders = self.config.folders();
        let has_ungrouped = self.config.saved.iter().any(|q| q.folder.is_empty());

        ui.add_space(S2);
        ui.horizontal_wrapped(|ui| {
            ui.scope(|ui| {
                ui.set_max_width(FILTER_W);
                filter_box(ui, &mut self.saved_view.filter, "filter library");
            });

            // Chips rather than a combo: with a handful of folders every one is
            // one click away, and the set is visible without opening anything.
            if ui
                .selectable_label(self.saved_view.folder.is_none(), "All")
                .clicked()
            {
                self.saved_view.folder = None;
            }
            for folder in &folders {
                let selected = self.saved_view.folder.as_deref() == Some(folder.as_str());
                if ui
                    .selectable_label(selected, icons::labelled(ui, icons::FOLDER, folder))
                    .clicked()
                {
                    self.saved_view.folder = if selected { None } else { Some(folder.clone()) };
                }
            }
            if has_ungrouped {
                let selected = self.saved_view.folder.as_deref() == Some("");
                if ui.selectable_label(selected, UNGROUPED).clicked() {
                    self.saved_view.folder = if selected { None } else { Some(String::new()) };
                }
            }

            let favs = self.saved_view.favs_only;
            if ui
                .selectable_label(favs, icons::labelled(ui, icons::STAR, "Favourites"))
                .clicked()
            {
                self.saved_view.favs_only = !favs;
            }
        });
    }

    /// The card grid, and the empty states around it.
    fn saved_grid(&mut self, ui: &mut egui::Ui) {
        if self.config.saved.is_empty() {
            empty(
                ui,
                icons::BOOKMARK_SIMPLE,
                "Nothing saved yet",
                "Run a query and press Save, or import a library file.",
            );
            return;
        }

        // Cloned so the grid closure does not hold a borrow of `self.config`
        // while the card actions mutate it. A library is tens of entries; the
        // clone is not the cost here.
        let visible: Vec<SavedQuery> = self
            .config
            .saved
            .iter()
            .filter(|q| self.saved_view.matches(q))
            .cloned()
            .collect();

        if visible.is_empty() {
            empty(
                ui,
                icons::FUNNEL,
                "No queries match",
                "Clear the filter, the folder or the favourites toggle.",
            );
            return;
        }

        let mut apply: Option<SavedQuery> = None;
        let mut toggle_fav: Option<String> = None;
        let mut delete: Option<String> = None;
        let mut edit_folder: Option<String> = None;
        let mut commit_folder: Option<(String, String)> = None;
        // Split off so the grid closure can borrow the draft mutably while the
        // rest of `self` stays available afterwards.
        let editing = self.saved_view.editing_folder.clone();
        let draft = &mut self.saved_view.folder_draft;

        card_grid(ui, CARD_MIN_W, &visible, |ui, query| {
            let editing_this = editing.as_deref() == Some(query.name.as_str());
            let (act, response) = clickable_card(ui, |ui| {
                // `card_grid` lays its items out inside a `horizontal_top`, and
                // `Ui::allocate_ui` inherits the parent's layout -- so the `Ui`
                // handed to a card body is *horizontal*. Without this the card's
                // four bands render side by side and the wrapped mono preview
                // wraps to one character per line. Measured: that is exactly
                // what the first capture of this view showed.
                //
                // `set_min_width` on top, because a vertical child of a
                // horizontal parent otherwise shrinks to its widest line and the
                // cards in a row end up different widths.
                ui.vertical(|ui| {
                    ui.set_min_width(ui.available_width());
                    card_body(ui, query, editing_this, draft)
                })
                .inner
            });

            match act {
                CardAction::Star => toggle_fav = Some(query.name.clone()),
                CardAction::Delete => delete = Some(query.name.clone()),
                CardAction::EditFolder => edit_folder = Some(query.name.clone()),
                CardAction::CommitFolder(folder) => {
                    commit_folder = Some((query.name.clone(), folder));
                }
                // Not while the folder field is open: the field sits inside the
                // card, so a click that lands beside it would otherwise both
                // commit nothing and navigate away.
                CardAction::None if response.clicked() && !editing_this => {
                    apply = Some(query.clone());
                }
                CardAction::None => {}
            }
        });

        if let Some(name) = edit_folder {
            self.saved_view.folder_draft = self
                .config
                .saved
                .iter()
                .find(|q| q.name == name)
                .map(|q| q.folder.clone())
                .unwrap_or_default();
            self.saved_view.editing_folder = Some(name);
        } else if let Some((name, folder)) = commit_folder {
            self.config.set_folder(&name, &folder);
            self.saved_view.editing_folder = None;
            self.saved_view.folder_draft.clear();
        } else if let Some(name) = toggle_fav {
            self.config.toggle_favourite(&name);
        } else if let Some(name) = delete {
            self.config.delete_saved(&name);
        } else if let Some(query) = apply {
            self.apply_saved_query(&query);
        }

        // Escape leaves the folder field without writing, which is the only way
        // out that does not name a folder.
        if self.saved_view.editing_folder.is_some()
            && ui.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.saved_view.editing_folder = None;
            self.saved_view.folder_draft.clear();
        }
    }

    /// Open a saved query in the Query view and run it.
    ///
    /// **Task 4.16.** The namespace has always been stored and was never read
    /// back, so a query saved under `root\subscription` reopened against
    /// whatever namespace happened to be active -- usually `root\CIMV2`, where
    /// `__EventFilter` does not exist. Restoring the namespace *first* is the
    /// whole fix: `run_query` reads `active_ns`, so the order is load-bearing.
    fn apply_saved_query(&mut self, query: &SavedQuery) {
        if !query.namespace.is_empty() {
            self.select_namespace(query.namespace.clone());
        }
        self.query_text = query.wql.clone();
        self.view = View::Query;
        self.run_query();
    }

    /// Ask for a library file. Returns at once; see `crate::io`.
    fn import_library(&mut self) {
        crate::io::pick(crate::io::PickFor::SavedLibrary, "JSON", &["json"]);
    }

    /// Merge a picked library file in. Reports what happened, including
    /// "nothing". Called from `drain_io`.
    ///
    /// The path is no longer part of the note: the read happened on the IO
    /// thread and the view is told the contents, not where they came from. That
    /// is a small loss and the honest one -- a note naming a file this code
    /// never opened would be a guess.
    pub(crate) fn apply_library_file(&mut self, text: &str) {
        let note = match crate::config::Config::library_from_json(text) {
            Ok(incoming) => {
                let found = incoming.len();
                let (added, replaced) = self.config.merge_library(incoming);
                format!("Imported {found}: {added} added, {replaced} replaced.")
            }
            Err(e) => {
                self.push_error(format!("Import library: {e}"));
                "The file could not be read as a query library.".to_string()
            }
        };
        self.saved_view.last_import = Some(note);
    }
}

/// Width of the library filter box. A library filter is a word or two, not a
/// path, so it does not need the pane.
const FILTER_W: f32 = 240.0;

/// Width of the in-card folder field. A folder name is a word; anything wider
/// would push the card's title out of its own header row.
const FOLDER_EDIT_W: f32 = 110.0;

/// The card title size. Between `Body` (13) and `Heading`, matching the mock's
/// 14.5px card title.
const CARD_TITLE_SIZE: f32 = 14.5;

/// The mono query preview's size.
const CARD_PREVIEW_SIZE: f32 = 11.5;

/// What a card reported this frame.
///
/// One value rather than four `bool`s out-parameters: the card's controls are
/// mutually exclusive (you cannot star and delete in the same click), and saying
/// so in the type is what keeps the caller's `match` exhaustive.
enum CardAction {
    None,
    Star,
    Delete,
    /// The folder tag was clicked; open the field.
    EditFolder,
    /// The folder field was committed with this text.
    CommitFolder(String),
}

/// One card's contents. Called inside a **vertical** `Ui` -- see the note at the
/// call site for why that is not the default.
fn card_body(
    ui: &mut egui::Ui,
    query: &SavedQuery,
    editing_folder: bool,
    draft: &mut String,
) -> CardAction {
    let mut action = CardAction::None;

    ui.horizontal(|ui| {
        let star_color = if query.fav { accent(ui) } else { muted(30) };
        if ui
            .add(
                Label::new(icons::glyph(icons::STAR).size(STAR_SIZE).color(star_color))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(if query.fav {
                "Remove from favourites"
            } else {
                "Add to favourites"
            })
            .clicked()
        {
            action = CardAction::Star;
        }
        ui.add(
            Label::new(RichText::new(&query.name).size(CARD_TITLE_SIZE))
                .truncate()
                .selectable(false),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if btn_icon(ui, icons::TRASH)
                .on_hover_text("Delete this saved query")
                .clicked()
            {
                action = CardAction::Delete;
            }
            if editing_folder {
                ui.scope(|ui| {
                    ui.set_max_width(FOLDER_EDIT_W);
                    let field = mono_input(ui, draft, "folder");
                    field.request_focus();
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        action = CardAction::CommitFolder(draft.clone());
                    }
                });
            } else {
                let label = if query.folder.is_empty() {
                    UNGROUPED
                } else {
                    query.folder.as_str()
                };
                if tag_neutral(ui, label)
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::Text)
                    .on_hover_text("Click to file this query under a folder")
                    .clicked()
                {
                    action = CardAction::EditFolder;
                }
            }
        });
    });

    // The namespace is on the card because it is half the query: the same WQL
    // against `root\subscription` and `root\CIMV2` are different questions, and
    // task 4.16 exists because that was once invisible.
    ui.horizontal(|ui| {
        tag_outline(ui, &query.namespace);
    });

    ui.add(
        Label::new(
            RichText::new(preview(&query.wql))
                .text_style(TextStyle::Monospace)
                .size(CARD_PREVIEW_SIZE)
                .color(muted(58)),
        )
        .wrap()
        .selectable(false),
    );

    ui.horizontal(|ui| {
        let (ms_text, ms_color) = match query.last_ms {
            Some(ms) if ms >= SLOW_MS => (last_ms_label(query.last_ms), WARN),
            Some(_) => (last_ms_label(query.last_ms), muted(45)),
            None => (UNMEASURED.to_string(), muted(30)),
        };
        ui.label(icons::labelled_styled(
            ui,
            icons::TIMER,
            &ms_text,
            TextStyle::Small,
            ms_color,
        ))
        .on_hover_text(if query.last_ms.is_some() {
            "The last measured run of this exact query in this namespace."
        } else {
            "This query has not run since it was saved, so there is nothing to report."
        });
        meta(ui, &last_rows_label(query.last_rows));
        if !query.author.is_empty() {
            meta(ui, &query.author);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(icons::glyph(icons::PLAY).size(PLAY_SIZE).color(accent(ui)));
        });
    });

    action
}

/// The favourite star and the run glyph, both sized to sit inside a card's
/// text rows rather than beside them.
const STAR_SIZE: f32 = 13.0;
const PLAY_SIZE: f32 = 12.0;

/// One muted meta item on a card.
fn meta(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .text_style(TextStyle::Small)
            .color(muted(45)),
    );
}

/// The view's empty states.
fn empty(ui: &mut egui::Ui, icon: &str, title: &str, note: &str) {
    ui.add_space(S6);
    ui.vertical_centered(|ui| {
        ui.label(icons::glyph(icon).size(30.0).color(muted(20)));
        ui.add_space(S3);
        ui.label(RichText::new(title).color(muted(55)));
        ui.label(
            RichText::new(note)
                .text_style(TextStyle::Small)
                .color(muted(38)),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(name: &str, folder: &str, fav: bool, wql: &str) -> SavedQuery {
        SavedQuery {
            name: name.into(),
            namespace: "root\\CIMV2".into(),
            wql: wql.into(),
            folder: folder.into(),
            fav,
            ..Default::default()
        }
    }

    fn library() -> Vec<SavedQuery> {
        vec![
            q("Processes", "Triage", true, "SELECT * FROM Win32_Process"),
            q("Services", "Triage", false, "SELECT * FROM Win32_Service"),
            q("Shares", "", false, "SELECT * FROM Win32_Share"),
        ]
    }

    fn names(view: &SavedView) -> Vec<String> {
        library()
            .iter()
            .filter(|query| view.matches(query))
            .map(|query| query.name.clone())
            .collect()
    }

    /// Nothing set: the whole library.
    #[test]
    fn an_empty_filter_matches_everything() {
        let view = SavedView::default();
        assert_eq!(names(&view), vec!["Processes", "Services", "Shares"]);
    }

    /// Task 4.18's acceptance, half one: the folder filter.
    #[test]
    fn the_folder_filter_selects_one_folder_and_the_ungrouped_bucket() {
        let view = SavedView {
            folder: Some("Triage".into()),
            ..Default::default()
        };
        assert_eq!(names(&view), vec!["Processes", "Services"]);

        // `Some("")` is the ungrouped bucket, which is a real selection and not
        // the same as "no filter".
        let view = SavedView {
            folder: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(names(&view), vec!["Shares"]);
    }

    /// Task 4.18's acceptance, half two: the favourites filter.
    #[test]
    fn the_favourites_filter_keeps_only_starred_queries() {
        let view = SavedView {
            favs_only: true,
            ..Default::default()
        };
        assert_eq!(names(&view), vec!["Processes"]);
    }

    /// The two filters and the text box compose rather than override.
    #[test]
    fn the_filters_compose() {
        let view = SavedView {
            favs_only: true,
            folder: Some("Triage".into()),
            filter: "service".into(),
            ..Default::default()
        };
        assert!(
            names(&view).is_empty(),
            "a favourite in Triage matching 'service' does not exist, so nothing should show"
        );

        let view = SavedView {
            folder: Some("Triage".into()),
            filter: "service".into(),
            ..Default::default()
        };
        assert_eq!(names(&view), vec!["Services"]);
    }

    /// The text filter reaches the query body, not just the name -- "which of
    /// these hits `__EventFilter`" is the question a library filter is for.
    #[test]
    fn the_text_filter_searches_name_folder_and_query() {
        let by_name = SavedView {
            filter: "proc".into(),
            ..Default::default()
        };
        assert_eq!(names(&by_name), vec!["Processes"]);

        let by_folder = SavedView {
            filter: "TRIAGE".into(),
            ..Default::default()
        };
        assert_eq!(names(&by_folder), vec!["Processes", "Services"]);

        let by_body = SavedView {
            filter: "win32_share".into(),
            ..Default::default()
        };
        assert_eq!(names(&by_body), vec!["Shares"]);
    }

    // -- preview -----------------------------------------------------------

    #[test]
    fn the_preview_collapses_whitespace_and_elides_long_queries() {
        assert_eq!(
            preview("SELECT   Name,\n\n   ProcessId\nFROM Win32_Process"),
            "SELECT Name,\nProcessId\nFROM Win32_Process"
        );
        assert_eq!(
            preview("a\nb\nc\nd\ne"),
            format!("a\nb\nc\n{}", "\u{2026}"),
            "a query longer than the preview must say so"
        );
        assert_eq!(preview(""), "");
    }

    // -- metric labels -----------------------------------------------------

    /// A query that has never run shows an em dash. A zero would read as
    /// "instant, and it found nothing", which is the one thing it must not say.
    #[test]
    fn unmeasured_metrics_render_as_an_em_dash() {
        assert_eq!(last_ms_label(None), UNMEASURED);
        assert_eq!(last_rows_label(None), UNMEASURED);
        assert_eq!(last_ms_label(Some(61)), "61 ms");
        assert_eq!(last_ms_label(Some(1_483)), "1.5 s");
        assert_eq!(last_rows_label(Some(0)), "0 rows");
        assert_eq!(
            last_rows_label(Some(1)),
            "1 row",
            "'1 rows' reads as generated"
        );
        assert_eq!(last_rows_label(Some(187)), "187 rows");
    }
}
