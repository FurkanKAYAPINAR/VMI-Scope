//! The Explorer tab, rebuilt (Phase 3).
//!
//! Three fixed columns -- a 224px namespace tree, a 290px class list, and a
//! detail pane -- and, under the detail's breadcrumb and action row, five
//! sub-tabs: Instances, Properties, Methods, Schema, Code. This module owns the
//! column layout and the sub-tab strip; each sub-tab lives in its own file.

use eframe::egui;
use eframe::egui::{Frame, Margin, Pos2, Rect};

use vmiscope_core::{ClassKind, Tally};

use crate::app::{CentralView, VmiScopeApp};
use crate::theme::icons;
use crate::theme::tokens::{BG, DIVIDER, S3, S4};
use crate::widgets::rule::{hrule, solid_vline, HAIRLINE};

pub(crate) mod classlist;
pub(crate) mod code;
pub(crate) mod detail;
pub(crate) mod instances;
pub(crate) mod methods;
pub(crate) mod properties;
pub(crate) mod schema;
pub(crate) mod search;
pub(crate) mod tree;

/// Namespace-tree column width. Exact, per task 3.16.
const NS_TREE_W: f32 = 224.0;
/// Class-list column width. Exact, per task 3.16.
const CLASS_LIST_W: f32 = 290.0;

/// The class-list facet chips: All / Dynamic / Association / Event / System.
///
/// Each is matched against a row's [`ClassKind`]; `All` passes everything. The
/// four kind facets are OR-membership tests, so a dynamic association shows
/// under both Dynamic and Association.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ClassChip {
    #[default]
    All,
    Dynamic,
    Association,
    Event,
    System,
}

impl ClassChip {
    /// Every chip, in strip order.
    pub(crate) const ALL: [ClassChip; 5] = [
        ClassChip::All,
        ClassChip::Dynamic,
        ClassChip::Association,
        ClassChip::Event,
        ClassChip::System,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ClassChip::All => "All",
            ClassChip::Dynamic => "Dynamic",
            ClassChip::Association => "Assoc",
            ClassChip::Event => "Event",
            ClassChip::System => "System",
        }
    }

    /// Does a class of `kind` pass this facet?
    pub(crate) fn matches(self, kind: ClassKind) -> bool {
        match self {
            ClassChip::All => true,
            ClassChip::Dynamic => kind.contains(ClassKind::DYNAMIC),
            ClassChip::Association => kind.contains(ClassKind::ASSOCIATION),
            ClassChip::Event => kind.contains(ClassKind::EVENT),
            ClassChip::System => kind.contains(ClassKind::SYSTEM),
        }
    }
}

/// The Code sub-tab's language. Four-way, where the persisted default
/// (`config::CodeLang` / `app::ScriptLang`) is only two -- PowerShell and
/// VBScript delegate to `util::generate_script`, C# and WQL are generated in
/// `code`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CodeTab {
    PowerShell,
    CSharp,
    VbScript,
    Wql,
}

/// The five detail sub-tabs, in strip order, each with its icon.
const SUB_TABS: [(CentralView, &str, &str); 5] = [
    (CentralView::Instances, icons::LIST_BULLETS, "Instances"),
    (CentralView::Properties, icons::INFO, "Properties"),
    (CentralView::Methods, icons::FUNCTION, "Methods"),
    (CentralView::Schema, icons::TREE_STRUCTURE, "Schema"),
    (CentralView::Code, icons::CODE, "Code"),
];

