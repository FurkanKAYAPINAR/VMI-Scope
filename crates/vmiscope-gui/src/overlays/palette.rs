//! The Ctrl+K command palette.
//!
//! One box that reaches everything: all eleven destinations in the rail, the
//! actions the title bar carries, the accent and density switches -- and the
//! same class / property / method index the Explorer's sidebar searches.
//!
//! The search half is deliberately not a second implementation. It calls
//! [`VmiScopeApp::compute_hits`] and [`VmiScopeApp::apply_search_hit`]
//! unchanged, so a hit found here behaves exactly as the same hit found in the
//! sidebar -- including the method case, which opens the Actions panel. The
//! palette is a second front door onto one index, not a second index.
//!
//! Three things about the input handling are load-bearing and invisible in a
//! diff. Each is commented again at its call site, because getting any of them
//! wrong still compiles and still renders:
//!
//! 1. The navigation keys come off the event queue **before** the `TextEdit` is
//!    added. A focused `TextEdit` publishes `vertical_arrows: true` in its
//!    `EventFilter`, which makes Up and Down its exclusive property -- they
//!    would walk the caret instead of the selection. Consuming them first
//!    deletes the events, so the field never sees them at all.
//! 2. Enter goes the same way: a single-line `TextEdit` treats it as its
//!    `return_key` and surrenders focus, which would leave the palette open
//!    over a dead input.
//! 3. Autofocus alone does not give a select-all. `TextEdit::show` hands back a
//!    *copy* of the widget's state, so setting a cursor range on it and not
//!    calling `store` throws the selection away and the caret just lands at the
//!    end of the text.

use eframe::egui;
use eframe::egui::text::{CCursor, CCursorRange};
use eframe::egui::{
    Align, Align2, Frame, Id, Key, Label, Layout, Margin, Modifiers, Response, RichText, Sense,
    Stroke, TextStyle, Ui, UiBuilder, Vec2,
};

use crate::app::{CentralView, VmiScopeApp};
use crate::theme::tokens::{muted, DIVIDER, R_LG, R_MD, S1, S2, SURFACE, TEXT};
use crate::theme::{icons, Accent, Density, Theme};
use crate::util::save_file;
use crate::views::nav::View;
use crate::widgets::button::accent;
use crate::widgets::rule::{hrule, HAIRLINE};

use vmiscope_core::SearchHit;

/// The palette's `Area` id, and the seed for the input's own id. Both have to
/// be stable across frames: the modal keeps its anchor under the first, and the
/// stored cursor range is filed under the second.
const PALETTE_ID: &str = "vs_palette";
const INPUT_ID: &str = "vs_palette_input";

/// Fixed width. A palette that grows with the window stops being a palette --
/// at 1900px it reads as a page, and the eye has to travel the whole span to
/// pair a label with its hint.
const WIDTH: f32 = 560.0;

/// How far below the top edge the box hangs. The design anchors it near the top
/// rather than dead centre so the results grow downward into empty space
/// instead of pushing the input around.
const TOP: f32 = 120.0;

/// The frame's inner padding. `Margin` is `i8`.
const PAD: i8 = 8;

/// Vertical padding inside the (frameless) query field.
const INPUT_PAD: i8 = 4;

/// Height of one result row.
const ROW_H: f32 = 30.0;

/// The row icon.
const ICON: f32 = 14.0;

/// Horizontal breathing room inside a row, so an icon never touches the pill.
const ROW_PAD: f32 = 6.0;

/// How far the selection pill sits inside its row, vertically.
const PILL_INSET: f32 = 1.0;

/// The selected row's pill: 15% of the live accent, the same figure the rail
/// uses for the selected destination, so "this is where you are" reads the same
/// in both places.
const SEL_TINT: f32 = 0.15;

/// The hover tint on an unselected row.
const HOVER_TINT: f32 = 0.06;

/// Ceiling on the results list. Past this it scrolls; the highlighted row is
/// kept in view by `scroll_to_me`.
const LIST_H: f32 = 360.0;

/// Shortest query that searches the index. Below two characters practically
/// every class in `root\CIMV2` matches and the list is noise -- the same
/// threshold the Explorer's sidebar search uses.
const MIN_QUERY: usize = 2;

