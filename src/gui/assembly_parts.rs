use egui::{Align, Grid, Layout, ScrollArea, TextEdit};

use crate::solidworks::{AssemblyItemKind, AssemblyTreeNode};

use super::AppTab;

/// Actions demandées par la vue de la liste des composants d'assemblage.
#[derive(Default)]
pub(super) struct Actions {
    pub(super) browse_clicked: bool,
    pub(super) process_clicked: bool,
    pub(super) export_clicked: bool,
    pub(super) export_dxf_clicked: bool,
    pub(super) path_changed: bool,
}

/// Affiche la liste aplatie des composants de l'assemblage courant.
pub(super) fn show(ui: &mut egui::Ui, tab: &mut AppTab, is_processing: bool) -> Actions {
    let mut actions = Actions::default();

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
                        !is_processing && !tab.assembly_file_path.trim().is_empty(),
                        egui::Button::new("Afficher les composants").min_size(button_size),
                    )
                    .clicked();

                actions.export_clicked = ui
                    .add_enabled(
                        !is_processing && tab.assembly_loaded,
                        egui::Button::new("Exporter en .xlsx").min_size(button_size),
                    )
                    .clicked();
                actions.export_dxf_clicked = ui
                    .add_enabled(
                        !is_processing && tab.assembly_loaded,
                        egui::Button::new("Extraire les mises à plat (.dxf)").min_size(button_size),
                    )
                    .clicked();
            },
        );

        ui.separator();
        let list_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(list_width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(list_width);
                ui.horizontal(|ui| {
                    ui.strong("Affichage :");
                    ui.add_enabled_ui(tab.assembly_loaded, |ui| {
                        ui.selectable_value(&mut tab.assembly_tree_view, false, "Liste");
                        ui.selectable_value(&mut tab.assembly_tree_view, true, "Arbre");
                    });
                });
                ui.separator();
                if tab.assembly_tree_view {
                    if let Some(tree) = &tab.assembly_tree {
                        ScrollArea::both()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                show_tree_node(ui, tree, egui::Id::new(("assembly_tree", tab.id)));
                            });
                    }
                } else if !tab.assembly_parts.is_empty() {
                    ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
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
                                        let label = match item.kind {
                                            AssemblyItemKind::Part => "Pièce",
                                            AssemblyItemKind::SheetMetal => "Tôlerie",
                                            AssemblyItemKind::Assembly => "Assemblage",
                                        };
                                        ui.with_layout(
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                show_kind_icon(ui, item.kind);
                                                ui.label(label);
                                            },
                                        );

                                        ui.label(&item.name);
                                        ui.label(&item.path);
                                        ui.label(item.quantity.to_string());
                                        ui.end_row();
                                    }
                                });
                        });
                } else if tab.assembly_loaded && tab.assembly_status.is_none() {
                    ui.label("Aucun composant trouvé dans cet assemblage.");
                }
            },
        );
    });

    actions
}

/// Affiche récursivement un nœud de l'arborescence avec son icône de type.
fn show_tree_node(ui: &mut egui::Ui, node: &AssemblyTreeNode, id: egui::Id) {
    let kind = match node.kind {
        AssemblyItemKind::Part => "Pièce",
        AssemblyItemKind::SheetMetal => "Tôlerie",
        AssemblyItemKind::Assembly => "Assemblage",
    };
    let label = format!("{kind} — {} (Qté : {})", node.name, node.quantity);

    if node.children.is_empty() {
        ui.push_id(id, |ui| {
            ui.horizontal(|ui| {
                show_kind_icon(ui, node.kind);
                ui.label(label);
            });
        });
    } else {
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                show_kind_icon(ui, node.kind);
                ui.label(label);
            })
            .body(|ui| {
                for (index, child) in node.children.iter().enumerate() {
                    show_tree_node(ui, child, id.with(index));
                }
            });
    }
}

/// Dessine l'icône d'une pièce, d'une tôlerie ou d'un assemblage avec la couleur du thème.
fn show_kind_icon(ui: &mut egui::Ui, kind: AssemblyItemKind) {
    let icon_size = ui.text_style_height(&egui::TextStyle::Body);
    let (icon_rect, _) =
        ui.allocate_exact_size(egui::vec2(icon_size, icon_size), egui::Sense::hover());
    let icon_color = ui.visuals().selection.stroke.color;
    let painter = ui.painter_at(icon_rect);
    let stroke = egui::Stroke::new(2.0_f32, icon_color);
    let left = icon_rect.left() + 3.0;
    let right = icon_rect.right() - 3.0;
    let top = icon_rect.top() + 3.0;
    let bottom = icon_rect.bottom() - 3.0;

    match kind {
        AssemblyItemKind::Part => {
            painter.circle_filled(icon_rect.center(), 4.0, icon_color);
        }
        AssemblyItemKind::SheetMetal => {
            painter.line_segment([egui::pos2(left, top), egui::pos2(left, bottom)], stroke);
            painter.line_segment(
                [egui::pos2(left, bottom), egui::pos2(right, bottom)],
                stroke,
            );
        }
        AssemblyItemKind::Assembly => {
            painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
            painter.line_segment([egui::pos2(right, top), egui::pos2(right, bottom)], stroke);
            painter.line_segment(
                [egui::pos2(right, bottom), egui::pos2(left, bottom)],
                stroke,
            );
            painter.line_segment([egui::pos2(left, bottom), egui::pos2(left, top)], stroke);
        }
    }
}
