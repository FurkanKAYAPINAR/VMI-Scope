//! The Nocturne data table.
//!
//! [`DataTable`] is one virtualised, sortable, selectable table that replaces
//! the four hand-rolled `TableBuilder` blocks in Network, Persistence,
//! Providers and the Explorer results grid. The design's table rules -- 11px
//! uppercase headers, a 1px row rule that fades over 48px at each end, a 4%
//! hover tint, a 2px accent selection marker -- live here rather than being
//! restated (and drifting) in every view.
//!
//! Almost everything below that looks over-careful is working around a real
//! `egui_extras` behaviour; each one is documented where it bites.
//!
//! There is deliberately **no** `allow(dead_code)` here. The one this module
//! carried through the reskin ("the views adopt this kit in the next commit")
//! outlived its reason and then hid `sortable_header` -- a function whose own
//! doc comment said it went away with the last hand-rolled table -- for two
//! whole phases. Taking the allow off found nine more: a `sort_changed` output
//! nobody read, four builder setters (`row_height`, `header_height`,
//! `resizable`, `row_rules`) whose defaults every one of the six tables was
//! happy with, and four `RowCtx` methods (`display_index`, `response`,
//! `tinted_cell`, `blank`). All gone. Each is three lines to bring back the day
//! a view needs it, and until then the compiler is what says the kit is the
//! size of its use.

use eframe::egui;
use egui::emath::GuiRounding as _;
use egui::{Color32, Rangef, Response};
use egui_extras::{Column, TableBuilder};

use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, DIVIDER, OK, TEXT, WARN};
use crate::util::smart_cmp;

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// The row rule fades to nothing over this much of each end of the row.
const RULE_FADE: f32 = 48.0;
/// Row rule strength, as a percentage of the body text colour.
const RULE_TINT: u8 = 8;
/// Row hover strength, as a percentage of the body text colour.
const HOVER_TINT: u8 = 4;
/// Header label strength, as a percentage of the body text colour.
const HEADER_TINT: u8 = 55;
/// Width of the trailing paint column. See [`DataTable::show`] for why it
/// exists; it is deliberately hairline-thin because nothing is drawn *in* it.
const PAINT_COL_W: f32 = 1.0;
/// Default header height. Matches `Metrics::header_h`, which the table cannot
/// read because it does not know the active density.
const HEADER_H: f32 = 22.0;
/// The selection marker: a 2px bar inset 1px from the row's left edge.
const MARKER_W: f32 = 2.0;
const MARKER_INSET: f32 = 1.0;

// ---------------------------------------------------------------------------
// Sort state
// ---------------------------------------------------------------------------

/// Which column a table is sorted by, and whether ascending. `None` is the
/// caller's natural order.
///
/// This is the same shape the pre-Nocturne views already store, so a view can
/// hand its existing field straight to [`DataTable::show`].
pub(crate) type Sort = Option<(usize, bool)>;

/// Cycle the sort for `col`: off -> ascending -> descending -> off.
///
/// Clicking a *different* column always starts at ascending, because "sorted
/// by a column I did not click, in a direction I did not pick" is never what
/// the user meant.
///
/// (`util::toggle_sort` is the pre-Nocturne twin of this function; it goes away
/// with the last hand-rolled table. The logic lives here because the tri-state
/// cycle is part of the header widget's contract, not a view-level detail.)
pub(crate) fn cycle_sort(sort: &mut Sort, col: usize) {
    *sort = match *sort {
        Some((c, true)) if c == col => Some((col, false)),
        Some((c, false)) if c == col => None,
        _ => Some((col, true)),
    };
}

