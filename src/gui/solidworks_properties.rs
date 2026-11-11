use egui::{Align, Grid, Layout, ScrollArea, TextEdit};

use super::PropertiesGui;

/// Actions demandées par la vue des propriétés à la fenêtre principale.
#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) process_clicked: bool,
    pub(super) update_clicked: bool,
    pub(super) file_path_changed: bool,
}

/// Affiche la page de lecture et de modification des propriétés SolidWorks.
pub(super) fn show(ui: &mut egui::Ui, app: &mut PropertiesGui, is_processing: bool) -> Actions {
    let mut actions = Actions::default();

    ui.heading("Propriétés SolidWorks");
    ui.label("Choisissez une pièce (.sldprt) ou un assemblage (.sldasm).");
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Fichier :");
        actions.file_path_changed = ui
            .add_enabled(
                !is_processing,
                TextEdit::singleline(&mut app.file_path).desired_width(f32::INFINITY),
            )
            .changed();
    });

    ui.separator();
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
                actions.process_clicked = ui
                    .add_enabled(
                        !is_processing && !app.file_path.trim().is_empty(),
                        egui::Button::new("Lire les propriétés").min_size(button_size),
                    )
                    .clicked();
                actions.update_clicked = ui
                    .add_enabled(
                        !is_processing && app.has_changes(),
                        egui::Button::new("Mettre à jour dans SolidWorks").min_size(button_size),
                    )
                    .clicked();
            },
        );

        ui.separator();
        let table_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(table_width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(table_width);
                if !app.properties.is_empty() {
                    ScrollArea::both()
                        .auto_shrink([false, false])
                        .id_salt("custom_properties_scroll")
                        .show(ui, |ui| {
                            Grid::new("custom_properties")
                                .striped(true)
                                .num_columns(2)
                                .show(ui, |ui| {
                                    ui.strong("Propriété");
                                    ui.strong("Valeur");
                                    ui.end_row();

                                    let mut current_group = None;
                                    for (index, property) in app.properties.iter_mut().enumerate() {
                                        if current_group != Some(property.configuration) {
                                            current_group = Some(property.configuration);
                                            ui.strong(if property.configuration {
                                                "Configuration active"
                                            } else {
                                                "Document"
                                            });
                                            ui.end_row();
                                        }
                                        ui.label(&property.name);
                                        ui.push_id(("custom_property_value", index), |ui| {
                                            ui.add(
                                                TextEdit::singleline(&mut property.value)
                                                    .desired_width(f32::INFINITY),
                                            );
                                        });
                                        ui.end_row();
                                    }
                                });
                        });
                } else if app.has_processed && app.status.is_none() {
                    ui.label("Aucune propriété personnalisée trouvée.");
                }
            },
        );
    });

    actions
}