/// How many hits any one search group contributes. The palette is a jump list,
/// not a results table: eleven hundred matching properties are a sign to type
/// another character, not something to scroll through.
const MAX_PER_GROUP: usize = 8;

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Everything the palette can do that is not "go to a search hit".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Command {
    /// Navigate to a destination. Placeholder destinations included: the rail
    /// already lists them, and each shows its own empty state rather than a
    /// blank pane, so a palette that hid them would be the one surface
    /// pretending the roadmap does not exist.
    Go(View),
    Refresh,
    RunQuery,
    ToggleLive,
    Export,
    SetAccent(Accent),
    SetDensity(Density),
}

impl Command {
    /// Every command, in the order the palette offers them.
    fn all() -> Vec<Self> {
        let mut all: Vec<Self> = View::ALL.into_iter().map(Self::Go).collect();
        all.extend([
            Self::Refresh,
            Self::RunQuery,
            Self::ToggleLive,
            Self::Export,
        ]);
        all.extend(Accent::ALL.into_iter().map(Self::SetAccent));
        all.extend(Density::ALL.into_iter().map(Self::SetDensity));
        all
    }

    /// The command's name, phrased as the thing it does.
    fn label(self) -> String {
        match self {
            Self::Go(view) => format!("Go to {}", view.title()),
            Self::Refresh => "Refresh".to_string(),
            Self::RunQuery => "Run query".to_string(),
            Self::ToggleLive => "Toggle live".to_string(),
            Self::Export => "Export results".to_string(),
            Self::SetAccent(accent) => format!("Accent: {}", accent.label()),
            Self::SetDensity(density) => format!("Density: {}", density.label()),
        }
    }

    /// One line of what it is for. Also searched, so "socket" can find Network.
    fn hint(self) -> &'static str {
        match self {
            Self::Go(view) => view.hint(),
            Self::Refresh => "Re-fetch the active view (F5)",
            Self::RunQuery => "Run the current WQL on the Explorer (Ctrl+Enter)",
            Self::ToggleLive => "Pause or resume the live pollers",
            Self::Export => "Save the current result set as CSV",
            Self::SetAccent(_) => "Switch the accent colour",
            Self::SetDensity(_) => "Switch row height and spacing",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Go(view) => view.icon(),
            Self::Refresh => icons::ARROWS_CLOCKWISE,
            Self::RunQuery => icons::PLAY,
            Self::ToggleLive => icons::PULSE,
            Self::Export => icons::DOWNLOAD_SIMPLE,
            Self::SetAccent(_) => icons::PALETTE,
            Self::SetDensity(_) => icons::ARROWS_IN_LINE_VERTICAL,
        }
    }
}

/// Rank a command against a lowercased query. `None` means it does not match.
///
/// Lower is better. The ordering is the one a palette user expects: what you
/// typed at the front of the name beats it in the middle, which beats a match
/// that only exists in the description. Split out from the drawing so it can be
/// tested without a `Ui`.
fn score(label: &str, hint: &str, q: &str) -> Option<u32> {
    if q.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let hint = hint.to_lowercase();
    if label.starts_with(q) {
        Some(0)
    } else if label.split_whitespace().any(|word| word.starts_with(q)) {
        Some(1)
    } else if label.contains(q) {
        Some(2)
    } else if hint.contains(q) {
        Some(3)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// The four kinds of thing the palette lists, in the order it lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Class,
    Property,
    Method,
    Command,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Class => "Class",
            Self::Property => "Property",
            Self::Method => "Method",
            Self::Command => "Command",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Class => icons::CUBE,
            Self::Property => icons::LIST_BULLETS,
            Self::Method => icons::FUNCTION,
            Self::Command => icons::LIGHTNING,
        }
    }

    /// Which group a search hit belongs to.
    fn of(hit: &SearchHit) -> Self {
        match &hit.member {
            None => Self::Class,
            Some(_) if hit.is_method => Self::Method,
            Some(_) => Self::Property,
        }
    }
}

/// What running a row does.
#[derive(Clone)]
enum Action {
    Hit(SearchHit),
    Run(Command),
}

/// One drawn row. The list is rebuilt every frame from the query, and the
/// keyboard selection is an index into it, so nothing can point at a row that
/// is no longer shown.
struct Row {
    kind: Kind,
    icon: &'static str,
    label: String,
    hint: &'static str,
    action: Action,
}