/// Build the display order for `len` rows under `sort`.
///
/// **The caller's data is never touched** -- this permutes an index vector, so
/// a fading Network row keeps its identity, a selection keyed by data index
/// stays valid, and sorting a 50k-row result does not move 50k strings.
///
/// `key(row, col)` returns the sort key for a cell. It is called once per row
/// (not once per comparison), so an implementation that formats a `String` is
/// fine even on large tables.
///
/// The sort is stable, so rows that tie keep the caller's natural order.
pub(crate) fn sort_order(
    len: usize,
    sort: Sort,
    key: impl Fn(usize, usize) -> String,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if let Some((col, ascending)) = sort {
        let keys: Vec<String> = (0..len).map(|row| key(row, col)).collect();
        order.sort_by(|&a, &b| {
            let ord = smart_cmp(&keys[a], &keys[b]);
            if ascending {
                ord
            } else {
                ord.reverse()
            }
        });
    }
    order
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// How a column claims horizontal space.
///
/// `Column::auto` is deliberately not offered: an auto width is measured from
/// the rows that happen to be visible, so it jitters while a virtualised table
/// scrolls (invariant I4).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ColWidth {
    /// Starts here; the user can drag it if the table is resizable.
    Initial(f32),
    /// Always exactly this wide, never resizable.
    Exact(f32),
    /// Soaks up whatever is left. Several `Remainder` columns share it evenly.
    Remainder,
}

/// One column: its header label and how it behaves.
#[derive(Clone, Debug)]
pub(crate) struct TableColumn {
    label: String,
    width: ColWidth,
    at_least: f32,
    clip: bool,
    sortable: bool,
    numeric: bool,
    tooltip: Option<String>,
}

impl TableColumn {
    /// A column with an initial width the user can drag.
    pub(crate) fn initial(label: impl Into<String>, width: f32) -> Self {
        Self::new(label, ColWidth::Initial(width))
    }

    /// A column pinned to one width.
    pub(crate) fn exact(label: impl Into<String>, width: f32) -> Self {
        Self::new(label, ColWidth::Exact(width))
    }

    /// A column that takes whatever space is left.
    pub(crate) fn remainder(label: impl Into<String>) -> Self {
        Self::new(label, ColWidth::Remainder)
    }

    fn new(label: impl Into<String>, width: ColWidth) -> Self {
        Self {
            label: label.into(),
            width,
            at_least: 40.0,
            clip: true,
            sortable: true,
            numeric: false,
            tooltip: None,
        }
    }

    /// Never shrink below this. Ignored for [`ColWidth::Exact`], which is
    /// already a fixed range.
    pub(crate) fn at_least(mut self, width: f32) -> Self {
        self.at_least = width;
        self
    }

    /// Clip (and ellipsize) content that does not fit. On by default.
    ///
    /// **A clipped column cannot be painted into.** `Visuals::clip_rect_margin`
    /// is 0.0 in egui 0.35, so `egui_extras` shrinks a clipped cell's clip rect
    /// to exactly its `max_rect` and anything outside is discarded entirely --
    /// including the half-`item_spacing` bleed that [`cell_background`] needs to
    /// cover the gap between cells. Turn clipping *off* on any column that
    /// carries a tint (the future Compare view's value columns).
    pub(crate) fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Whether clicking the header sorts by this column. On by default.
    pub(crate) fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Right-align this column's header and cells.
    ///
    /// Alignment is a property of the column, so a cell never has to remember
    /// it. It is applied with `ui.with_layout(Layout::right_to_left(..))`:
    /// `Label::halign` does *not* work inside a table cell, because the cell's
    /// `Ui` is built with an explicit `max_rect` and the label aligns inside
    /// its own allocated rect rather than inside the cell.
    pub(crate) fn numeric(mut self, numeric: bool) -> Self {
        self.numeric = numeric;
        self
    }

