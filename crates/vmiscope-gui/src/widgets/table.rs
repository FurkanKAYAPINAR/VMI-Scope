//! Table building blocks shared by the results, network, persistence and
//! provider tables.

use eframe::egui;

use crate::theme::icons;

/// A sortable header cell: shows the title plus an up/down caret marker when
/// it's the active sort column. Returns true if the user clicked it.
pub(crate) fn sortable_header(
    ui: &mut egui::Ui,
    title: &str,
    col: usize,
    sort: Option<(usize, bool)>,
) -> bool {
    let marker = match sort {
        Some((c, true)) if c == col => format!(" {}", icons::CARET_UP),
        Some((c, false)) if c == col => format!(" {}", icons::CARET_DOWN),
        _ => String::new(),
    };
    ui.add(egui::Button::new(egui::RichText::new(format!("{title}{marker}")).strong()).frame(false))
        .clicked()
}