// ---------------------------------------------------------------------------
// The overlay
// ---------------------------------------------------------------------------

impl VmiScopeApp {
    /// Draw the palette, if it is open.
    pub(crate) fn ui_palette(&mut self, ui: &mut Ui, now: f64) {
        if !self.palette_open {
            // Re-arm the autofocus for the next open.
            self.palette_shown = false;
            return;
        }
        let just_opened = !std::mem::replace(&mut self.palette_shown, true);

        let id = Id::new(PALETTE_ID);
        let modal = egui::Modal::new(id)
            // `Modal::default_area` anchors CENTER_CENTER; a palette belongs
            // near the top, so the results grow into empty space rather than
            // shifting the input down the screen as they arrive.
            .area(palette_area(id))
            .frame(palette_frame())
            .show(ui.ctx(), |ui| self.palette_body(ui, just_opened, now));

        if modal.inner || modal.should_close() {
            self.palette_open = false;
        }
    }

    /// The palette's contents. Returns true when it asked to be closed.
    fn palette_body(&mut self, ui: &mut Ui, just_opened: bool, now: f64) -> bool {
        ui.set_width(WIDTH);

        // TRAP 1 and 2, and they have to be sprung here -- before the field is
        // added, not after. Up and Down are exclusive to a focused `TextEdit`
        // (its `EventFilter` sets `vertical_arrows`), and Enter is its
        // `return_key`; taking all four off the queue now is what stops the
        // caret from moving and the field from surrendering focus.
        //
        // Escape is taken here too rather than left to `ModalResponse::
        // should_close`, which reads it from the same queue after the content
        // has run: whoever consumes it first wins, and the palette has to be
        // the one that does or the Escape that closes it also reaches the view
        // underneath.
        let (down, up, run, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });

        let changed = self.palette_input(ui, just_opened);
        hrule(ui);

        let rows = self.palette_rows();

        // Keep the selection on a row that exists. The list is rebuilt from the
        // query every frame, so an index kept across a keystroke is otherwise
        // free to point past the end.
        if rows.is_empty() {
            self.palette_sel = 0;
        } else {
            if changed {
                self.palette_sel = 0;
            }
            self.palette_sel = self.palette_sel.min(rows.len() - 1);
            if down {
                self.palette_sel = (self.palette_sel + 1) % rows.len();
            }
            if up {
                self.palette_sel = (self.palette_sel + rows.len() - 1) % rows.len();
            }
        }

        let moved = down || up;
        let selected = self.palette_sel;
        let mut activated = None;

