//! The class list: the Explorer's second column.

use eframe::egui;
use eframe::egui::{Align, Layout, RichText, TextStyle};

use vmiscope_core::{ClassKind, Tally};

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{muted, WARN};
use crate::views::explorer::ClassChip;
use crate::widgets::button::btn_secondary;
use crate::widgets::chip::{kind_badge, Kind};
use crate::widgets::field::filter_box;
use crate::widgets::loading::{empty_state, spinner};

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // UI: class list
    // ------------------------------------------------------------------

    pub(crate) fn ui_class_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(icons::labelled(ui, icons::CUBE, "Classes"));
            if self.classes_loading {
                spinner(ui, "listing");
            }
        });

        // The filter spans the column. `spacing.text_edit_width` is where a kit
        // input takes its width from, and egui's 280 default would leave a wide
        // column half empty.
        ui.spacing_mut().text_edit_width = ui.available_width();
        filter_box(ui, &mut self.class_filter, "filter classes");

        // Settings -> Results -> Show system classes. `root\CIMV2` carries
        // several hundred `__`-prefixed system classes (`__EventFilter`,
        // `__NAMESPACE`, `__Win32Provider`, the whole `__` meta-model) and for
        // most browsing they are noise between the classes someone came to
        // read. Hiding them is a *list* filter, never a data one: the System
        // chip still reaches every one of them with the setting off, so nothing
        // becomes unreachable -- which is the line between decluttering and
        // concealing evidence in a security tool.
        let hide_system = !self.config.show_system_classes && self.class_chip != ClassChip::System;

        // Facet chips, each with a live count over the whole class list (not the
        // text filter, so the counts stay a stable overview of the namespace).
        // The System count is deliberately over everything, so it reads as
        // "and there are 412 more over here" rather than agreeing with a list
        // that is hiding them.
        let mut chip_counts = [0usize; ClassChip::ALL.len()];
        for c in &self.classes {
            for (i, chip) in ClassChip::ALL.iter().enumerate() {
                if chip.matches(c.kind) {
                    chip_counts[i] += 1;
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            for (i, chip) in ClassChip::ALL.into_iter().enumerate() {
                let label = format!("{} {}", chip.label(), chip_counts[i]);
                if ui
                    .selectable_label(self.class_chip == chip, RichText::new(label))
                    .clicked()
                {
                    self.class_chip = chip;
                }
            }
        });

        // Apply both filters once, cloning the little each row needs so the list
        // body can borrow `self` for the count state without fighting the class
        // vector.
        let filter = self.class_filter.to_lowercase();
        let chip = self.class_chip;
        let filtered: Vec<(String, ClassKind)> = self
            .classes
            .iter()
            .filter(|c| chip.matches(c.kind))
            .filter(|c| !(hide_system && is_system(c)))
            .filter(|c| filter.is_empty() || c.name.to_lowercase().contains(&filter))
            .map(|c| (c.name.clone(), c.kind))
            .collect();
        let total = self.classes.len();
        let hidden = if hide_system {
            self.classes
                .iter()
                .filter(|c| chip.matches(c.kind) && is_system(c))
                .count()
        } else {
            0
        };

        // The one explicit way to count a whole list. Counts are expensive and
        // per-class, so nothing here fires until the user asks -- or until a
        // single class is selected (handled in `select_class`).
        let mut count_all = false;
        ui.horizontal(|ui| {
            if btn_secondary(ui, icons::labelled(ui, icons::LIST_BULLETS, "Count"))
                .on_hover_text("Count instances of every class in the filtered list")
                .clicked()
            {
                count_all = true;
            }
            ui.label(
                RichText::new(format!("{} of {} classes", filtered.len(), total))
                    .text_style(TextStyle::Name("caption".into()))
                    .color(muted(45)),
            );
            ui.label(
                RichText::new(format!("\u{00b7} {}", chip.label()))
                    .text_style(TextStyle::Name("caption".into()))
                    .color(muted(35)),
            );
            // Never silently: a list that is shorter than the namespace has to
            // say so, and say what turns it back on. Nothing in this tool
            // removes rows without a count beside the hole.
            if hidden > 0 {
                ui.label(
                    RichText::new(format!("\u{00b7} {hidden} system hidden"))
                        .text_style(TextStyle::Name("caption".into()))
                        .color(muted(35)),
                )
                .on_hover_text(
                    "Classes whose names begin '__'. The System chip still lists every one \
                     of them, and Settings -> Results -> Show system classes puts them back \
                     in this list.",
                );
            }
        });

        // Task 7.6: the list scrolled an empty rectangle whenever the filter,
        // the facet chip or the system-class setting excluded everything, and
        // there was no state at all for "the namespace has not been enumerated
        // yet". Four situations, four sentences -- and each names what to undo,
        // because "no classes" in a namespace that has hundreds is a filter
        // result, not a fact about WMI.
        if filtered.is_empty() {
            let (title, note) = class_empty_note(
                total,
                self.classes_loading,
                hidden > 0,
                !filter.is_empty(),
                chip != ClassChip::All,
            );
            empty_state(ui, icons::CUBE, title, note);
            return;
        }

        let mut clicked: Option<String> = None;
        let row_h = ui.text_style_height(&egui::TextStyle::Body).max(15.0) + 6.0;
        egui::ScrollArea::vertical()
            .id_salt("class-list")
            .auto_shrink([false, false])
            .show_rows(ui, row_h, filtered.len(), |ui, range| {
                for i in range {
                    let (name, kind) = &filtered[i];
                    let selected = self.selected_class.as_deref() == Some(name.as_str());
                    ui.horizontal(|ui| {
                        kind_badge(ui, badge_kind(*kind));
                        if ui.selectable_label(selected, name).clicked() {
                            clicked = Some(name.clone());
                        }
                        self.count_cell(ui, name);
                    });
                }
            });

        if count_all {
            for (name, _) in &filtered {
                self.request_instance_count(name.clone());
            }
        }
        if let Some(class) = clicked {
            self.select_class(class);
        }
    }

    /// The right-aligned instance-count cell for one class row.
    ///
    /// A skipped class shows an em dash, a partial count a trailing `+`, and a
    /// class still being counted a small spinner -- never a bare `0` that would
    /// read as "no instances" when it means "not counted".
    fn count_cell(&self, ui: &mut egui::Ui, class: &str) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(tally) = self.instance_counts.get(class) {
                let (color, tip) = match tally {
                    Tally::Skipped(_) => (muted(35), tally.note()),
                    Tally::Counted { .. } if tally.exact().is_some() => (muted(65), None),
                    // Counted but not complete: a lower bound.
                    Tally::Counted { .. } => (WARN, tally.note()),
                };
                let label = ui.label(
                    RichText::new(tally.badge())
                        .text_style(TextStyle::Name("code".into()))
                        .color(color),
                );
                if let Some(tip) = tip {
                    label.on_hover_text(tip);
                }
            } else if self.counting.contains(class) {
                ui.add(egui::Spinner::new().size(11.0));
            }
        });
    }
}