    /// A tooltip on the header cell.
    ///
    /// Exists for exactly one situation, and it should stay rare: a column whose
    /// sort key is not its cell text. Persistence's Risk column sorts by
    /// severity while the cells read `High`/`Medium`/`Low`, so an ascending
    /// caret sits over a column running `Low, Medium, High` and looks inverted.
    /// The header says which order it means instead of leaving the user to
    /// infer it. A column whose key *is* its text needs nothing here.
    pub(crate) fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    fn to_extras(&self, resizable: bool) -> Column {
        match self.width {
            ColWidth::Initial(w) => Column::initial(w)
                .at_least(self.at_least)
                .resizable(resizable),
            // `Column::exact` sets its own `range(w..=w)`; `at_least` would
            // widen that range back out and un-pin the column.
            ColWidth::Exact(w) => Column::exact(w).resizable(false),
            ColWidth::Remainder => Column::remainder()
                .at_least(self.at_least)
                .resizable(resizable),
        }
        .clip(self.clip)
    }
}

// ---------------------------------------------------------------------------
// State and output
// ---------------------------------------------------------------------------

/// Everything a [`DataTable`] remembers between frames. Views own one of these
/// per table and hand it back each frame; the table reads it, then applies this
/// frame's clicks to it after the row closures have finished.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct DataTableState {
    /// Active sort column and direction.
    pub(crate) sort: Sort,
    /// Selected row, as a *data* index (not a display index).
    pub(crate) selected: Option<usize>,
}

/// What a [`DataTable::show`] did this frame.
#[derive(Clone, Default, Debug)]
pub(crate) struct DataTableOutput {
    /// `order[display_index] == data_index`, for callers that need to map back
    /// (exporting the visible order, or a follow-up "scroll to selection").
    pub(crate) order: Vec<usize>,
    /// Data index of the row clicked this frame. Already written to
    /// `state.selected` when the table is selectable.
    pub(crate) clicked: Option<usize>,
}

// ---------------------------------------------------------------------------
// The builder
// ---------------------------------------------------------------------------

/// A virtualised, sortable data table.
///
/// ```ignore
/// let out = DataTable::new("providers")
///     .columns([
///         TableColumn::initial("Provider", 220.0),
///         TableColumn::exact("Host PID", 74.0).numeric(true),
///     ])
///     .sort_key(|row, col| prov_col_value(&providers[row], col))
///     .show(ui, &mut self.providers_table, providers.len(), |row| {
///         let p = &providers[row.data_index()];
///         row.text(p.provider.as_str());
///         row.text(p.host_pid.to_string());
///     });
/// ```
pub(crate) struct DataTable<'a> {
    id_salt: &'a str,
    columns: Vec<TableColumn>,
    selectable: bool,
    sort_key: Option<Box<dyn Fn(usize, usize) -> String + 'a>>,
}

impl<'a> DataTable<'a> {
    /// A new table. `id_salt` must be unique within the parent `Ui`, because
    /// egui_extras stores column widths under it.
    pub(crate) fn new(id_salt: &'a str) -> Self {
        Self {
            id_salt,
            columns: Vec::new(),
            selectable: false,
            sort_key: None,
        }
    }

    /// Append one column.
    pub(crate) fn column(mut self, column: TableColumn) -> Self {
        self.columns.push(column);
        self
    }

    /// Append several columns.
    pub(crate) fn columns(mut self, columns: impl IntoIterator<Item = TableColumn>) -> Self {
        self.columns.extend(columns);
        self
    }

    /// Whether clicking a row selects it (writing the data index into
    /// `DataTableState::selected`). Off by default.
    pub(crate) fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// The cell-to-sort-key function. Without it the header is inert and rows
    /// render in the caller's order.
    ///
    /// It is boxed rather than a generic parameter so that `DataTable` stays a
    /// single type and can be built up across `if` branches.
    pub(crate) fn sort_key(mut self, key: impl Fn(usize, usize) -> String + 'a) -> Self {
        self.sort_key = Some(Box::new(key));
        self
    }

