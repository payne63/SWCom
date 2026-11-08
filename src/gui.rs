use std::thread;
use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver, TryRecvError},
};

use egui::{Align, CentralPanel, Color32, Grid, Layout, ScrollArea, TextEdit, TopBottomPanel};
use rfd::FileDialog;

use crate::solidworks::{
    AssemblyItem, AssemblyItemKind, ComApartment, CustomProperty, SolidWorksApp,
};

type OperationResult = std::result::Result<WorkerOutcome, String>;
type TaskResult = (u64, OperationResult);

const MAIN_TAB_ID: u64 = 0;

#[derive(Clone, Copy)]
enum TabKind {
    SolidWorksProperties,
    AssemblyParts,
}

struct AppTab {
    id: u64,
    title: String,
    kind: TabKind,
    assembly_file_path: String,
    assembly_parts: Vec<AssemblyItem>,
    assembly_thumbnails: HashMap<String, egui::TextureHandle>,
    assembly_loaded: bool,
    assembly_status: Option<String>,
    assembly_status_is_error: bool,
}

/// Résultat métier transmis par le thread de travail à l'interface.
enum WorkerOutcome {
    /// Propriétés chargées pour le chemin indiqué.
    PropertiesLoaded(String, Vec<CustomProperty>),
    /// Propriétés modifiées et sauvegardées.
    PropertiesUpdated,
    /// Composants chargés pour le chemin d'assemblage indiqué.
    AssemblyComponentsLoaded(String, Vec<AssemblyItem>),
}

/// Interface de sélection d'un fichier et d'affichage de ses propriétés SolidWorks.
pub struct PropertiesGui {
    tabs: Vec<AppTab>,
    active_tab_id: u64,
    next_tab_id: u64,
    file_path: String,
    loaded_file_path: Option<String>,
    properties: Vec<CustomProperty>,
    original_properties: Vec<CustomProperty>,
    status: Option<String>,
    status_is_error: bool,
    dark_mode: bool,
    is_processing: bool,
    processing_tab_id: Option<u64>,
    has_processed: bool,
    fonts_scaled: bool,
    result_receiver: Option<Receiver<TaskResult>>,
}

impl PropertiesGui {
    /// Crée l'interface graphique avec un état initial vide.
    pub fn new() -> Self {
        Self {
            tabs: vec![
                AppTab {
                    id: MAIN_TAB_ID,
                    title: "Solidworks property".to_owned(),
                    kind: TabKind::SolidWorksProperties,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_thumbnails: HashMap::new(),
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                },
                AppTab {
                    id: MAIN_TAB_ID + 1,
                    title: "Liste assemblage".to_owned(),
                    kind: TabKind::AssemblyParts,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_thumbnails: HashMap::new(),
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                },
            ],
            active_tab_id: MAIN_TAB_ID,
            next_tab_id: MAIN_TAB_ID + 2,
            file_path: String::new(),
            loaded_file_path: None,
            properties: Vec::new(),
            original_properties: Vec::new(),
            status: None,
            status_is_error: false,
            dark_mode: true,
            is_processing: false,
            processing_tab_id: None,
            has_processed: false,
            fonts_scaled: false,
            result_receiver: None,
        }
    }

    /// Ouvre la boîte de dialogue et remplace le chemin par le fichier choisi.
    fn select_file(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("Fichiers SolidWorks", &["sldprt", "sldasm"])
            .pick_file()
        {
            self.file_path = path.display().to_string();
            self.clear_loaded_properties();
        }
    }

    /// Efface les valeurs affichées et leur copie de référence.
    fn clear_loaded_properties(&mut self) {
        self.loaded_file_path = None;
        self.properties.clear();
        self.original_properties.clear();
        self.status = None;
        self.status_is_error = false;
        self.has_processed = false;
    }

    /// Démarre la lecture des propriétés sur un thread de travail.
    fn process_file(&mut self, context: &egui::Context) {
        if self.is_processing {
            return;
        }
        if self.file_path.trim().is_empty() {
            self.status = Some("Sélectionnez d'abord un fichier SolidWorks.".to_owned());
            self.status_is_error = true;
            return;
        }

        self.clear_loaded_properties();
        let file_path = self.file_path.clone();
        self.start_worker(MAIN_TAB_ID, context, move || {
            let properties = read_properties_on_worker(&file_path)?;
            Ok(WorkerOutcome::PropertiesLoaded(file_path, properties))
        });
    }