        egui::ScrollArea::vertical()
            .id_salt("vs-palette-rows")
            .max_height(LIST_H)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.add_space(S2);
                    ui.add(
                        Label::new(RichText::new("No matches").color(muted(45))).selectable(false),
                    );
                    return;
                }
                let mut heading: Option<Kind> = None;
                for (index, row) in rows.iter().enumerate() {
                    if heading != Some(row.kind) {
                        group_heading(ui, row.kind);
                        heading = Some(row.kind);
                    }
                    let is_selected = index == selected;
                    let response = draw_row(ui, row, is_selected);
                    if response.clicked() {
                        activated = Some(index);
                    }
                    // Only on a keyboard move: calling this every frame would
                    // fight the mouse wheel, snapping the list back the moment
                    // the user scrolled it.
                    if is_selected && moved {
                        response.scroll_to_me(None);
                    }
                }
            });

        footer(ui, rows.len());

        // A click names its own row, so it wins over Enter in the frame where
        // both land -- otherwise a click on row 9 would run row 1.
        if activated.is_none() && run && !rows.is_empty() {
            activated = Some(selected);
        }
        if let Some(row) = activated.and_then(|i| rows.get(i)) {
            match row.action.clone() {
                Action::Hit(hit) => {
                    // `apply_search_hit` selects a class, runs a query and may
                    // open the Actions panel -- all of which live on the
                    // Explorer, and the palette can be opened from anywhere.
                    self.view = View::Explorer;
                    self.apply_search_hit(hit);
                }
                Action::Run(command) => self.run_command(ui, command, now),
            }
            return true;
        }
        escape
    }

    /// The query field. Returns true when the text changed this frame.
    fn palette_input(&mut self, ui: &mut Ui, just_opened: bool) -> bool {
        let out = egui::TextEdit::singleline(&mut self.palette_query)
            .id(Id::new(INPUT_ID))
            // Monospace for the same reason every other field in the kit is:
            // most of what gets typed here is a class or property name, and
            // proportional digits make two similar identifiers hard to tell
            // apart.
            .font(TextStyle::Monospace)
            .prefix(icons::glyph(icons::MAGNIFYING_GLASS).color(muted(45)))
            .hint_text(RichText::new("Search classes, properties, commands").color(muted(38)))
            .desired_width(f32::INFINITY)
            // No frame, and no focus ring: this is the modal's only focus stop,
            // so a box inside a box and a ring around the only thing that can
            // have focus would both be saying nothing. `TextEdit::frame` takes
            // a `Frame` in 0.35 rather than the older `bool`, and passing one
            // suppresses the default background AND its inner margin -- hence
            // the explicit vertical padding, or the field sits flush against
            // the rule underneath it.
            .frame(Frame::NONE.inner_margin(Margin::symmetric(0, INPUT_PAD)))
            .show(ui);

        let changed = out.response.changed();

        if just_opened {
            // TRAP 3. `request_focus` alone lands the caret at the end of
            // whatever was typed last time. Selecting it all is what makes the
            // previous query a starting point you can either refine or type
            // straight over -- and the selection only survives because the
            // state is stored back: `show` handed us a copy.
            let id = out.response.id;
            out.response.request_focus();
            let end = CCursor::new(self.palette_query.chars().count());
            let mut state = out.state;
            state
                .cursor
                .set_char_range(Some(CCursorRange::two(CCursor::new(0), end)));
            state.store(ui.ctx(), id);
        }

        changed
    }

    /// Everything the current query matches, grouped and in group order.
    fn palette_rows(&self) -> Vec<Row> {
        let query = self.palette_query.trim().to_lowercase();
        let mut rows = Vec::new();

        // Class / Property / Method, straight off the Explorer's index.
        if query.len() >= MIN_QUERY {
            let hits = self.compute_hits(&query);
            for kind in [Kind::Class, Kind::Property, Kind::Method] {
                rows.extend(
                    hits.iter()
                        .filter(|hit| Kind::of(hit) == kind)
                        .take(MAX_PER_GROUP)
                        .map(|hit| Row {
                            kind,
                            icon: kind.icon(),
                            label: match &hit.member {
                                None => hit.class.clone(),
                                Some(m) if hit.is_method => format!("{} :: {m}()", hit.class),
                                Some(m) => format!("{} :: {m}", hit.class),
                            },
                            hint: "",
                            action: Action::Hit(hit.clone()),
                        }),
                );
            }
        }

        // Commands. Ranked, but `sort_by_key` is stable, so equally good
        // matches keep the canonical order of `Command::all`.
        let mut scored: Vec<(u32, Command)> = Command::all()
            .into_iter()
            .filter_map(|c| score(&c.label(), c.hint(), &query).map(|s| (s, c)))
            .collect();
        scored.sort_by_key(|(s, _)| *s);
        rows.extend(scored.into_iter().map(|(_, command)| Row {
            kind: Kind::Command,
            icon: command.icon(),
            label: command.label(),
            hint: command.hint(),
            action: Action::Run(command),
        }));

        rows
    }

    /// Run one command.
    ///
    /// Takes a `Ui` rather than a `Context` only because the two theme commands
    /// re-install the style through it; nothing here draws.
    pub(crate) fn run_command(&mut self, ui: &Ui, command: Command, now: f64) {
        match command {
            Command::Go(view) => self.view = view,
            Command::Refresh => self.refresh_active_view(now),
            Command::RunQuery => {
                // The result table only exists on the Explorer, so running a
                // query from anywhere else has to take you where you can see
                // it; otherwise the command silently does nothing visible.
                self.view = View::Explorer;
                self.central_view = CentralView::Instances;
                self.run_query();
            }
            Command::ToggleLive => self.net_paused = !self.net_paused,
            Command::Export => self.export_result(),
            Command::SetAccent(accent) => self.set_theme(
                ui,
                Theme {
                    accent,
                    density: self.config.density,
                },
            ),
            Command::SetDensity(density) => self.set_theme(
                ui,
                Theme {
                    accent: self.config.accent,
                    density,
                },
            ),
        }
    }

    /// Install a theme and persist it.
    ///
    /// `Config` is the single source of truth for the accent and the density --
    /// `app::new` installs from it at boot, and the Settings view writes the
    /// same two fields -- so the palette writes there too. Installing into the
    /// live style alone would leave Settings showing one thing, the window
    /// rendering another, and a restart quietly undoing the switch.
    fn set_theme(&mut self, ui: &Ui, theme: Theme) {
        self.config.accent = theme.accent;
        self.config.density = theme.density;
        crate::theme::install(ui.ctx(), theme);
        self.config.save();
    }

    /// Save the current result set. Same CSV the Explorer's own button writes.
    fn export_result(&mut self) {
        match self.result.as_ref().filter(|r| !r.rows.is_empty()) {
            Some(result) => save_file("query.csv", &vmiscope_core::export::query_to_csv(result)),
            // Silence would read as a broken command; the status bar's error
            // line is where every other "that did not happen" lands.
            None => self.push_error("Export: no query results to write.".to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// The palette's area: the modal default, re-anchored to the top.
fn palette_area(id: Id) -> egui::Area {
    egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP))
}

/// The box itself.
///
/// Built from tokens rather than left as `Frame::popup`, which takes the
/// tighter `menu_margin` and the menu radius. No shadow: the modal already
/// dims everything behind it, and a drop shadow under a scrim is a second
/// answer to a question that has been answered.
fn palette_frame() -> Frame {
    Frame::NONE
        .fill(SURFACE)
        .stroke(Stroke::new(HAIRLINE, DIVIDER))
        .corner_radius(R_LG)
        .inner_margin(Margin::same(PAD))
}

/// The `Class` / `Property` / `Method` / `Command` label above a group.
fn group_heading(ui: &mut Ui, kind: Kind) {
    ui.add_space(S2);
    ui.add(
        Label::new(icons::labelled_styled(
            ui,
            kind.icon(),
            kind.label(),
            TextStyle::Name("th".into()),
            muted(40),
        ))
        .selectable(false),
    );
    ui.add_space(S1);
}

/// One row: icon, label, and its hint pushed to the right.
fn draw_row(ui: &mut Ui, row: &Row, selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());

    let a = accent(ui);
    let pill = rect.shrink2(Vec2::new(0.0, PILL_INSET));
    if selected {
        ui.painter()
            .rect_filled(pill, R_MD, a.gamma_multiply(SEL_TINT));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(pill, R_MD, TEXT.gamma_multiply(HOVER_TINT));
    }

    let fg = if selected { a } else { muted(60) };
    // A child Ui rather than `ui.put`: the rect is already allocated, and `put`
    // would advance the parent's cursor over it a second time.
    let mut inner = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(ROW_PAD, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    inner.add(Label::new(icons::glyph(row.icon).size(ICON).color(fg)).selectable(false));
    inner.add(
        Label::new(RichText::new(&row.label).color(if selected { TEXT } else { muted(85) }))
            .selectable(false)
            .truncate(),
    );
    if !row.hint.is_empty() {
        inner.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                Label::new(
                    RichText::new(row.hint)
                        .text_style(TextStyle::Small)
                        .color(muted(35)),
                )
                .selectable(false)
                .truncate(),
            );
        });
    }
    drop(inner);

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// The count on the left, the keys on the right.
fn footer(ui: &mut Ui, count: usize) {
    hrule(ui);
    ui.horizontal(|ui| {
        let small = |text: String| {
            Label::new(
                RichText::new(text)
                    .text_style(TextStyle::Small)
                    .color(muted(35)),
            )
            .selectable(false)
        };
        ui.add(small(match count {
            1 => "1 result".to_string(),
            n => format!("{n} results"),
        }));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(small(
                "Up / Down to move \u{00b7} Enter to run \u{00b7} Esc to close".to_string(),
            ));
        });
    });
}

