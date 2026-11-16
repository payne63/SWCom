use egui::{Align, Layout, ScrollArea};
use egui_extras::{Column, TableBuilder};
use std::path::Path;

use super::AppTab;

#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) clear_clicked: bool,
    pub(super) show_plans_clicked: bool,
    pub(super) print_clicked: bool,
    pub(super) paths_changed: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    tab: &mut AppTab,
    available_printers: &[String],
    printer_a4_a3: &mut String,
    printer_a2_a0: &mut String,
    is_processing: bool,
) -> Actions {
    let mut actions = Actions::default();

    ui.heading("Impression Plans");
    ui.label("Sélectionnez un ou plusieurs plans SolidWorks (.slddrw), ou déposez-les ici.");
    ui.horizontal(|ui| {
        ui.label("Imprimante A4 / A3 :");
        printer_combo(ui, "printer_a4_a3", available_printers, printer_a4_a3);
        ui.label("Imprimante A2 / A1 / A0 :");
        printer_combo(ui, "printer_a2_a0", available_printers, printer_a2_a0);
    });
    if available_printers.is_empty() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            "Aucune imprimante disponible détectée sur ce poste.",
        );
    }
    ui.add_space(8.0);

    if !is_processing {
        if let Some(dropped_paths) = dropped_drawing_paths(ui) {
            for path in dropped_paths {
                let path = path.display().to_string();
                if !tab.print_plan_paths.contains(&path) {
                    tab.print_plan_paths.push(path.clone());
                    tab.print_plan_rows.push((path, String::new()));
                    tab.print_plans_visible = true;
                    actions.paths_changed = true;
                }
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
                actions.clear_clicked = ui
                    .add_enabled(
                        !is_processing && !tab.print_plan_paths.is_empty(),
                        egui::Button::new("Effacer la liste").min_size(button_size),
                    )
                    .clicked();
                actions.show_plans_clicked = ui
                    .add_enabled(
                        !is_processing && !tab.print_plan_paths.is_empty(),
                        egui::Button::new("Recherche du format").min_size(button_size),
                    )
                    .clicked();
                actions.print_clicked = ui
                    .add_enabled(
                        !is_processing
                            && tab.print_plans_visible
                            && !tab.print_plan_rows.is_empty()
                            && tab
                                .print_plan_rows
                                .iter()
                                .all(|(_, format)| !format.is_empty())
                            && !printer_a4_a3.is_empty()
                            && !printer_a2_a0.is_empty(),
                        egui::Button::new("Démarrer les impressions").min_size(button_size),
                    )
                    .clicked();
                ui.add_space(8.0);
                ui.label(format!(
                    "{} plan(s) sélectionné(s)",
                    tab.print_plan_paths.len()
                ));
                if tab.print_plan_paths.is_empty() {
                    ui.add_space(4.0);
                    ui.label("Aucun plan sélectionné.");
                } else if tab
                    .print_plan_rows
                    .iter()
                    .any(|(_, format)| format.is_empty())
                {
                    ui.add_space(4.0);
                    ui.label("Cliquez sur « Afficher les plans » pour charger les formats.");
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

fn printer_combo(
    ui: &mut egui::Ui,
    id: &'static str,
    printers: &[String],
    selected_printer: &mut String,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if selected_printer.is_empty() {
            "Choisir une imprimante"
        } else {
            selected_printer.as_str()
        })
        .show_ui(ui, |ui| {
            for printer in printers {
                ui.selectable_value(selected_printer, printer.clone(), printer);
            }
        });
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
