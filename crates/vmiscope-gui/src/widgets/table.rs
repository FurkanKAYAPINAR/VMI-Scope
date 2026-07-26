//! Table building blocks shared by the results, network, persistence and
//! provider tables.

use eframe::egui;

/// A sortable header cell: shows the title plus a ▲/▼ marker when it's the
/// active sort column. Returns true if the user clicked it.
pub(crate) fn sortable_header(
    ui: &mut egui::Ui,
    title: &str,
    col: usize,
    sort: Option<(usize, bool)>,
) -> bool {
    let marker = match sort {
        Some((c, true)) if c == col => " \u{25b2}",
        Some((c, false)) if c == col => " \u{25bc}",
        _ => "",
    };
    ui.add(egui::Button::new(egui::RichText::new(format!("{title}{marker}")).strong()).frame(false))
        .clicked()
}