    /// Render the table.
    ///
    /// `add_row` is called once per *visible* row -- `egui_extras` virtualises
    /// the body, so 50k rows cost the same as 40.
    ///
    /// Two `egui_extras` behaviours shape the implementation:
    ///
    /// * **The header click needs `sense`.** A header cell's `Response` only
    ///   reports clicks because the builder sets `.sense(Sense::click())`;
    ///   without it `resp.clicked()` is always false. The click is collected
    ///   into a local and applied to `state` *after* the closures return, since
    ///   the header closure borrows the sort it would be mutating.
    /// * **The row rule needs to be painted last.** Every cell paints its own
    ///   stripe/selection/hover fill before its contents, so a rule painted
    ///   from the first cell is buried under the fills of cells 2..n. The table
    ///   therefore appends one hairline, *unclipped* trailing column whose only
    ///   job is to paint the row rule and the selection marker across the whole
    ///   row, after every other cell has had its say. `set_overline` is not an
    ///   option: it reads `widgets.noninteractive.bg_stroke`, which is also the
    ///   column-resize separator colour, so the two cannot be styled apart.
    pub(crate) fn show(
        self,
        ui: &mut egui::Ui,
        state: &mut DataTableState,
        row_count: usize,
        mut add_row: impl FnMut(&mut RowCtx<'_, '_, '_>),
    ) -> DataTableOutput {
        let Self {
            id_salt,
            columns,
            selectable,
            sort_key,
        } = self;

        let sort = state.sort;
        let selected = state.selected;
        let order = match sort_key.as_ref() {
            Some(key) => sort_order(row_count, sort, key),
            None => (0..row_count).collect(),
        };

        // From `spacing.interact_size.y`, which `theme::install` sets from
        // `Metrics::row_h` -- so every table follows the density switch without
        // being told about it.
        let row_h = ui.spacing().interact_size.y;
        let base_color = ui.visuals().text_color();
        // The row's left edge, captured before the `Ui` is borrowed by the
        // builder. The body lives inside a vertical-only `ScrollArea`, so this
        // x never moves and is valid for both the header and every row.
        let table_left = ui.available_rect_before_wrap().left();

        let mut header_clicked: Option<usize> = None;
        let mut clicked_row: Option<usize> = None;

        let mut builder = TableBuilder::new(ui)
            .id_salt(id_salt)
            // The design has no zebra striping: rows are separated by the rule
            // and the hover tint, nothing else.
            .striped(false)
            // Every table in the app wants draggable separators; there is no
            // setter to turn them off, because nothing has ever wanted to.
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(0.0);
        for column in &columns {
            builder = builder.column(column.to_extras(true));
        }
        // The trailing paint column. Unclipped, because a clipped column's clip
        // rect is exactly its own `max_rect` and everything this column draws is
        // outside it.
        builder = builder.column(Column::exact(PAINT_COL_W).clip(false).resizable(false));

        let table = builder.header(HEADER_H, |mut header| {
            for (ci, column) in columns.iter().enumerate() {
                let sorted = sorted_dir(sort, ci);
                let numeric = column.numeric;
                let label = column.label.as_str();
                let (_, resp) = header.col(|ui| {
                    let text = header_text(ui, label, sorted);
                    let add = |ui: &mut egui::Ui| {
                        ui.add(egui::Label::new(text).selectable(false));
                    };
                    if numeric {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
                    } else {
                        add(ui);
                    }
                });
                let resp = match column.tooltip.as_deref() {
                    Some(tip) => resp.on_hover_text(tip),
                    None => resp,
                };
                if column.sortable {
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        header_clicked = Some(ci);
                    }
                }
            }
            // The header's own hairline, painted from the trailing column so it
            // spans the full width rather than one cell.
            header.col(|ui| {
                let rect = ui.max_rect();
                let y = (rect.bottom() + 0.5 * ui.spacing().item_spacing.y).round_ui();
                ui.painter().hline(
                    Rangef::new(table_left, rect.right()),
                    y,
                    egui::Stroke::new(1.0, DIVIDER),
                );
            });
        });

        let n_cols = columns.len();
        table.body(|mut body| {
            {
                let visuals = body.ui_mut().visuals_mut();
                // The design's 4% hover tint. egui_extras resolves hover from
                // the *previous* frame's response, so the tint lands one frame
                // late; that is inherent to how the row response is captured
                // and is accepted rather than worked around.
                visuals.widgets.hovered.bg_fill = muted(HOVER_TINT);
                // Cells in a selected row get `override_text_color =
                // selection.stroke.color`. Leave it at the body colour or the
                // selected row's text silently turns into whatever stock egui
                // put there.
                visuals.selection.stroke.color = TEXT;
            }

            body.rows(row_h, order.len(), |mut row| {
                let data = order[row.index()];
                let is_selected = selectable && selected == Some(data);
                // Must happen before the first `col`: the flag is read per cell
                // as the cell is built, so a later call would only tint the
                // remaining cells.
                row.set_selected(is_selected);

                {
                    let mut ctx = RowCtx {
                        row: &mut row,
                        columns: &columns,
                        data,
                        color: base_color,
                        alpha: 1.0,
                    };
                    add_row(&mut ctx);
                }

                // Keep the paint column in the paint column even if the caller
                // filled fewer cells than it declared.
                while row.col_index() < n_cols {
                    row.col(|_| {});
                }

                row.col(|ui| {
                    let rect = ui.max_rect();
                    let spacing = ui.spacing().item_spacing.y;
                    let x = Rangef::new(table_left, rect.right());
                    // Above the row, not below it: the rule then sits on top of
                    // both neighbours' fills, and the top-most visible row's
                    // rule doubles as the line under the header.
                    let y = (rect.top() - 0.5 * spacing).round_ui();
                    faded_hline(ui.painter(), x, y, RULE_FADE, muted(RULE_TINT));
                    if is_selected {
                        let marker = egui::Rect::from_min_max(
                            egui::pos2(x.min + MARKER_INSET, rect.top()),
                            egui::pos2(x.min + MARKER_INSET + MARKER_W, rect.bottom()),
                        );
                        ui.painter().rect_filled(
                            marker.round_ui(),
                            egui::CornerRadius::same(1),
                            ui.visuals().hyperlink_color,
                        );
                    }
                });

                if row.response().clicked() {
                    clicked_row = Some(data);
                }
            });
        });

        // Applied only now: both closures borrowed the state they would change.
        if let Some(ci) = header_clicked {
            cycle_sort(&mut state.sort, ci);
        }
        if selectable {
            if let Some(data) = clicked_row {
                state.selected = Some(data);
            }
        }

        DataTableOutput {
            order,
            clicked: clicked_row,
        }
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// The cell API handed to a row closure.
///
/// Cells are added left to right; the column's own [`TableColumn::numeric`]
/// flag decides the alignment, so a call site never repeats it.
pub(crate) struct RowCtx<'a, 'b, 'c> {
    row: &'c mut egui_extras::TableRow<'a, 'b>,
    columns: &'c [TableColumn],
    data: usize,
    color: Color32,
    alpha: f32,
}

impl RowCtx<'_, '_, '_> {
    /// Index of this row in the caller's data, *before* sorting.
    pub(crate) fn data_index(&self) -> usize {
        self.data
    }

