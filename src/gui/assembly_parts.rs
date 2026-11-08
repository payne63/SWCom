use egui::{Align, Color32, Grid, Layout, ScrollArea, TextEdit};

use crate::solidworks::AssemblyItemKind;

use super::AppTab;

/// Actions demandées par la vue de la liste des composants d'assemblage.
#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) process_clicked: bool,
    pub(super) path_changed: bool,
}

/// Affiche la liste aplatie des composants de l'assemblage courant.
pub(super) fn show(
    ui: &mut egui::Ui,
    tab: &mut AppTab,
    is_processing: bool,
    processing_tab_id: Option<u64>,
) -> (Actions, Option<(String, egui::TextureHandle)>) {
    let mut actions = Actions::default();
    let mut hovered_thumbnail = None;

    ui.heading("Liste a plat des composants");
    ui.label(
        "Les pièces, sous-assemblages et l'assemblage maître sont regroupés avec leurs quantités.",
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Assemblage :");
        actions.path_changed = ui
            .add_enabled(
                !is_processing,
                TextEdit::singleline(&mut tab.assembly_file_path).desired_width(f32::INFINITY),
            )
            .changed();
    });

    ui.horizontal(|ui| {
        actions.browse_clicked = ui
            .add_enabled(!is_processing, egui::Button::new("Parcourir…"))
            .clicked();
        actions.process_clicked = ui
            .add_enabled(
                !is_processing && !tab.assembly_file_path.trim().is_empty(),
                egui::Button::new("Afficher les composants"),
            )
            .clicked();
    });

    if is_processing && processing_tab_id == Some(tab.id) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Lecture de l'assemblage en cours…");
        });
    } else if is_processing {
        ui.label("Une autre opération est en cours…");
    } else if let Some(status) = &tab.assembly_status {
        let color = if tab.assembly_status_is_error {
            Color32::LIGHT_RED
        } else {
            Color32::LIGHT_GREEN
        };
        ui.colored_label(color, status);
    }

    if !tab.assembly_parts.is_empty() {
        ui.separator();
        ScrollArea::vertical().show(ui, |ui| {
            Grid::new(("assembly_parts", tab.id))
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong("Type");
                    ui.strong("Nom");
                    ui.strong("Chemin");
                    ui.strong("Quantité totale");
                    ui.end_row();

                    for item in &tab.assembly_parts {
                        let thumbnail = tab.assembly_thumbnails.get(&item.path.to_lowercase());
                        let (label, color) = match item.kind {
                            AssemblyItemKind::Part => ("Pièce", Color32::LIGHT_BLUE),
                            AssemblyItemKind::SheetMetal => ("Tôlerie", Color32::LIGHT_YELLOW),
                            AssemblyItemKind::Assembly => ("Assemblage", Color32::LIGHT_GREEN),
                        };

                        let type_response = ui
                            .with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let icon_size = ui.text_style_height(&egui::TextStyle::Body);
                                let (icon_rect, icon_response) = ui.allocate_exact_size(
                                    egui::vec2(icon_size, icon_size),
                                    egui::Sense::hover(),
                                );
                                let painter = ui.painter_at(icon_rect);
                                let stroke = egui::Stroke::new(2.0_f32, color);
                                let left = icon_rect.left() + 3.0;
                                let right = icon_rect.right() - 3.0;
                                let top = icon_rect.top() + 3.0;
                                let bottom = icon_rect.bottom() - 3.0;

                                match item.kind {
                                    AssemblyItemKind::Part => {
                                        painter.circle_filled(icon_rect.center(), 4.0, color);
                                    }
                                    AssemblyItemKind::SheetMetal => {
                                        painter.line_segment(
                                            [egui::pos2(left, top), egui::pos2(left, bottom)],
                                            stroke,
                                        );
                                        painter.line_segment(
                                            [egui::pos2(left, bottom), egui::pos2(right, bottom)],
                                            stroke,
                                        );
                                    }
                                    AssemblyItemKind::Assembly => {
                                        painter.line_segment(
                                            [egui::pos2(left, top), egui::pos2(right, top)],
                                            stroke,
                                        );
                                        painter.line_segment(
                                            [egui::pos2(right, top), egui::pos2(right, bottom)],
                                            stroke,
                                        );
                                        painter.line_segment(
                                            [egui::pos2(right, bottom), egui::pos2(left, bottom)],
                                            stroke,
                                        );
                                        painter.line_segment(
                                            [egui::pos2(left, bottom), egui::pos2(left, top)],
                                            stroke,
                                        );
                                    }
                                }
                                (icon_response, ui.label(label))
                            })
                            .inner;

                        if type_response.0.hovered() || type_response.1.hovered() {
                            hovered_thumbnail = thumbnail
                                .cloned()
                                .map(|texture| (item.name.clone(), texture));
                        }
                        let name_response = ui.label(&item.name);
                        if name_response.hovered() {
                            hovered_thumbnail = thumbnail
                                .cloned()
                                .map(|texture| (item.name.clone(), texture));
                        }
                        let path_response = ui.label(&item.path);
                        if path_response.hovered() {
                            hovered_thumbnail = thumbnail
                                .cloned()
                                .map(|texture| (item.name.clone(), texture));
                        }
                        let quantity_response = ui.label(item.quantity.to_string());
                        if quantity_response.hovered() {
                            hovered_thumbnail = thumbnail
                                .cloned()
                                .map(|texture| (item.name.clone(), texture));
                        }
                        ui.end_row();
                    }
                });
        });
    } else if tab.assembly_loaded && tab.assembly_status.is_none() {
        ui.label("Aucun composant trouvé dans cet assemblage.");
    }

    (actions, hovered_thumbnail)
}