/// Which "no classes" this is, and what to undo about it.
///
/// Ordered by which explanation is most actionable: an active filter is the
/// likeliest cause and the easiest to reverse, so it leads even when the facet
/// chip is also narrowing the list.
fn class_empty_note(
    total: usize,
    loading: bool,
    hiding_system: bool,
    filtering: bool,
    faceted: bool,
) -> (&'static str, &'static str) {
    if loading {
        return ("Enumerating classes", "The namespace is being read.");
    }
    if total == 0 {
        return (
            "No classes",
            "This namespace returned none. Some namespaces are containers for other \
             namespaces and hold no classes of their own; others refuse the enumeration \
             entirely, which shows as an error in the status bar.",
        );
    }
    if filtering {
        (
            "No classes match the filter",
            "The namespace has classes; none of their names contain what you typed.",
        )
    } else if faceted {
        (
            "No classes in this facet",
            "The namespace has classes, but none of this kind. Pick All to see them.",
        )
    } else if hiding_system {
        (
            "Only system classes here",
            "Every class in this namespace begins '__'. Settings -> Results -> Show system \
             classes puts them in the list, and the System chip lists them either way.",
        )
    } else {
        (
            "No classes",
            "Nothing matched, and no filter is set \u{2014} which should not happen. Re-select \
             the namespace to enumerate it again.",
        )
    }
}

/// Is this one of WMI's system classes?
///
/// The name, not [`ClassKind::SYSTEM`], and the difference is measurable: the
/// kind flag comes from the class's own qualifiers, and plenty of classes carry
/// it without a `__` prefix while the Settings row promises exactly "names
/// beginning `__`". The user-facing rule has to be the one the label states, or
/// the setting hides a set nobody can predict.
fn is_system(class: &vmiscope_core::ClassBrief) -> bool {
    class.name.starts_with("__")
}

/// Map a class's full [`ClassKind`] onto the single-letter badge the list
/// shows. Event outranks association outranks everything else, matching the
/// skip-list's own precedence: a dynamic association is an `A`, an event class
/// (even an abstract one) is an `E`, and a plain or dynamic class is a `C`.
fn badge_kind(kind: ClassKind) -> Kind {
    if kind.contains(ClassKind::EVENT) {
        Kind::Event
    } else if kind.contains(ClassKind::ASSOCIATION) {
        Kind::Association
    } else {
        Kind::Class
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The badge precedence has to match the skip-list's: event beats
    /// association beats class, so an abstract event still reads `E` and a
    /// dynamic association still reads `A`.
    #[test]
    fn badge_precedence_matches_the_skip_list() {
        assert_eq!(badge_kind(ClassKind::DYNAMIC), Kind::Class);
        assert_eq!(badge_kind(ClassKind::NONE), Kind::Class);
        assert_eq!(
            badge_kind(ClassKind::DYNAMIC | ClassKind::ASSOCIATION),
            Kind::Association
        );
        assert_eq!(
            badge_kind(ClassKind::ABSTRACT | ClassKind::ASSOCIATION),
            Kind::Association
        );
        assert_eq!(
            badge_kind(ClassKind::EVENT | ClassKind::ABSTRACT | ClassKind::SYSTEM),
            Kind::Event
        );
        // Event wins even when the class is also an association.
        assert_eq!(
            badge_kind(ClassKind::EVENT | ClassKind::ASSOCIATION),
            Kind::Event
        );
    }

    /// The facet chips are OR-membership tests, and `All` passes everything --
    /// the property the footer count and the badge filter both rely on.
    #[test]
    fn chips_match_by_membership() {
        assert!(ClassChip::All.matches(ClassKind::NONE));
        assert!(ClassChip::Dynamic.matches(ClassKind::DYNAMIC | ClassKind::PERF));
        assert!(!ClassChip::Dynamic.matches(ClassKind::ABSTRACT));
        assert!(ClassChip::Association.matches(ClassKind::DYNAMIC | ClassKind::ASSOCIATION));
        assert!(ClassChip::Event.matches(ClassKind::EVENT | ClassKind::ABSTRACT));
        assert!(ClassChip::System.matches(ClassKind::SYSTEM | ClassKind::DYNAMIC));
        assert!(!ClassChip::System.matches(ClassKind::DYNAMIC));
    }
}