    /// Envoie au thread de travail les propriétés dont la valeur a changé.
    fn update_file(&mut self, context: &egui::Context) {
        if self.is_processing || !self.has_changes() {
            return;
        }
        let Some(file_path) = self.loaded_file_path.clone() else {
            self.status = Some("Relisez le fichier avant de modifier ses propriétés.".to_owned());
            self.status_is_error = true;
            return;
        };

        let changes: Vec<CustomProperty> = self
            .properties
            .iter()
            .zip(&self.original_properties)
            .filter(|(property, original)| property.value != original.value)
            .map(|(property, _)| property.clone())
            .collect();

        self.start_worker(MAIN_TAB_ID, context, move || {
            update_properties_on_worker(&file_path, &changes)?;
            Ok(WorkerOutcome::PropertiesUpdated)
        });
    }

    /// Lance la lecture aplatie des pièces d'un onglet d'assemblage.
    fn process_assembly_tab(&mut self, tab_id: u64, context: &egui::Context) {
        if self.is_processing {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        if tab.assembly_file_path.trim().is_empty() {
            tab.assembly_status = Some("Sélectionnez un fichier .sldasm.".to_owned());
            tab.assembly_status_is_error = true;
            return;
        }

        let file_path = tab.assembly_file_path.clone();
        tab.assembly_parts.clear();
        tab.assembly_thumbnails.clear();
        tab.assembly_loaded = false;
        tab.assembly_status = None;
        self.start_worker(tab_id, context, move || {
            let components = read_assembly_components_on_worker(&file_path)?;
            Ok(WorkerOutcome::AssemblyComponentsLoaded(
                file_path, components,
            ))
        });
    }

    /// Ouvre un sélecteur de fichier `.sldasm` pour l'onglet indiqué.
    fn select_assembly_file(&mut self, tab_id: u64) {
        let Some(path) = FileDialog::new()
            .add_filter("Assemblages SolidWorks", &["sldasm"])
            .pick_file()
        else {
            return;
        };

        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.assembly_file_path = path.display().to_string();
            tab.assembly_parts.clear();
            tab.assembly_thumbnails.clear();
            tab.assembly_loaded = false;
            tab.assembly_status = None;
            tab.assembly_status_is_error = false;
        }
    }

    /// Indique si au moins une valeur diffère de sa valeur chargée.
    fn has_changes(&self) -> bool {
        self.loaded_file_path.as_deref() == Some(self.file_path.as_str())
            && self
                .properties
                .iter()
                .zip(&self.original_properties)
                .any(|(property, original)| property.value != original.value)
    }

    /// Lance une opération bloquante sur un thread séparé et conserve son canal de résultat.
    fn start_worker<F>(&mut self, tab_id: u64, context: &egui::Context, operation: F)
    where
        F: FnOnce() -> OperationResult + Send + 'static,
    {
        if tab_id == MAIN_TAB_ID {
            self.status = None;
            self.status_is_error = false;
        } else if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.assembly_status = None;
            tab.assembly_status_is_error = false;
        }
        self.is_processing = true;
        self.processing_tab_id = Some(tab_id);

        let repaint_context = context.clone();
        let (sender, receiver) = mpsc::channel();
        self.result_receiver = Some(receiver);

        let worker = thread::Builder::new()
            .name("solidworks-properties".to_owned())
            .spawn(move || {
                let _ = sender.send((tab_id, operation()));
                repaint_context.request_repaint();
            });

