//! The 40px title bar: identity on the left, the palette trigger in the
//! middle, live/refresh and the window buttons on the right.

use eframe::egui;
use eframe::egui::{
    Align, Color32, CornerRadius, CursorIcon, Frame, Label, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2, ViewportCommand,
};

use crate::app::{ConnStatus, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, DIVIDER, NEUTRAL, OK, R_SM, S2, S3, TEXT, WARN};
use crate::views::nav::View;
use crate::widgets::button::{accent, btn_ghost};
use crate::widgets::rule::{solid_hline, solid_vline, HAIRLINE};

use super::{chrome_fill, TITLEBAR_H};

/// The app glyph's outlined box.
const GLYPH_BOX: f32 = 22.0;
/// The glyph inside it. Deliberately smaller than the box: Phosphor draws to a
/// full em, so a 22px glyph in a 22px box would touch the outline on all sides.
const GLYPH_SIZE: f32 = 13.0;

/// Window buttons: 38 wide, the full bar tall, so the pointer can slam into the
/// top-right corner and still hit close.
const WIN_BTN_W: f32 = 38.0;

/// The palette trigger's width, capped so it stays a search box rather than
/// stretching into a banner on a wide window.
const PALETTE_W: f32 = 380.0;
/// Its horizontal padding. `Margin` is `i8`, and the inner `set_width` has to
/// subtract the same figure, so it is named once.
const PALETTE_PAD: i8 = 8;

/// The dot in the machine chip.
const DOT: f32 = 6.0;

/// The transport, stated as the truth rather than as the mock's label.
///
/// The core speaks raw DCOM through `IWbemLocator` in both the local and the
/// remote path, and sets `RPC_C_IMP_LEVEL_IMPERSONATE` on every proxy blanket
/// it configures (`enumerate.rs::set_blanket`, `remote.rs`). There is no WinRM
/// anywhere in this project, so the chip must never say so.
const TRANSPORT: &str = "DCOM \u{00b7} Impersonate";