    /// Fade the whole row. Network uses this to dim a closed connection over a
    /// few seconds without removing it, so the eye can follow what left.
    ///
    /// Multiplies every colour the row paints, including ones the caller passes
    /// explicitly, so a fading row fades as a unit.
    pub(crate) fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    /// Override the row's base text colour.
    pub(crate) fn set_color(&mut self, color: Color32) {
        self.color = color;
    }

    /// A text cell in the row's colour.
    pub(crate) fn text(&mut self, text: impl Into<egui::RichText>) -> Response {
        let color = self.tinted(self.color);
        self.cell(move |ui| {
            ui.add(egui::Label::new(text.into().color(color)));
        })
    }

    /// A text cell led by an icon, in the row's colour.
    ///
    /// The icon cannot ride in the same string as the text -- it needs the icon
    /// family, and one `RichText` carries one family -- so the cell is laid out
    /// as two sections. `icon` is a [`crate::theme::icons`] constant.
    pub(crate) fn icon_text(&mut self, icon: &str, text: &str) -> Response {
        let color = self.tinted(self.color);
        let (icon, text) = (icon.to_owned(), text.to_owned());
        self.cell(move |ui| {
            let job = icons::labelled_styled(ui, &icon, &text, egui::TextStyle::Body, color);
            ui.add(egui::Label::new(job));
        })
    }