// ---------------------------------------------------------------------------
// Global shortcuts
//
// They live here rather than in `app.rs` because the palette is what most of
// them are for: Ctrl+K opens it, Escape closes it, and Ctrl+Enter runs the same
// `Command::RunQuery` one of its rows does, so there is one definition of what
// "run the query" means rather than two that drift.
// ---------------------------------------------------------------------------

/// Open the palette, or dismiss it if it is already up.
const PALETTE_KEY: egui::KeyboardShortcut = egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
/// Re-fetch whatever the active view is showing.
const REFRESH_KEY: egui::KeyboardShortcut = egui::KeyboardShortcut::new(Modifiers::NONE, Key::F5);
/// Run the current WQL.
const RUN_KEY: egui::KeyboardShortcut = egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
/// Close the frontmost overlay.
const CLOSE_KEY: egui::KeyboardShortcut = egui::KeyboardShortcut::new(Modifiers::NONE, Key::Escape);

impl VmiScopeApp {
    /// The application's global keys.
    ///
    /// Called once, before the shell, so a shortcut is decided before any view
    /// has had a chance to read the same keystroke.
    ///
    /// Two rules run this function, and both are easy to get wrong in ways that
    /// only show up as "the app ate my keystroke":
    ///
    /// * **Most specific first.** `consume_shortcut` matches modifiers with
    ///   `Modifiers::matches_logically`, which ignores *extra* Shift and Alt.
    ///   Ctrl and Command are matched exactly, so Ctrl+Enter can never be
    ///   swallowed by a bare-Enter binding -- but the modified pairs are still
    ///   checked ahead of the bare ones, because the moment a Shift variant of
    ///   any of these is added the other order silently starts eating it.
    /// * **A focused text field owns the keyboard.** Everything except Ctrl+K
    ///   and F5 stands down while one has focus, or typing a WQL string with an
    ///   Escape in it closes the app's dialogs and F5 fires mid-word.
    ///
    /// The focus test is [`egui::Context::text_edit_focused`] rather than a
    /// bare `memory().focused().is_some()`: egui gives keyboard focus to
    /// buttons on click too, so the broader test would leave Escape dead for as
    /// long as the last thing clicked was a button.
    pub(crate) fn handle_shortcuts(&mut self, ui: &Ui, now: f64) {
        let ctx = ui.ctx();

        // The two that fire mid-word: the palette has to be reachable from
        // inside any field (that is the whole point of a palette), and F5 is
        // the one key every Windows user expects to mean "reload" everywhere.
        if ctx.input_mut(|i| i.consume_shortcut(&PALETTE_KEY)) {
            self.palette_open = !self.palette_open;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&REFRESH_KEY)) {
            self.refresh_active_view(now);
        }

