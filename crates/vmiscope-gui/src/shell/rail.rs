//! The 64px navigation rail: every destination this application has, all the
//! time.
//!
//! The rail is the roadmap. A destination that is not built yet renders at
//! reduced opacity rather than being hidden -- hiding it would make the tool
//! look finished and leave no place for the work to land -- and selecting one
//! shows an explicit empty state instead of a blank pane.

use eframe::egui;
use eframe::egui::{CursorIcon, Frame, Label, Pos2, Rect, RichText, Sense, TextStyle, Ui, Vec2};

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, DIVIDER, R_MD, S2, TEXT};
use crate::views::nav::{Group, View};
use crate::widgets::button::accent;
use crate::widgets::rule::{faded_hline, solid_vline, HAIRLINE};

use super::{chrome_fill, RAIL_W};

/// Height of one rail item. Eleven of these plus the group rules is 484px,
/// which fits inside the 560px minimum content height with room to spare.
const ITEM_H: f32 = 44.0;

/// How far the selection pill sits inside its item.
const PILL_INSET: f32 = 4.0;

/// The rail icon, and the label under it.
const ICON_SIZE: f32 = 17.0;

/// Width of the hairline between groups. Short and centred, so it reads as a
/// break in a list rather than as a panel edge.
const GROUP_RULE_W: f32 = 40.0;

/// How much of its colour a not-yet-built destination keeps.
const PLACEHOLDER_FADE: f32 = 0.45;

/// The selected item's background pill: 15% of the live accent.
const PILL_TINT: f32 = 0.15;

/// The hover tint on an unselected item.
const HOVER_TINT: f32 = 0.06;

/// Show the rail.
pub(crate) fn show(app: &mut VmiScopeApp, ui: &mut Ui) {
    egui::Panel::left("vs_rail")
        .exact_size(RAIL_W)
        // `Panel::left` and `right` are constructed with `resizable: true`;
        // only `top` and `bottom` default to false. Without this the rail grows
        // a drag handle, and the separator flag below is ignored on its hover
        // and drag branches.
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(chrome_fill()))
        .show(ui, |ui| {
            let rail = ui.max_rect();
            // Our own edge, for the same reason as the title bar's: the panel's
            // separator is painted by the parent Ui and would sit under our fill.
            solid_vline(
                ui.painter(),
                Rect::from_min_max(
                    Pos2::new(rail.right() - HAIRLINE, rail.top()),
                    rail.right_bottom(),
                ),
                DIVIDER,
            );

            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(S2);

            // Everything except the bottom cluster, with a rule wherever the
            // group changes.
            let mut previous: Option<Group> = None;
            for view in View::ALL {
                if view.group() == Group::Config {
                    continue;
                }
                if previous.is_some_and(|g| g != view.group()) {
                    group_rule(ui);
                }
                previous = Some(view.group());
                item(app, ui, view);
            }

            // `Group::Config` is pinned to the bottom: settings is not a
            // destination you scan for, it is one you go to.
            let bottom: Vec<View> = View::ALL
                .into_iter()
                .filter(|v| v.group() == Group::Config)
                .collect();
            let needed = ITEM_H * bottom.len() as f32 + S2;
            ui.add_space((ui.available_height() - needed).max(S2));
            for view in bottom {
                item(app, ui, view);
            }
        });
}

/// The short faded hairline that separates two groups.
fn group_rule(ui: &mut Ui) {
    ui.add_space(S2);
    let (row, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), HAIRLINE), Sense::hover());
    let half = GROUP_RULE_W * 0.5;
    let x = row.center().x;
    // 40px against a 48px fade ramp means `faded_hline` clamps to a symmetric
    // peak -- which is the shape wanted here, not a compromise.
    faded_hline(
        ui.painter(),
        Rect::from_min_max(
            Pos2::new(x - half, row.top()),
            Pos2::new(x + half, row.bottom()),
        ),
        DIVIDER,
    );
    ui.add_space(S2);
}

/// One destination: icon over label, in a pill when selected.
fn item(app: &mut VmiScopeApp, ui: &mut Ui, view: View) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ITEM_H), Sense::click());

    let selected = app.view == view;
    let a = accent(ui);
    let pill = rect.shrink(PILL_INSET);
    if selected {
        ui.painter()
            .rect_filled(pill, R_MD, a.gamma_multiply(PILL_TINT));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(pill, R_MD, TEXT.gamma_multiply(HOVER_TINT));
    }

    let fade = if view.is_placeholder() {
        PLACEHOLDER_FADE
    } else {
        1.0
    };
    let fg = if selected { a } else { muted(65) }.gamma_multiply(fade);

    // A child Ui rather than two `ui.put` calls: the rect is already allocated,
    // and `put` would advance the parent's cursor over it a second time.
    let mut stack = super::stacked_in(ui, rect);
    stack.spacing_mut().item_spacing.y = 1.0;
    stack.add_space(6.0);
    stack.add(Label::new(icons::glyph(view.icon()).size(ICON_SIZE).color(fg)).selectable(false));
    stack.add(
        Label::new(
            RichText::new(view.rail_label())
                .text_style(TextStyle::Name("rail".into()))
                .color(fg),
        )
        .selectable(false),
    );
    drop(stack);

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if response.clicked() {
        app.view = view;
    }

    let tip = if view.is_placeholder() {
        format!("{} \u{2014} {} (not built yet)", view.title(), view.hint())
    } else {
        format!("{} \u{2014} {}", view.title(), view.hint())
    };
    response.on_hover_text(tip);
}
