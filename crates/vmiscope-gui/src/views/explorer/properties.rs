//! The Properties sub-tab: the class's declared properties, with the selected
//! instance's values beside them.

use eframe::egui;
use eframe::egui::TextStyle;

use vmiscope_core::QueryResult;

use crate::app::VmiScopeApp;
use crate::theme::icons;
use crate::theme::tokens::{a300, muted};
use crate::widgets::button::{accent, accent_ramp};
use crate::widgets::field::filter_box;
use crate::widgets::loading::spinner;
use crate::widgets::table::{DataTable, DataTableState, TableColumn};

/// Em dash for an absent value -- "we looked and there was nothing", never
/// confused with a value we simply did not fetch.
const EMPTY: &str = "\u{2014}";

/// One rendered property row, precomputed so the table body borrows only this
/// and not the schema, result and selection at once.
struct PropRow {
    name: String,
    cim: String,
    value: String,
    quals: String,
    icon: &'static str,
    /// Key properties get their glyph in the accent; everything else is muted.
    icon_accent: bool,
}

impl VmiScopeApp {
    pub(crate) fn ui_properties_tab(&mut self, ui: &mut egui::Ui) {
        let Some(class) = self.selected_class.clone() else {
            return;
        };

        // No schema yet: say whether it is coming or absent.
        if self.schema_for_selected().is_none() {
            if self.schema_loading && self.schema_class == class {
                spinner(ui, "reflecting properties\u{2026}");
            } else {
                ui.label(egui::RichText::new("No schema for this class.").color(muted(50)));
            }
            return;
        }

        ui.spacing_mut().text_edit_width = ui.available_width();
        filter_box(ui, &mut self.schema_filter, "filter properties");
        let filter = self.schema_filter.to_lowercase();

        // Header: the selected instance's relative path when a row is selected,
        // else the class. Built from the key properties' values, which is what a
        // WMI __RELPATH is -- the system property itself does not survive the
        // query path, so it is reconstructed here.
        let header = self.instance_header(&class);
        ui.label(icons::labelled_styled(
            ui,
            icons::CROSSHAIR_SIMPLE,
            &header,
            TextStyle::Body,
            muted(60),
        ));

        // Precompute every visible row. Borrows end here so the table body is
        // free of them.
        let rows: Vec<PropRow> = {
            let schema = self.schema_for_selected().expect("checked above");
            let selected = self
                .selected_row
                .and_then(|ri| self.result.as_ref().map(|r| (r, ri)));
            schema
                .properties
                .iter()
                .filter(|p| {
                    filter.is_empty()
                        || p.name.to_lowercase().contains(&filter)
                        || p.cim_type.to_lowercase().contains(&filter)
                })
                .map(|p| {
                    let value = selected
                        .and_then(|(result, ri)| value_of(result, ri, &p.name))
                        .unwrap_or_default();
                    let (icon, icon_accent) = prop_icon(p.is_key, p.is_write, &p.cim_type);
                    PropRow {
                        name: p.name.clone(),
                        cim: p.cim_type.clone(),
                        value,
                        quals: qualifiers_note(
                            p.is_key,
                            p.is_read,
                            p.is_write,
                            p.units.as_deref(),
                            p.value_map.len(),
                        ),
                        icon,
                        icon_accent,
                    }
                })
                .collect()
        };

        if rows.is_empty() {
            ui.label(egui::RichText::new("No properties match the filter.").color(muted(50)));
            return;
        }

        // CIM type renders in accent-300 (task 3.25). Resolved once per frame.
        let cim_color = a300(accent_ramp(ui));
        let key_color = accent(ui);

        DataTable::new("explorer-properties")
            .columns([
                TableColumn::initial("Property", 190.0)
                    .at_least(90.0)
                    .sortable(false),
                TableColumn::initial("CIM type", 130.0)
                    .at_least(60.0)
                    .sortable(false),
                TableColumn::remainder("Value").sortable(false),
                TableColumn::initial("Qualifiers", 170.0)
                    .at_least(60.0)
                    .sortable(false),
            ])
            .show(ui, &mut DataTableState::default(), rows.len(), |row| {
                let p = &rows[row.data_index()];
                let (icon, name, accent, is_accent) =
                    (p.icon, p.name.clone(), key_color, p.icon_accent);
                row.cell(move |ui| {
                    let base = ui.visuals().text_color();
                    let icon_color = if is_accent { accent } else { muted(55) };
                    // Section 0 of the job is the icon; recolour it alone so a key
                    // shows its glyph in the accent while its name stays legible.
                    let mut job = icons::labelled_styled(ui, icon, &name, TextStyle::Body, base);
                    if let Some(section) = job.sections.first_mut() {
                        section.format.color = icon_color;
                    }
                    ui.add(egui::Label::new(job));
                });
                row.colored(p.cim.clone(), cim_color);
                if p.value.is_empty() {
                    row.colored(EMPTY, muted(35));
                } else {
                    row.path(p.value.clone());
                }
                row.colored(p.quals.clone(), muted(55));
            });
    }

