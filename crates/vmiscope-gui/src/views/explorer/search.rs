//! Global search over the built class / property / method index.

use eframe::egui;

use crate::app::{CentralView, VmiScopeApp};
use crate::theme::icons;

use vmiscope_core::SearchHit;

impl VmiScopeApp {
    // ------------------------------------------------------------------
    // Global search
    // ------------------------------------------------------------------

    pub(crate) fn compute_hits(&self, q: &str) -> Vec<SearchHit> {
        let Some(idx) = self.search_index.as_ref() else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for c in &idx.classes {
            if c.to_lowercase().contains(q) {
                hits.push(SearchHit {
                    class: c.clone(),
                    member: None,
                    is_method: false,
                });
            }
        }
        for (class, props) in &idx.properties {
            for p in props {
                if p.to_lowercase().contains(q) {
                    hits.push(SearchHit {
                        class: class.clone(),
                        member: Some(p.clone()),
                        is_method: false,
                    });
                }
            }
        }
        if idx.has_methods {
            for (class, methods) in &idx.methods {
                for m in methods {
                    if m.to_lowercase().contains(q) {
                        hits.push(SearchHit {
                            class: class.clone(),
                            member: Some(m.clone()),
                            is_method: true,
                        });
                    }
                }
            }
        }
        // Stable order (HashMap iteration is not deterministic).
        hits.sort_by(|a, b| {
            a.class
                .to_lowercase()
                .cmp(&b.class.to_lowercase())
                .then(a.member.cmp(&b.member))
        });
        hits.truncate(300);
        hits
    }

    pub(crate) fn apply_search_hit(&mut self, h: SearchHit) {
        self.central_view = CentralView::Instances;
        match h.member {
            None => self.select_class(h.class),
            Some(m) if h.is_method => {
                self.selected_class = Some(h.class.clone());
                self.query_text = format!("SELECT * FROM {}", h.class);
                self.run_query();
                self.actions_open = true;
                self.act_method = Some(m);
                self.act_args.clear();
                self.act_bools.clear();
                self.act_outcome = None;
                self.request_schema(h.class);
            }
            Some(m) => {
                self.selected_class = Some(h.class.clone());
                self.query_text = format!("SELECT {m} FROM {}", h.class);
                self.run_query();
            }
        }
    }

    pub(crate) fn ui_search(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(icons::labelled(
            ui,
            icons::MAGNIFYING_GLASS,
            "Global search",
        ))
        .id_salt("global-search")
        .show(ui, |ui| {
            let mut build = false;
            ui.horizontal(|ui| {
                if ui.button("Build index").clicked() {
                    build = true;
                }
                ui.checkbox(&mut self.search_methods, "methods");
                if self.search_loading {
                    ui.spinner();
                }
            });
            if build {
                self.request_search_index(self.search_methods);
            }
            let indexed = self.search_index.as_ref().map(|i| i.classes.len());
            match indexed {
                None => {
                    ui.weak("build the index to search class / property / method names");
                    return;
                }
                Some(n) => {
                    ui.weak(format!("{n} classes indexed"));
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .hint_text("search names")
                    .desired_width(f32::INFINITY),
            );
            let q = self.search_text.trim().to_lowercase();
            if q.len() < 2 {
                ui.weak("type at least 2 characters");
                return;
            }
            let hits = self.compute_hits(&q);
            let mut clicked: Option<SearchHit> = None;
            egui::ScrollArea::vertical()
                .id_salt("search-results")
                .max_height(240.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if hits.is_empty() {
                        ui.weak("no matches");
                    }
                    for h in &hits {
                        // A class hit is led by its icon, which needs the
                        // icon family; the member hits are plain text.
                        let label: egui::WidgetText = match &h.member {
                            None => icons::labelled(ui, icons::TREE_STRUCTURE, &h.class).into(),
                            Some(m) if h.is_method => format!("{} :: {}()", h.class, m).into(),
                            Some(m) => format!("{} :: {}", h.class, m).into(),
                        };
                        if ui.selectable_label(false, label).clicked() {
                            clicked = Some(h.clone());
                        }
                    }
                });
            if let Some(h) = clicked {
                self.apply_search_hit(h);
            }
        });
    }
}
