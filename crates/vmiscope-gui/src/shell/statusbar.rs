//! The 24px status bar: where we are connected, what the active view is
//! looking at, and the two shortcuts worth advertising.

use eframe::egui;
use eframe::egui::{Align, Frame, Label, Layout, Pos2, Rect, RichText, Sense, TextStyle, Ui, Vec2};

use crate::app::{ConnStatus, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::{muted, BAD, DIVIDER, NEUTRAL, OK, S2, S3, WARN};
use crate::views::nav::View;
use crate::widgets::button::btn_secondary;
use crate::widgets::rule::{solid_hline, HAIRLINE};

use super::{chrome_fill, PAD_X, STATUS_H};

/// The connection dot.
const DOT: f32 = 6.0;

/// Show the status bar.
pub(crate) fn show(app: &mut VmiScopeApp, ui: &mut Ui) {
    egui::Panel::bottom("vs_status")
        .exact_size(STATUS_H)
        .resizable(false)
        .show_separator_line(false)
        // Same `exact_size` caveat as the title bar: the size is the OUTER one,
        // and the default panel frame's `Margin::symmetric(8, 2)` would leave
        // 20px of a 24px bar.
        .frame(Frame::NONE.fill(chrome_fill()))
        .show(ui, |ui| {
            let bar = ui.max_rect();
            solid_hline(
                ui.painter(),
                Rect::from_min_max(bar.left_top(), Pos2::new(bar.right(), bar.top() + HAIRLINE)),
                DIVIDER,
            );

            ui.horizontal_centered(|ui| {
                ui.add_space(PAD_X);

                if let Some(error) = &app.error {
                    // An error owns the whole left half: the namespace and the
                    // last query are still true, they are just not the news.
                    ui.add(
                        Label::new(icons::labelled_styled(
                            ui,
                            icons::WARNING,
                            &error.replace('\n', "  \u{2014}  "),
                            TextStyle::Small,
                            BAD,
                        ))
                        .selectable(false)
                        .truncate(),
                    );
                } else {
                    connection(app, ui);
                    dim(ui, "\u{00b7}");
                    dim(ui, &context_line(app));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(PAD_X);
                    // The error log is the one control here, and it only exists
                    // once something has gone wrong.
                    if !app.error_log.is_empty()
                        && btn_secondary(ui, format!("Log ({})", app.error_log.len()))
                            .on_hover_text("Everything that has failed this session")
                            .clicked()
                    {
                        app.error_log_open = !app.error_log_open;
                    }
                    ui.add_space(S2);
                    dim(ui, "F5 refresh");
                    dim(ui, "\u{00b7}");
                    dim(ui, "Ctrl K palette");
                });
            });
        });
}

/// The live dot and what it is connected to.
fn connection(app: &mut VmiScopeApp, ui: &mut Ui) {
    let (color, text) = match &app.conn_status {
        ConnStatus::Local => (NEUTRAL[4], "Local machine".to_string()),
        ConnStatus::Connecting => (WARN, "Connecting\u{2026}".to_string()),
        ConnStatus::Remote(host) => {
            let mode = if app.conn_use_creds {
                "alt creds"
            } else {
                "current user"
            };
            (OK, format!("{host} \u{00b7} {mode}"))
        }
        ConnStatus::Failed(e) => (
            BAD,
            e.lines().next().unwrap_or("connection failed").to_string(),
        ),
    };

    let (rect, _) = ui.allocate_exact_size(Vec2::splat(DOT), Sense::hover());
    ui.painter().circle_filled(rect.center(), DOT * 0.5, color);
    ui.add_space(S2);
    ui.add(
        Label::new(
            RichText::new(text)
                .text_style(TextStyle::Small)
                .color(muted(70)),
        )
        .selectable(false),
    );
}

/// What the active view is currently looking at.
///
/// One line per destination, because "Namespace: root\CIMV2" is meaningless on
/// the Network view and a blank status bar is worse than a terse one.
fn context_line(app: &VmiScopeApp) -> String {
    match app.view {
        View::Explorer | View::Query => {
            if app.result_wql.is_empty() {
                format!("Namespace: {}", app.active_ns)
            } else {
                format!("{} \u{00b7} {}", app.active_ns, app.result_wql)
            }
        }
        View::Events => {
            if app.monitor.is_some() {
                format!("Monitoring \u{00b7} {} events", app.events_log.len())
            } else {
                "Monitor stopped".to_string()
            }
        }
        View::Network => {
            let live = app.net_conns.values().filter(|c| c.alive).count();
            let state = if app.net_paused { "paused" } else { "live" };
            format!("{live} connections \u{00b7} {state}")
        }
        View::Persistence => match &app.events_report {
            Some(report) => format!("{} subscriptions", report.subscriptions.len()),
            None => "No scan yet".to_string(),
        },
        View::Providers => match &app.providers {
            Some(providers) => format!("{} providers", providers.len()),
            None => "Not loaded".to_string(),
        },
        // Composed in `views::process`, which owns the state it counts.
        View::Process => app.proc_status(),
        // The rail shows these; the status bar should agree rather than going
        // blank and implying the view simply has nothing to say.
        other => format!("{} \u{2014} not built yet", other.title()),
    }
}

/// A quiet run of text. The status bar is all secondary information; nothing in
/// it competes with the content above.
fn dim(ui: &mut Ui, text: &str) {
    ui.add_space(S3 - ui.spacing().item_spacing.x);
    ui.add(
        Label::new(
            RichText::new(text)
                .text_style(TextStyle::Small)
                .color(muted(42)),
        )
        .selectable(false),
    );
}