/// Show the title bar.
///
/// The caller must have registered [`super::chrome::title_drag`] *before*
/// calling this, so that every button in here wins the hit test against the
/// drag region -- see the ordering rules in [`super::chrome`].
pub(crate) fn show(app: &mut VmiScopeApp, ui: &mut Ui) {
    egui::Panel::top("vs_titlebar")
        .exact_size(TITLEBAR_H)
        .resizable(false)
        .show_separator_line(false)
        // `exact_size` is the OUTER size, margins and stroke included, and the
        // default panel frame is `Margin::symmetric(8, 2)` -- which would leave
        // 36px of content in a 40px bar. `Frame::NONE` with no margin at all
        // also keeps `ui.max_rect()` equal to the panel's outer rect, which is
        // what the full-bleed hairline below is measured from; the horizontal
        // padding is done with `add_space` instead.
        .frame(Frame::NONE.fill(chrome_fill()))
        .show(ui, |ui| {
            let bar = ui.max_rect();
            // Our own separator: `show_separator_line(false)` alone is not
            // enough on a resizable panel, and even suppressed the panel's line
            // is drawn by the *parent* Ui, so it would sit under our fill.
            solid_hline(
                ui.painter(),
                Rect::from_min_max(
                    Pos2::new(bar.left(), bar.bottom() - HAIRLINE),
                    bar.right_bottom(),
                ),
                DIVIDER,
            );

            ui.horizontal_centered(|ui| {
                ui.add_space(super::PAD_X);
                app_glyph(ui);
                ui.add_space(S2);
                ui.add(
                    Label::new(
                        RichText::new("VMI-Scope")
                            .text_style(TextStyle::Heading)
                            .size(13.5),
                    )
                    .selectable(false),
                );
                pill(ui, &format!("v{}", env!("CARGO_PKG_VERSION")));
                ui.add_space(S3);
                machine_chip(app, ui);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Right to left, so the first thing added is the rightmost.
                    if app.decorated {
                        // The OS caption already carries minimise/maximise/close.
                        // Drawing our own here is the doubled-chrome bug the
                        // escape hatch exists to avoid.
                        ui.add_space(super::PAD_X);
                    } else {
                        window_buttons(ui);
                    }
                    ui.add_space(S2);
                    refresh_button(app, ui);
                    live_toggle(app, ui);
                    ui.add_space(S3);

                    // Whatever is left between the two clusters belongs to the
                    // palette trigger, centred in it.
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        palette_trigger(app, ui);
                    });
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Left cluster
// ---------------------------------------------------------------------------

/// The accent-outlined app mark.
fn app_glyph(ui: &mut Ui) {
    let a = accent(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(GLYPH_BOX), Sense::hover());
    ui.painter()
        .rect_stroke(rect, R_SM, Stroke::new(HAIRLINE, a), StrokeKind::Inside);
    super::centered(
        ui,
        rect,
        icons::glyph(icons::TREE_STRUCTURE)
            .size(GLYPH_SIZE)
            .color(a),
    );
}

/// A small mono badge on a faint text tint -- the version, and the `Ctrl K`
/// hint in the palette trigger.
fn pill(ui: &mut Ui, text: &str) {
    Frame::NONE
        .fill(TEXT.gamma_multiply(0.08))
        .corner_radius(R_SM)
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.add(
                Label::new(
                    RichText::new(text)
                        .text_style(TextStyle::Monospace)
                        .size(10.0)
                        .color(muted(55)),
                )
                .selectable(false),
            );
        });
}

/// Where we are connected, and how. Clicking navigates to the Machines view,
/// which is where the connection controls live.
fn machine_chip(app: &mut VmiScopeApp, ui: &mut Ui) {
    let (dot, host) = match &app.conn_status {
        // Local is neutral rather than OK: it is the resting state, not an
        // achievement.
        ConnStatus::Local => (NEUTRAL[4], local_host()),
        ConnStatus::Connecting => {
            let typed = app.conn_host.trim();
            let host = if typed.is_empty() {
                local_host()
            } else {
                format!("\\\\{typed}")
            };
            (WARN, host)
        }
        ConnStatus::Remote(h) => (OK, format!("\\\\{h}")),
        // A failed connect leaves the worker bound to the local machine, so the
        // host reads local while the dot carries the failure.
        ConnStatus::Failed(_) => (BAD, local_host()),
    };

    let response = Frame::NONE
        .fill(TEXT.gamma_multiply(0.05))
        .corner_radius(R_SM)
        .inner_margin(Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = S2;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(DOT), Sense::hover());
                ui.painter().circle_filled(rect.center(), DOT * 0.5, dot);
                ui.add(
                    Label::new(
                        RichText::new(host)
                            .text_style(TextStyle::Monospace)
                            .size(11.0)
                            .color(muted(85)),
                    )
                    .selectable(false),
                );
                // An explicit 12px rect rather than `rule::vrule`, which takes
                // `ui.available_height()` -- inside a 40px bar that is the whole
                // bar, and the chip would grow to match it.
                let (seam, _) = ui.allocate_exact_size(Vec2::new(HAIRLINE, 12.0), Sense::hover());
                solid_vline(ui.painter(), seam, DIVIDER);
                ui.add(
                    Label::new(
                        RichText::new(TRANSPORT)
                            .text_style(TextStyle::Small)
                            .color(muted(50)),
                    )
                    .selectable(false),
                );
                ui.add(
                    Label::new(icons::glyph(icons::CARET_DOWN).size(9.0).color(muted(40)))
                        .selectable(false),
                );
            });
        })
        .response
        .interact(Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if response.clicked() {
        app.view = View::Machines;
    }
    response.on_hover_text("Connection target");
}

/// The machine name, in the `\\HOST` form WMI itself uses.
fn local_host() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "LOCALHOST".to_string());
    format!("\\\\{}", name.to_uppercase())
}