    /// The instance-path header for the Properties tab.
    fn instance_header(&self, class: &str) -> String {
        let (Some(ri), Some(schema), Some(result)) = (
            self.selected_row,
            self.schema_for_selected(),
            self.result.as_ref(),
        ) else {
            return class.to_string();
        };
        if ri >= result.rows.len() {
            return class.to_string();
        }
        let keys: Vec<String> = schema
            .properties
            .iter()
            .filter(|p| p.is_key)
            .filter_map(|p| value_of(result, ri, &p.name).map(|v| format!("{}=\"{}\"", p.name, v)))
            .collect();
        if keys.is_empty() {
            format!(
                "{class} \u{00b7} instance {} of {}",
                ri + 1,
                result.rows.len()
            )
        } else {
            format!("{class}.{}", keys.join(","))
        }
    }
}

/// The value of property `name` in result row `ri`, or `None` when the result
/// has no such column (system and projected-away properties).
fn value_of(result: &QueryResult, ri: usize, name: &str) -> Option<String> {
    let col = result.columns.iter().position(|c| c == name)?;
    result.rows.get(ri)?.get(col).cloned()
}

/// The per-property leading icon: key, then by type (datetime, numeric), then
/// writability, else plain text.
fn prop_icon(is_key: bool, is_write: bool, cim: &str) -> (&'static str, bool) {
    if is_key {
        return (icons::KEY, true);
    }
    let c = cim.to_lowercase();
    if c.contains("datetime") {
        (icons::CLOCK, false)
    } else if c.contains("int") || c.starts_with("real") || c.starts_with("byte") {
        (icons::HASH, false)
    } else if is_write {
        (icons::PENCIL_SIMPLE, false)
    } else {
        (icons::TEXT_AA, false)
    }
}

/// A compact qualifiers cell built from the flags `PropertySchema` carries.
fn qualifiers_note(
    is_key: bool,
    is_read: bool,
    is_write: bool,
    units: Option<&str>,
    value_map_len: usize,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if is_key {
        parts.push("key".into());
    }
    if is_read {
        parts.push("read".into());
    }
    if is_write {
        parts.push("write".into());
    }
    if let Some(u) = units {
        if !u.is_empty() {
            parts.push(format!("[{u}]"));
        }
    }
    if value_map_len > 0 {
        parts.push(format!("enum({value_map_len})"));
    }
    parts.join(" \u{00b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Win32_Process.Handle` is a read-only key `string`, so its icon is the
    /// key -- and, crucially, in the accent (the tab's stated acceptance).
    #[test]
    fn a_key_property_gets_the_key_glyph_in_accent() {
        let (icon, accent) = prop_icon(true, false, "string");
        assert_eq!(icon, icons::KEY);
        assert!(accent, "the key glyph must be the accented one");
    }

    /// Type drives the non-key icon: a datetime is a clock, a uint a hash, a
    /// writable string a pencil, a read-only string plain text.
    #[test]
    fn non_key_icons_follow_type_then_writability() {
        assert_eq!(prop_icon(false, false, "datetime").0, icons::CLOCK);
        assert_eq!(prop_icon(false, false, "uint32").0, icons::HASH);
        assert_eq!(prop_icon(false, false, "sint64").0, icons::HASH);
        assert_eq!(prop_icon(false, true, "string").0, icons::PENCIL_SIMPLE);
        assert_eq!(prop_icon(false, false, "string").0, icons::TEXT_AA);
        // None of the non-key icons is accented.
        assert!(!prop_icon(false, true, "string").1);
    }

    #[test]
    fn qualifiers_note_lists_flags_and_enum_arity() {
        assert_eq!(
            qualifiers_note(true, true, false, None, 0),
            "key \u{00b7} read"
        );
        assert_eq!(
            qualifiers_note(false, true, true, Some("bytes"), 3),
            "read \u{00b7} write \u{00b7} [bytes] \u{00b7} enum(3)"
        );
        assert_eq!(qualifiers_note(false, false, false, Some(""), 0), "");
    }

    #[test]
    fn value_of_matches_by_column_name() {
        let result = QueryResult {
            columns: vec!["Handle".into(), "Name".into()],
            rows: vec![vec!["4".into(), "System".into()]],
            ..Default::default()
        };
        assert_eq!(value_of(&result, 0, "Handle"), Some("4".to_string()));
        assert_eq!(value_of(&result, 0, "Name"), Some("System".to_string()));
        // A property the projection did not return has no column.
        assert_eq!(value_of(&result, 0, "ExecutablePath"), None);
    }
}