        if ctx.text_edit_focused() {
            return;
        }

        // The palette is modal, so it owns Enter while it is up -- and it takes
        // Escape off the queue itself, inside its own body, for the same
        // reason. Guarding here keeps a query from running behind it when the
        // pointer has taken focus off its input.
        if !self.palette_open && ctx.input_mut(|i| i.consume_shortcut(&RUN_KEY)) {
            self.run_command(ui, Command::RunQuery, now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&CLOSE_KEY)) {
            self.close_topmost_overlay();
        }
    }

    /// Close the overlay nearest the front.
    ///
    /// The order is the stacking order, front to back: the palette is a
    /// foreground modal, the invoke gate is the modal behind it, and the error
    /// log is the one you leave open while you work. Escape must close exactly
    /// one of them -- closing the lot would be a single keystroke undoing several
    /// decisions.
    fn close_topmost_overlay(&mut self) {
        for open in [
            &mut self.palette_open,
            &mut self.invoke_open,
            &mut self.save_query_open,
            &mut self.mof_open,
            &mut self.error_log_open,
        ] {
            if *open {
                *open = false;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette is the keyboard route to everywhere. A destination missing
    /// from the command list is one that can only be reached with the mouse --
    /// which is invisible unless someone goes looking for it.
    #[test]
    fn every_destination_is_a_command() {
        let all = Command::all();
        for view in View::ALL {
            assert!(
                all.contains(&Command::Go(view)),
                "{view:?} is not reachable from the palette"
            );
        }
    }

    /// The actions the title bar carries, plus both theme switches. Task 2.20
    /// asks for at least eighteen commands; eleven destinations and these seven
    /// make twenty.
    #[test]
    fn the_actions_and_the_theme_switches_are_commands() {
        let all = Command::all();
        for command in [
            Command::Refresh,
            Command::RunQuery,
            Command::ToggleLive,
            Command::Export,
        ] {
            assert!(all.contains(&command), "{command:?} is missing");
        }
        for accent in Accent::ALL {
            assert!(all.contains(&Command::SetAccent(accent)), "{accent:?}");
        }
        for density in Density::ALL {
            assert!(all.contains(&Command::SetDensity(density)), "{density:?}");
        }
        assert!(all.len() >= 18, "only {} commands", all.len());
    }

    /// Two commands with one name is a palette where the highlighted row and
    /// the row you meant are not the same row.
    #[test]
    fn command_labels_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for command in Command::all() {
            assert!(
                seen.insert(command.label()),
                "{:?} duplicates a label",
                command
            );
        }
    }

    /// The ranking, in the order a palette user expects it.
    #[test]
    fn a_label_prefix_outranks_a_label_hit_outranks_a_hint_hit() {
        let prefix = score("Go to Network", "Live TCP and UDP connections", "go").unwrap();
        let word = score("Go to Network", "Live TCP and UDP connections", "net").unwrap();
        let inside = score("Go to Network", "Live TCP and UDP connections", "etwo").unwrap();
        let hint = score("Go to Network", "Live TCP and UDP connections", "udp").unwrap();
        assert!(prefix < word, "{prefix} !< {word}");
        assert!(word < inside, "{word} !< {inside}");
        assert!(inside < hint, "{inside} !< {hint}");
    }

    /// An empty query lists everything, in the canonical order.
    #[test]
    fn an_empty_query_matches_every_command_equally() {
        for command in Command::all() {
            assert_eq!(score(&command.label(), command.hint(), ""), Some(0));
        }
    }

    /// A query that is in neither the name nor the description must not match,
    /// or the palette answers every keystroke with the whole command list.
    #[test]
    fn an_unrelated_query_matches_nothing() {
        assert_eq!(
            score("Refresh", "Re-fetch the active view (F5)", "zzz"),
            None
        );
    }

    /// Matching is case-insensitive on the query's side only -- the caller
    /// lowercases it -- so a capitalised label still has to be found.
    #[test]
    fn matching_ignores_the_labels_case() {
        assert_eq!(score("Export results", "Save as CSV", "export"), Some(0));
    }

    /// The bindings, stated once. Distinct keys are what makes the check order
    /// in `handle_shortcuts` a stylistic choice rather than a correctness one.
    #[test]
    fn the_bindings_are_distinct_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for binding in [PALETTE_KEY, RUN_KEY, REFRESH_KEY, CLOSE_KEY] {
            assert!(seen.insert(binding.logical_key), "{binding:?} shares a key");
        }
        assert_eq!(
            PALETTE_KEY,
            egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::K)
        );
        assert_eq!(
            RUN_KEY,
            egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter)
        );
        assert_eq!(
            REFRESH_KEY,
            egui::KeyboardShortcut::new(Modifiers::NONE, Key::F5)
        );
        assert_eq!(
            CLOSE_KEY,
            egui::KeyboardShortcut::new(Modifiers::NONE, Key::Escape)
        );
    }

    /// The palette takes bare Enter off the queue to run its highlighted row.
    /// That must never be able to swallow Ctrl+Enter, or "run query" would
    /// stop working the moment the palette was open behind a focused field.
    #[test]
    fn ctrl_enter_does_not_match_the_palettes_bare_enter() {
        assert!(!RUN_KEY.modifiers.matches_logically(Modifiers::NONE));
        assert!(Modifiers::NONE.matches_logically(Modifiers::NONE));
    }

    /// Every group must be able to name and draw itself; a hit that landed in
    /// no group would simply not be listed.
    #[test]
    fn every_hit_shape_has_a_group() {
        let class = SearchHit {
            class: "Win32_Process".into(),
            member: None,
            is_method: false,
        };
        let property = SearchHit {
            member: Some("Name".into()),
            ..class.clone()
        };
        let method = SearchHit {
            member: Some("Create".into()),
            is_method: true,
            ..class.clone()
        };
        assert_eq!(Kind::of(&class), Kind::Class);
        assert_eq!(Kind::of(&property), Kind::Property);
        assert_eq!(Kind::of(&method), Kind::Method);
    }
}