// ---------------------------------------------------------------------------
// Middle
// ---------------------------------------------------------------------------

/// The command palette's trigger. The palette itself lands with task 2.16; this
/// only owns the flag and the affordance.
fn palette_trigger(app: &mut VmiScopeApp, ui: &mut Ui) {
    let width = PALETTE_W.min(ui.available_width());
    let lead = ((ui.available_width() - width) * 0.5).max(0.0);
    ui.add_space(lead);

    let open = app.palette_open;
    let stroke = Stroke::new(HAIRLINE, if open { accent(ui) } else { DIVIDER });

    let response = Frame::NONE
        .fill(TEXT.gamma_multiply(0.04))
        .stroke(stroke)
        .corner_radius(R_SM)
        .inner_margin(Margin::symmetric(PALETTE_PAD, 3))
        .show(ui, |ui| {
            ui.set_width(width - 2.0 * f32::from(PALETTE_PAD));
            ui.horizontal(|ui| {
                ui.add(
                    Label::new(
                        icons::glyph(icons::MAGNIFYING_GLASS)
                            .size(12.0)
                            .color(muted(45)),
                    )
                    .selectable(false),
                );
                ui.add(
                    Label::new(
                        RichText::new("Search classes, properties, commands")
                            .text_style(TextStyle::Small)
                            .color(muted(38)),
                    )
                    .selectable(false),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, "Ctrl K");
                });
            });
        })
        .response
        .interact(Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
    }
    if response.clicked() {
        app.palette_open = true;
    }
}

// ---------------------------------------------------------------------------
// Right cluster
// ---------------------------------------------------------------------------

/// Pause or resume the live pollers. Today that is the Network view's 1.5s
/// snapshot; the event monitor keeps its own explicit Start/Stop, because a
/// notification subscription is a resource on the far side rather than a poll.
fn live_toggle(app: &mut VmiScopeApp, ui: &mut Ui) {
    let live = !app.net_paused;
    let (icon, label) = if live {
        (icons::PULSE, "Live")
    } else {
        (icons::PAUSE, "Paused")
    };
    if btn_ghost(ui, icons::labelled(ui, icon, label))
        .on_hover_text(if live {
            "Pause the live pollers"
        } else {
            "Resume the live pollers"
        })
        .clicked()
    {
        app.net_paused = !app.net_paused;
    }
}

/// Re-run whatever the active view is showing.
fn refresh_button(app: &mut VmiScopeApp, ui: &mut Ui) {
    let now = ui.input(|i| i.time);
    if btn_ghost(ui, icons::glyph(icons::ARROWS_CLOCKWISE))
        .on_hover_text(format!("Refresh {}", app.view.title()))
        .clicked()
    {
        app.refresh_active_view(now);
    }
}

/// Minimise, maximise/restore and close, right to left.
fn window_buttons(ui: &mut Ui) {
    let is_max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));

    // Close is the only destructive one, so it is the only one that changes
    // colour rather than just lifting.
    if window_button(ui, icons::X, BAD.gamma_multiply(0.30)).clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let restore_icon = if is_max {
        icons::CORNERS_IN
    } else {
        icons::SQUARE
    };
    if window_button(ui, restore_icon, TEXT.gamma_multiply(0.07)).clicked() {
        // No toggle command exists; read the state and send its negation.
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!is_max));
    }
    if window_button(ui, icons::MINUS, TEXT.gamma_multiply(0.07)).clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// One 38x40 frameless caption button.
///
/// Hand-rolled rather than `widgets::button::btn_icon_sized`, which is square
/// and rounded: a caption button has to be full-bleed to the top edge or the
/// pointer can overshoot into a 2px dead strip at the very corner.
fn window_button(ui: &mut Ui, glyph: &str, hover: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(WIN_BTN_W, TITLEBAR_H), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, hover);
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let fg = if hovered { TEXT } else { muted(60) };
    super::centered(ui, rect, icons::glyph(glyph).size(12.0).color(fg));
    response
}