    /// A text cell in an explicit colour (risk, protocol state, ...), still
    /// subject to the row's alpha.
    pub(crate) fn colored(&mut self, text: impl Into<egui::RichText>, color: Color32) -> Response {
        let color = self.tinted(color);
        self.cell(move |ui| {
            ui.add(egui::Label::new(text.into().color(color)));
        })
    }

    /// A cell for a long value -- a path, a WQL query, a reason list.
    ///
    /// On a clipped column egui_extras already forces `TextWrapMode::Truncate`,
    /// and `Label` shows the full text on hover *only when it actually elided*,
    /// so this is ellipsis-plus-tooltip with no bookkeeping. Do not add
    /// `on_hover_text` on top unless the tooltip should say something the cell
    /// does not.
    pub(crate) fn path(&mut self, text: impl Into<egui::RichText>) -> Response {
        let color = self.tinted(self.color);
        self.cell(move |ui| {
            ui.add(egui::Label::new(text.into().color(color)).truncate());
        })
    }

    // A `tinted_cell` convenience lived here for the Compare view's changed
    // values. Compare builds those cells through `cell` and calls
    // `cell_background` itself, because it needs the tint and the text in
    // different colours; the wrapper was never used and is gone.

    /// An arbitrary cell. The column's alignment is applied around `add`.
    pub(crate) fn cell(&mut self, add: impl FnOnce(&mut egui::Ui)) -> Response {
        let numeric = self
            .columns
            .get(self.row.col_index())
            .is_some_and(|c| c.numeric);
        debug_assert!(
            self.row.col_index() < self.columns.len(),
            "row closure added more cells than the table declared columns"
        );
        let (_, resp) = self.row.col(|ui| {
            if numeric {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
            } else {
                add(ui);
            }
        });
        resp
    }

    fn tinted(&self, color: Color32) -> Color32 {
        if self.alpha >= 1.0 {
            color
        } else {
            color.gamma_multiply(self.alpha)
        }
    }
}

// ---------------------------------------------------------------------------
// Painting helpers
// ---------------------------------------------------------------------------

/// Paint a full-bleed background behind the cell being built. Call it as the
/// first statement of a cell closure.
///
/// The rect is the cell's `max_rect` grown by half the item spacing so the
/// tints of neighbouring cells meet instead of leaving a gap -- the same
/// `gapless_rect` egui_extras uses for its own stripe and selection fills.
///
/// **The column must be unclipped.** `Visuals::clip_rect_margin` defaults to
/// 0.0 in egui 0.35, so on a `.clip(true)` column egui_extras narrows the
/// cell's clip rect to exactly `max_rect`, and this rect -- which by
/// construction extends past it -- is discarded entirely rather than merely
/// trimmed.
pub(crate) fn cell_background(ui: &egui::Ui, color: Color32) {
    let rect = ui
        .max_rect()
        .expand2(0.5 * ui.spacing().item_spacing)
        .round_ui();
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, color);
}

