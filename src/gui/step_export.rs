use egui::{Align, Layout, ScrollArea};
use egui_extras::{Column, TableBuilder};
use std::path::Path;

use super::AppTab;

#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) clear_clicked: bool,
    pub(super) export_clicked: bool,
    pub(super) paths_changed: bool,
}

pub(super) fn show(ui: &mut egui::Ui, tab: &mut AppTab, is_processing: bool) -> Actions {
    let mut actions = Actions::default();
    ui.heading("Export STEP");
    ui.label(
        "Ajoutez des pièces (.sldprt) et des assemblages (.sldasm) à exporter en STEP AP214. Les assemblages sont exportés complets.",
    );
    ui.add_space(8.0);

    if !is_processing {
        if let Some(paths) = dropped_model_paths(ui) {
            for path in paths {
                let path = path.display().to_string();
                if !tab.step_file_paths.contains(&path) {
                    tab.step_file_paths.push(path);
                    actions.paths_changed = true;
                }
            }
        }
    }

    let content_height = ui.available_height();
    ui.horizontal(|ui| {
        let actions_width = (ui.available_width() * 0.32).clamp(220.0, 270.0);
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
                actions.clear_clicked = ui
                    .add_enabled(
                        !is_processing && !tab.step_file_paths.is_empty(),
                        egui::Button::new("Effacer la liste").min_size(button_size),
                    )
                    .clicked();
                actions.export_clicked = ui
                    .add_enabled(
                        !is_processing && !tab.step_file_paths.is_empty(),
                        egui::Button::new("Exporter en STEP AP214").min_size(button_size),
                    )
                    .clicked();
                ui.add_space(8.0);
                ui.label(format!(
                    "{} fichier(s) sélectionné(s)",
                    tab.step_file_paths.len()
                ));
            },
        );

        ui.separator();
        let table_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(table_width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(table_width);
                if !tab.step_file_paths.is_empty() {
                    ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            TableBuilder::new(ui)
                                .id_salt(("step_export_files", tab.id))
                                .striped(true)
                                .resizable(true)
                                .column(Column::initial(300.0).at_least(140.0).clip(true))
                                .column(Column::initial(130.0).at_least(100.0).clip(true))
                                .header(26.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Nom du fichier");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Type");
                                    });
                                })
                                .body(|mut body| {
                                    for file_path in &tab.step_file_paths {
                                        let path = Path::new(file_path);
                                        let name = path
                                            .file_name()
                                            .map(|name| name.to_string_lossy())
                                            .unwrap_or_default();
                                        let kind = path
                                            .extension()
                                            .map(|extension| {
                                                extension.to_string_lossy().to_ascii_uppercase()
                                            })
                                            .unwrap_or_else(|| "—".to_owned());
                                        body.row(26.0, |mut row| {
                                            row.col(|ui| {
                                                ui.label(name);
                                            });
                                            row.col(|ui| {
                                                ui.label(kind);
                                            });
                                        });
                                    }
                                });
                        });
                } else {
                    ui.label("Aucun fichier sélectionné.");
                }
            },
        );
    });

    actions
}

fn dropped_model_paths(ui: &egui::Ui) -> Option<Vec<std::path::PathBuf>> {
    let paths: Vec<_> = ui.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| {
                let path = file.path.as_ref()?;
                let extension = path.extension()?.to_str()?;
                ["sldprt", "sldasm"]
                    .iter()
                    .any(|accepted| extension.eq_ignore_ascii_case(accepted))
                    .then(|| path.clone())
            })
            .collect()
    });
    (!paths.is_empty()).then_some(paths)
}
