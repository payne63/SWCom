use egui::{Color32, Grid, ScrollArea, TextEdit};

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

    ui.horizontal(|ui| {
        actions.browse_clicked = ui
            .add_enabled(!is_processing, egui::Button::new("Parcourir…"))
            .clicked();
        actions.process_clicked = ui
            .add_enabled(
                !is_processing && !app.file_path.trim().is_empty(),
                egui::Button::new("Lire les propriétés"),
            )
            .clicked();
        actions.update_clicked = ui
            .add_enabled(
                !is_processing && app.has_changes(),
                egui::Button::new("Mettre à jour dans SolidWorks"),
            )
            .clicked();
    });

    if is_processing {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Traitement en cours…");
        });
    } else {
        let (status_text, status_color) = if let Some(status) = &app.status {
            let color = if app.status_is_error {
                Color32::LIGHT_RED
            } else {
                Color32::LIGHT_GREEN
            };
            (status.as_str(), color)
        } else if app.has_changes() {
            ("Modifications non enregistrées.", ui.visuals().text_color())
        } else {
            (" ", ui.visuals().text_color())
        };
        ui.colored_label(status_color, status_text);
    }

    if !app.properties.is_empty() {
        ui.separator();
        ScrollArea::vertical()
            .id_salt("custom_properties_scroll")
            .show(ui, |ui| {
                Grid::new("custom_properties")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.strong("Propriété");
                        ui.strong("Valeur");
                        ui.end_row();

                        for (index, property) in app.properties.iter_mut().enumerate() {
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

    actions
}