/// A 1px horizontal rule that fades to nothing over `fade` points at each end.
///
/// Neither `Frame` nor `RectShape` can hold a gradient, so the rule is an
/// 8-vertex `Mesh`: three quads (transparent-to-solid, solid, solid-to-
/// transparent) sharing their inner vertices. Six triangles per visible row is
/// nothing next to the text they sit under.
///
/// The mesh must stay untextured -- `Mesh::colored_vertex` debug-asserts that
/// the mesh has no texture, so this can never be merged with a textured one.
///
/// (`widgets::rule` owns the general-purpose version of this; the copy here
/// keeps the table free of a cross-widget dependency while the kit is still
/// being built, and collapses into it during the reskin.)
fn faded_hline(painter: &egui::Painter, x: Rangef, y: f32, fade: f32, color: Color32) {
    if x.span() <= 0.0 {
        return;
    }
    let fade = fade.clamp(0.0, x.span() * 0.5);
    let clear = color.gamma_multiply(0.0);

    let mut mesh = egui::epaint::Mesh::default();
    let stops = [
        (x.min, clear),
        (x.min + fade, color),
        (x.max - fade, color),
        (x.max, clear),
    ];
    for (x, color) in stops {
        mesh.colored_vertex(egui::pos2(x, y), color);
        mesh.colored_vertex(egui::pos2(x, y + 1.0), color);
    }
    for quad in 0..3u32 {
        let i = quad * 2;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// The sort direction for `col`, or `None` when some other column is active.
fn sorted_dir(sort: Sort, col: usize) -> Option<bool> {
    match sort {
        Some((c, ascending)) if c == col => Some(ascending),
        _ => None,
    }
}

/// The design's header treatment: 11px, uppercase, letter-spaced, muted until
/// it is the active sort column, with a caret for the direction.
///
/// The caret *trails* its title, which is the one place in the app where
/// `icons::labelled` -- which leads with the icon -- does not fit. The two
/// halves are therefore appended to a `LayoutJob` in order; they need separate
/// sections regardless, since only the caret is in the icon family.
fn header_text(ui: &egui::Ui, title: &str, sorted: Option<bool>) -> egui::WidgetText {
    // `TextStyle::resolve` panics on a missing key, and a widget has no
    // business panicking because the theme was not installed.
    let font = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Name("th".into()))
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(11.0));

    let color = if sorted.is_some() {
        // The live accent. `Visuals` has no accent field; `theme::apply_accent`
        // puts a500 in `hyperlink_color`, so that is where a widget reads it.
        ui.visuals().hyperlink_color
    } else {
        muted(HEADER_TINT)
    };

    let Some(ascending) = sorted else {
        return egui::RichText::new(title.to_uppercase())
            .font(font)
            .extra_letter_spacing(0.5)
            .color(color)
            .into();
    };

    let caret = if ascending {
        icons::CARET_UP
    } else {
        icons::CARET_DOWN
    };
    let mut job = egui::text::LayoutJob::default();
    // The separating space belongs to the title's section: it is text, and the
    // icon font's own space is not the one this header was measured against.
    egui::RichText::new(format!("{} ", title.to_uppercase()))
        .font(font.clone())
        .extra_letter_spacing(0.5)
        .color(color)
        .append_to(
            &mut job,
            ui.style(),
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    icons::glyph(caret).size(font.size).color(color).append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    job.into()
}

// `sortable_header` -- a standalone header cell for a hand-rolled
// `TableBuilder` -- lived here until task 1.31 moved the last of the four
// pre-Nocturne tables onto `DataTable`. Its own doc said it went away with
// them; it then survived two more phases only because this module carried a
// blanket `allow(dead_code)`. Deleted with that allow, which is what would have
// caught it.

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

/// Colour a measured number by how alarming it is: [`OK`] below `warn`, [`WARN`]
/// from there, [`BAD`] from `bad`.
///
/// Thresholds are the caller's, because "high" for a working set is not "high"
/// for a handle count. `bad` is checked first so an inverted pair (bad < warn)
/// still reports the worse of the two rather than silently going quiet.
pub(crate) fn numeric_threshold_color(value: f64, warn: f64, bad: f64) -> Color32 {
    if value >= bad {
        BAD
    } else if value >= warn {
        WARN
    } else {
        OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys<'a>(rows: &'a [&'a str]) -> impl Fn(usize, usize) -> String + 'a {
        move |row, _col| rows[row].to_string()
    }

    /// The order vector must be a permutation of `0..len` no matter the sort:
    /// a duplicated or dropped index renders the wrong row, or panics on the
    /// index into the caller's data.
    #[test]
    fn order_is_always_a_permutation() {
        let rows = ["b", "a", "c", "a"];
        for sort in [None, Some((0, true)), Some((0, false)), Some((7, true))] {
            let order = sort_order(rows.len(), sort, keys(&rows));
            let mut seen = order.clone();
            seen.sort_unstable();
            assert_eq!(seen, vec![0, 1, 2, 3], "{sort:?} is not a permutation");
        }
    }

    /// Unsorted must mean "the caller's order", untouched.
    #[test]
    fn no_sort_is_the_natural_order() {
        let rows = ["b", "a", "c"];
        assert_eq!(sort_order(3, None, keys(&rows)), vec![0, 1, 2]);
    }

    /// Ascending and descending must be exact mirrors, and ties must keep the
    /// caller's order -- Network relies on that so a fading row does not jump
    /// while it dims.
    #[test]
    fn sort_is_directional_and_stable() {
        let rows = ["b", "a", "c", "a"];
        assert_eq!(
            sort_order(4, Some((0, true)), keys(&rows)),
            vec![1, 3, 0, 2],
            "ascending, ties in original order"
        );
        assert_eq!(
            sort_order(4, Some((0, false)), keys(&rows)),
            vec![2, 0, 1, 3],
            "descending, ties still in original order"
        );
    }

    /// The keys go through `smart_cmp`, so a numeric column must not sort
    /// "10" before "9".
    #[test]
    fn numeric_keys_sort_numerically() {
        let rows = ["9", "10", "100", "1"];
        assert_eq!(
            sort_order(4, Some((0, true)), keys(&rows)),
            vec![3, 0, 1, 2]
        );
    }

    /// An empty table must not panic or produce a phantom row.
    #[test]
    fn empty_table_sorts_to_nothing() {
        let rows: [&str; 0] = [];
        assert!(sort_order(0, Some((0, true)), keys(&rows)).is_empty());
    }

    /// The header contract: three clicks on one column return it to unsorted.
    #[test]
    fn sort_cycles_through_three_states() {
        let mut sort: Sort = None;
        cycle_sort(&mut sort, 2);
        assert_eq!(sort, Some((2, true)), "first click ascends");
        cycle_sort(&mut sort, 2);
        assert_eq!(sort, Some((2, false)), "second click descends");
        cycle_sort(&mut sort, 2);
        assert_eq!(sort, None, "third click clears");
    }

    /// Clicking a different column starts that column ascending, whatever the
    /// previous column was doing.
    #[test]
    fn sort_moves_to_a_new_column_ascending() {
        let mut sort: Sort = Some((2, false));
        cycle_sort(&mut sort, 5);
        assert_eq!(sort, Some((5, true)));
    }

    /// `bad` wins over `warn` even if the caller passes them the wrong way
    /// round.
    #[test]
    fn thresholds_report_the_worse_of_the_two() {
        assert_eq!(numeric_threshold_color(1.0, 10.0, 20.0), OK);
        assert_eq!(numeric_threshold_color(10.0, 10.0, 20.0), WARN);
        assert_eq!(numeric_threshold_color(20.0, 10.0, 20.0), BAD);
        assert_eq!(numeric_threshold_color(15.0, 20.0, 10.0), BAD);
    }

    /// `Column::exact` pins its own width range; re-applying `at_least` would
    /// widen it and quietly make a fixed column resizable.
    #[test]
    fn exact_columns_stay_exact() {
        let pinned = TableColumn::exact("PID", 64.0).at_least(200.0);
        assert_eq!(pinned.to_extras(true), Column::exact(64.0).resizable(false));
    }
}