        if let Err(error) = worker {
            self.result_receiver = None;
            self.is_processing = false;
            self.processing_tab_id = None;
            self.set_task_error(tab_id, format!("Impossible de démarrer la tâche : {error}"));
        }
    }

    /// Applique à l'interface le résultat de la tâche lorsqu'il est disponible.
    fn poll_worker_result(&mut self, context: &egui::Context) {
        let result = self
            .result_receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    self.processing_tab_id.unwrap_or(MAIN_TAB_ID),
                    Err("La tâche de lecture s'est interrompue.".to_owned()),
                )),
            });

        if let Some((tab_id, result)) = result {
            self.result_receiver = None;
            self.is_processing = false;
            self.processing_tab_id = None;
            if tab_id == MAIN_TAB_ID {
                self.has_processed = true;
            }

            match result {
                Ok(WorkerOutcome::PropertiesLoaded(file_path, properties))
                    if tab_id == MAIN_TAB_ID =>
                {
                    self.loaded_file_path = Some(file_path);
                    self.properties = properties.clone();
                    self.original_properties = properties;
                    self.status = None;
                    self.status_is_error = false;
                }
                Ok(WorkerOutcome::PropertiesUpdated) if tab_id == MAIN_TAB_ID => {
                    self.original_properties = self.properties.clone();
                    self.status =
                        Some("Les propriétés ont été mises à jour dans le fichier.".to_owned());
                    self.status_is_error = false;
                }
                Ok(WorkerOutcome::AssemblyComponentsLoaded(file_path, components)) => {
                    let thumbnails = load_assembly_thumbnails(context, tab_id, &components);
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.assembly_file_path = file_path;
                        tab.assembly_parts = components;
                        tab.assembly_thumbnails = thumbnails;
                        tab.assembly_loaded = true;
                        tab.assembly_status = None;
                        tab.assembly_status_is_error = false;
                    }
                }
                Ok(_) => self.set_task_error(tab_id, "Résultat inattendu de la tâche.".to_owned()),
                Err(error) => self.set_task_error(tab_id, error),
            }
        }
    }

    /// Affiche une erreur dans la page qui a lancé la tâche.
    fn set_task_error(&mut self, tab_id: u64, error: String) {
        if tab_id == MAIN_TAB_ID {
            self.status = Some(error);
            self.status_is_error = true;
        } else if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.assembly_status = Some(error);
            tab.assembly_status_is_error = true;
        }
    }
}

/// Initialise COM et lit les propriétés sur le thread de travail.
fn read_properties_on_worker(file_path: &str) -> std::result::Result<Vec<CustomProperty>, String> {
    run_solidworks_operation(|sw_app| sw_app.get_file_properties(file_path))
}

/// Initialise COM et sauvegarde les valeurs modifiées sur le thread de travail.
fn update_properties_on_worker(
    file_path: &str,
    properties: &[CustomProperty],
) -> std::result::Result<(), String> {
    run_solidworks_operation(|sw_app| sw_app.update_file_properties(file_path, properties))
}

/// Initialise COM et lit les composants et quantités sur le thread de travail.
fn read_assembly_components_on_worker(
    file_path: &str,
) -> std::result::Result<Vec<AssemblyItem>, String> {
    run_solidworks_operation(|sw_app| sw_app.get_assembly_components(file_path))
}