impl VmiScopeApp {
    /// The whole Explorer view: two fixed left columns, the optional Actions
    /// panel, and the detail central panel.
    pub(crate) fn ui_explorer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("vs_ns_tree")
            .exact_size(NS_TREE_W)
            // `Panel::left` is constructed resizable; a fixed column needs both
            // of these, and even suppressed the parent draws the separator, so
            // the column paints its own edge below.
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| {
                column_edge(ui);
                padded(ui, |ui| {
                    self.ui_namespace_tree(ui);
                    hrule(ui);
                    self.ui_search(ui);
                });
            });

        egui::Panel::left("vs_class_list")
            .exact_size(CLASS_LIST_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(BG))
            .show(ui, |ui| {
                column_edge(ui);
                padded(ui, |ui| self.ui_class_list(ui));
            });

        // The method-invocation panel, unchanged from before the rebuild: the
        // detail's Invoke action opens it, task 3.31/3.32 replace it with a modal.
        if self.actions_open {
            egui::Panel::right("vs_actions")
                .resizable(true)
                .default_size(360.0)
                .size_range(egui::Rangef::new(260.0, 620.0))
                .show(ui, |ui| self.ui_actions(ui));
        }

        egui::CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(BG)
                    .inner_margin(Margin::symmetric(S4 as i8, S3 as i8)),
            )
            .show(ui, |ui| self.ui_explorer_detail(ui));
    }

    /// The detail pane: breadcrumb + header + action row, then -- when a class
    /// is selected -- the sub-tab strip and the active sub-tab. When no class is
    /// selected it says so rather than showing a blank rectangle (task 3.33).
    fn ui_explorer_detail(&mut self, ui: &mut egui::Ui) {
        self.ui_detail(ui);

        if self.selected_class.is_none() {
            // Through the kit rather than hand-built: this was the third copy
            // of the same centred icon-title-note block, and task 7.6 collapsed
            // them into `widgets::loading::empty_state`.
            crate::widgets::loading::empty_state(
                ui,
                icons::CUBE,
                "No class selected",
                "Pick a class from the list to inspect its instances, properties, methods \
                 and schema.",
            );
            return;
        }

        hrule(ui);
        self.ui_subtab_strip(ui);
        hrule(ui);

        match self.central_view {
            CentralView::Instances => self.ui_instances_tab(ui),
            CentralView::Properties => self.ui_properties_tab(ui),
            CentralView::Methods => self.ui_methods_tab(ui),
            CentralView::Schema => self.ui_schema_tab(ui),
            CentralView::Code => self.ui_code_tab(ui),
        }
    }

    /// The five-tab strip. Each tab shows its icon, its name and -- where the
    /// data is real -- a count. A tab whose count is not yet known shows no
    /// number rather than a `0`, which would read as "none".
    fn ui_subtab_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (tab, icon, name) in SUB_TABS {
                let label = match self.subtab_count(tab) {
                    Some(count) => format!("{name}  {count}"),
                    None => name.to_string(),
                };
                let selected = self.central_view == tab;
                if ui
                    .selectable_label(selected, icons::labelled(ui, icon, &label))
                    .clicked()
                {
                    self.central_view = tab;
                }
            }
        });
    }

    /// The count shown beside a sub-tab, drawn from real data, or `None` when
    /// there is nothing honest to show yet.
    fn subtab_count(&self, tab: CentralView) -> Option<String> {
        match tab {
            CentralView::Instances => self.instances_tab_count(),
            CentralView::Properties => self
                .schema_for_selected()
                .map(|s| s.properties.len().to_string()),
            CentralView::Methods => self
                .schema_for_selected()
                .map(|s| s.methods.len().to_string()),
            // Associations, once fetched, are the Schema tab's headline number.
            CentralView::Schema => {
                let class = self.selected_class.as_deref()?;
                if self.assoc_class == class {
                    self.associations.as_ref().map(|a| a.len().to_string())
                } else {
                    None
                }
            }
            CentralView::Code => None,
        }
    }

    /// The Instances tab's count badge: the per-class tally when we have one
    /// (honest about skips and partials via [`Tally::badge`]), else the row count
    /// of the loaded result.
    fn instances_tab_count(&self) -> Option<String> {
        if let Some(class) = self.selected_class.as_deref() {
            if let Some(tally) = self.instance_counts.get(class) {
                return Some(tally.badge());
            }
        }
        self.result.as_ref().map(|r| r.rows.len().to_string())
    }

    /// The reflected schema, but only when it belongs to the selected class --
    /// a stale schema from the previous selection must not label this one.
    pub(crate) fn schema_for_selected(&self) -> Option<&vmiscope_core::ClassSchema> {
        let class = self.selected_class.as_deref()?;
        match &self.schema {
            Some(schema) if self.schema_class == class => Some(schema),
            _ => None,
        }
    }

    /// The instance tally for the selected class, if one has arrived.
    pub(crate) fn selected_tally(&self) -> Option<&Tally> {
        let class = self.selected_class.as_deref()?;
        self.instance_counts.get(class)
    }
}

/// Content padding inside a fixed column, applied as an inner frame so the
/// column's outer rect (and therefore its painted edge) stays flush with the
/// panel's exact width.
fn padded<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::NONE
        .inner_margin(Margin::symmetric(S3 as i8, (S3 * 0.5) as i8))
        .show(ui, add)
        .inner
}

/// Paint a column's own right-hand divider. The panel's separator is suppressed
/// (it is drawn by the parent Ui and would sit under the fill), so the edge is
/// painted here, flush with the panel's outer rect.
fn column_edge(ui: &egui::Ui) {
    let r = ui.max_rect();
    solid_vline(
        ui.painter(),
        Rect::from_min_max(Pos2::new(r.right() - HAIRLINE, r.top()), r.right_bottom()),
        DIVIDER,
    );
}
