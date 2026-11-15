use egui::{Align, Layout, ScrollArea};
use egui_extras::{Column, TableBuilder};
use std::path::Path;

use super::AppTab;

#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) show_plans_clicked: bool,
    pub(super) print_clicked: bool,
    pub(super) paths_changed: bool,
}

pub(super) fn show(ui: &mut egui::Ui, tab: &mut AppTab, is_processing: bool) -> Actions {
    let mut actions = Actions::default();

    ui.heading("Impression Plans");
    ui.label("Sélectionnez un ou plusieurs plans SolidWorks (.slddrw), ou déposez-les ici.");
    ui.add_space(8.0);

    if let Some(dropped_paths) = dropped_drawing_paths(ui) {
        for path in dropped_paths {
            let path = path.display().to_string();
            if !tab.print_plan_paths.contains(&path) {
                tab.print_plan_paths.push(path);
                actions.paths_changed = true;
            }
        }
    }

    let content_height = ui.available_height();
    ui.horizontal(|ui| {
        let available_width = ui.available_width();
        let actions_width = (available_width * 0.32).clamp(220.0, 270.0);
        ui.allocate_ui_with_layout(
            egui::vec2(actions_width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(actions_width);
                let button_size = egui::vec2(actions_width, ui.spacing().interact_size.y);
                actions.browse_clicked = ui
                    .add_enabled(
                        !is_processing,
                        egui::Button::new("Parcourir…").min_size(button_size),
                    )
                    .clicked();
                actions.show_plans_clicked = ui
                    .add_enabled(
                        !is_processing && !tab.print_plan_paths.is_empty(),
                        egui::Button::new("Afficher les plans").min_size(button_size),
                    )
                    .clicked();
                actions.print_clicked = ui
                    .add_enabled(
                        !is_processing
                            && tab.print_plans_visible
                            && !tab.print_plan_paths.is_empty(),
                        egui::Button::new("Démarrer les impressions").min_size(button_size),
                    )
                    .clicked();
                ui.add_space(8.0);
                ui.label(format!(
                    "{} plan(s) sélectionné(s)",
                    tab.print_plan_paths.len()
                ));
                if !tab.print_plans_visible {
                    ui.add_space(4.0);
                    ui.label("La liste sera affichée après validation.");
                }
            },
        );

        ui.separator();
        let table_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(table_width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(table_width);
                if tab.print_plans_visible {
                    ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            TableBuilder::new(ui)
                                .id_salt(("print_plans", tab.id))
                                .striped(true)
                                .resizable(true)
                                .column(Column::initial(300.0).at_least(120.0).clip(true))
                                .column(Column::initial(150.0).at_least(100.0).clip(true))
                                .header(26.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Nom du plan");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Format papier");
                                    });
                                })
                                .body(|mut body| {
                                    for (file_path, format) in &tab.print_plan_rows {
                                        let path = Path::new(file_path);
                                        let name = path
                                            .file_stem()
                                            .map(|name| name.to_string_lossy())
                                            .unwrap_or_default();
                                        body.row(26.0, |mut row| {
                                            row.col(|ui| {
                                                ui.label(name);
                                            });
                                            row.col(|ui| {
                                                ui.label(format);
                                            });
                                        });
                                    }
                                });
                        });
                }
            },
        );
    });

    actions
}

fn dropped_drawing_paths(ui: &egui::Ui) -> Option<Vec<std::path::PathBuf>> {
    let paths: Vec<_> = ui.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| {
                let path = file.path.as_ref()?;
                let extension = path.extension()?.to_str()?;
                extension
                    .eq_ignore_ascii_case("slddrw")
                    .then(|| path.clone())
            })
            .collect()
    });
    (!paths.is_empty()).then_some(paths)
}