/// Exécute une opération SolidWorks dans un appartement COM STA temporaire.
fn run_solidworks_operation<T>(
    operation: impl FnOnce(&SolidWorksApp) -> windows::core::Result<T>,
) -> std::result::Result<T, String> {
    let com_apartment = ComApartment::initialize()
        .map_err(|error| format!("Initialisation COM impossible : {error:?}"))?;
    let sw_app = SolidWorksApp::connect()
        .map_err(|error| format!("Connexion à SolidWorks impossible : {error:?}"))?;

    let operation_result = operation(&sw_app).map_err(|error| format!("{error:?}"));
    let shutdown_result = sw_app
        .shutdown()
        .map_err(|error| format!("Fermeture de SolidWorks impossible : {error:?}"));

    drop(sw_app);
    drop(com_apartment);

    match operation_result {
        Ok(value) => {
            shutdown_result?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

/// Crée une texture egui pour chaque aperçu disponible dans les composants.
fn load_assembly_thumbnails(
    context: &egui::Context,
    tab_id: u64,
    components: &[AssemblyItem],
) -> HashMap<String, egui::TextureHandle> {
    components
        .iter()
        .filter_map(|component| {
            let thumbnail = component.thumbnail.as_ref()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [thumbnail.width, thumbnail.height],
                &thumbnail.rgba,
            );
            let texture = context.load_texture(
                format!("assembly-thumbnail-{tab_id}-{}", component.path),
                image,
                egui::TextureOptions::LINEAR,
            );
            Some((component.path.to_lowercase(), texture))
        })
        .collect()
}

impl eframe::App for PropertiesGui {
    /// Dessine la fenêtre et traite les actions de sélection, lecture et sauvegarde.
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.fonts_scaled {
            let mut style = (*context.style()).clone();
            for font in style.text_styles.values_mut() {
                font.size *= 1.2;
            }
            context.set_style(style);
            self.fonts_scaled = true;
        }

        self.poll_worker_result(context);
        context.set_visuals(if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        let mut browse_clicked = false;
        let mut process_clicked = false;
        let mut update_clicked = false;
        let mut file_path_changed = false;
        let mut selected_tab_id = None;
        let mut add_tab_clicked = false;
        let mut assembly_browse_tab_id = None;
        let mut assembly_process_tab_id = None;
        let mut assembly_path_changed_tab_id = None;
        let mut hovered_thumbnail: Option<(String, egui::TextureHandle)> = None;

        TopBottomPanel::top("app_toolbar").show(context, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (icon, tooltip) = if self.dark_mode {
                    ("☀", "Passer en mode clair")
                } else {
                    ("🌙", "Passer en mode sombre")
                };
                let theme_button = ui.button(icon).on_hover_text(tooltip);
                if theme_button.clicked() {
                    self.dark_mode = !self.dark_mode;
                    context.set_visuals(if self.dark_mode {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                }
                ui.separator();

                ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            for tab in &self.tabs {
                                if ui
                                    .selectable_label(self.active_tab_id == tab.id, &tab.title)
                                    .clicked()
                                {
                                    selected_tab_id = Some(tab.id);
                                }
                            }
                            add_tab_clicked = ui.button("+").clicked();
                        });
                    });
            });
        });

        if let Some(tab_id) = selected_tab_id {
            self.active_tab_id = tab_id;
        }

        if add_tab_clicked {
            let tab_id = self.next_tab_id;
            self.next_tab_id += 1;
            self.tabs.push(AppTab {
                id: tab_id,
                title: "Liste assemblage".to_owned(),
                kind: TabKind::AssemblyParts,
                assembly_file_path: String::new(),
                assembly_parts: Vec::new(),
                assembly_thumbnails: HashMap::new(),
                assembly_loaded: false,
                assembly_status: None,
                assembly_status_is_error: false,
            });
            self.active_tab_id = tab_id;
        }

        let active_tab = self.tabs.iter().find(|tab| tab.id == self.active_tab_id);
        let active_tab_kind = active_tab.map_or(TabKind::SolidWorksProperties, |tab| tab.kind);

        let is_processing = self.is_processing;
        let processing_tab_id = self.processing_tab_id;
        CentralPanel::default().show(context, |ui| match active_tab_kind {
            TabKind::SolidWorksProperties => {
                ui.heading("Propriétés SolidWorks");
                ui.label("Choisissez une pièce (.sldprt) ou un assemblage (.sldasm).");
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Fichier :");
                    file_path_changed = ui
                        .add_enabled(
                            !self.is_processing,
                            TextEdit::singleline(&mut self.file_path).desired_width(f32::INFINITY),
                        )
                        .changed();
                });

                ui.horizontal(|ui| {
                    browse_clicked = ui
                        .add_enabled(!self.is_processing, egui::Button::new("Parcourir…"))
                        .clicked();
                    process_clicked = ui
                        .add_enabled(
                            !self.is_processing && !self.file_path.trim().is_empty(),
                            egui::Button::new("Lire les propriétés"),
                        )
                        .clicked();
                    update_clicked = ui
                        .add_enabled(
                            !self.is_processing && self.has_changes(),
                            egui::Button::new("Mettre à jour dans SolidWorks"),
                        )
                        .clicked();
                });

                if self.is_processing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Traitement en cours…");
                    });
                } else {
                    let (status_text, status_color) = if let Some(status) = &self.status {
                        let color = if self.status_is_error {
                            Color32::LIGHT_RED
                        } else {
                            Color32::LIGHT_GREEN
                        };
                        (status.as_str(), color)
                    } else if self.has_changes() {
                        ("Modifications non enregistrées.", ui.visuals().text_color())
                    } else {
                        (" ", ui.visuals().text_color())
                    };
                    ui.colored_label(status_color, status_text);
                }

                if !self.properties.is_empty() {
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

                                for (index, property) in self.properties.iter_mut().enumerate() {
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
                } else if self.has_processed && self.status.is_none() {
                    ui.label("Aucune propriété personnalisée trouvée.");
                }
            }
            TabKind::AssemblyParts => {
                ui.heading("Liste a plat des composants");
                ui.label(
                    "Les pièces, sous-assemblages et l'assemblage maître sont regroupés avec leurs quantités.",
                );
                ui.add_space(8.0);

                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == self.active_tab_id)
                {
                    ui.horizontal(|ui| {
                        ui.label("Assemblage :");
                        let path_response = ui.add_enabled(
                            !is_processing,
                            TextEdit::singleline(&mut tab.assembly_file_path)
                                .desired_width(f32::INFINITY),
                        );
                        if path_response.changed() {
                            assembly_path_changed_tab_id = Some(tab.id);
                        }
                    });

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!is_processing, egui::Button::new("Parcourir…"))
                            .clicked()
                        {
                            assembly_browse_tab_id = Some(tab.id);
                        }
                        if ui
                            .add_enabled(
                                !is_processing && !tab.assembly_file_path.trim().is_empty(),
                                egui::Button::new("Afficher les composants"),
                            )
                            .clicked()
                        {
                            assembly_process_tab_id = Some(tab.id);
                        }
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
                                        let thumbnail = tab
                                            .assembly_thumbnails
                                            .get(&item.path.to_lowercase());
                                        let (label, color) = match item.kind {
                                            AssemblyItemKind::Part => {
                                                ("Pièce", Color32::LIGHT_BLUE)
                                            }
                                            AssemblyItemKind::SheetMetal => {
                                                ("Tôlerie", Color32::LIGHT_YELLOW)
                                            }
                                            AssemblyItemKind::Assembly => {
                                                ("Assemblage", Color32::LIGHT_GREEN)
                                            }
                                        };
                                        let type_response = ui.with_layout(
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                let icon_size = ui.text_style_height(
                                                    &egui::TextStyle::Body,
                                                );
                                                let (icon_rect, icon_response) =
                                                    ui.allocate_exact_size(
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
                                                        painter.circle_filled(
                                                            icon_rect.center(),
                                                            4.0,
                                                            color,
                                                        );
                                                    }
                                                    AssemblyItemKind::SheetMetal => {
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(left, top),
                                                                egui::pos2(left, bottom),
                                                            ],
                                                            stroke,
                                                        );
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(left, bottom),
                                                                egui::pos2(right, bottom),
                                                            ],
                                                            stroke,
                                                        );
                                                    }
                                                    AssemblyItemKind::Assembly => {
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(left, top),
                                                                egui::pos2(right, top),
                                                            ],
                                                            stroke,
                                                        );
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(right, top),
                                                                egui::pos2(right, bottom),
                                                            ],
                                                            stroke,
                                                        );
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(right, bottom),
                                                                egui::pos2(left, bottom),
                                                            ],
                                                            stroke,
                                                        );
                                                        painter.line_segment(
                                                            [
                                                                egui::pos2(left, bottom),
                                                                egui::pos2(left, top),
                                                            ],
                                                            stroke,
                                                        );
                                                    }
                                                }
                                                (icon_response, ui.label(label))
                                            },
                                        )
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
                                        let quantity_response =
                                            ui.label(item.quantity.to_string());
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
                }
            }
        });

        if let (Some((name, texture)), Some(pointer_position)) =
            (hovered_thumbnail, context.pointer_hover_pos())
        {
            egui::Area::new(egui::Id::new("assembly_thumbnail_preview"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer_position + egui::vec2(12.0, 12.0))
                .interactable(false)
                .show(context, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(name);
                        ui.image((texture.id(), egui::vec2(288.0, 216.0)));
                    });
                });
        }

        if file_path_changed {
            self.clear_loaded_properties();
        }
        if let Some(tab_id) = assembly_path_changed_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.assembly_parts.clear();
                tab.assembly_thumbnails.clear();
                tab.assembly_loaded = false;
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
        if browse_clicked {
            self.select_file();
        }
        if let Some(tab_id) = assembly_browse_tab_id {
            self.select_assembly_file(tab_id);
        }
        if process_clicked {
            self.process_file(context);
        } else if update_clicked {
            self.update_file(context);
        } else if let Some(tab_id) = assembly_process_tab_id {
            self.process_assembly_tab(tab_id, context);
        }
    }
}
