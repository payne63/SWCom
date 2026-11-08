use std::thread;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

use egui::{Align, CentralPanel, Layout, ScrollArea, TopBottomPanel};
use rfd::FileDialog;

use crate::solidworks::{AssemblyItem, ComApartment, CustomProperty, SolidWorksApp};

mod assembly_parts;
mod solidworks_properties;

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
    /// Mises à plat DXF exportées et éventuels échecs par pièce.
    SheetMetalDxfExported {
        output_directory: String,
        exported_count: usize,
        failures: Vec<String>,
    },
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

    /// Démarre l'extraction des mises à plat des pièces de tôlerie de l'onglet.
    fn export_assembly_sheet_metal(&mut self, tab_id: u64, context: &egui::Context) {
        if self.is_processing {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        if !tab.assembly_loaded {
            return;
        }

        let assembly_path = tab.assembly_file_path.clone();
        let sheet_metal_paths = tab
            .assembly_parts
            .iter()
            .filter(|item| item.kind == crate::solidworks::AssemblyItemKind::SheetMetal)
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();

        self.start_worker(tab_id, context, move || {
            let (output_directory, exported_count, failures) =
                export_sheet_metal_dxf_on_worker(&assembly_path, &sheet_metal_paths)?;
            Ok(WorkerOutcome::SheetMetalDxfExported {
                output_directory: output_directory.display().to_string(),
                exported_count,
                failures,
            })
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
                Ok(WorkerOutcome::SheetMetalDxfExported {
                    output_directory,
                    exported_count,
                    failures,
                }) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        if failures.is_empty() {
                            tab.assembly_status = Some(if exported_count == 0 {
                                "Aucune pièce de tôlerie à exporter.".to_owned()
                            } else {
                                format!(
                                    "{exported_count} fichier(s) DXF exporté(s) dans {output_directory}"
                                )
                            });
                            tab.assembly_status_is_error = false;
                        } else {
                            let first_failure = &failures[0];
                            tab.assembly_status = Some(format!(
                                "{exported_count} DXF créé(s), {} échec(s). Premier échec : {first_failure}",
                                failures.len()
                            ));
                            tab.assembly_status_is_error = true;
                        }
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

/// Initialise COM puis exporte les mises à plat sur le thread de travail.
fn export_sheet_metal_dxf_on_worker(
    assembly_path: &str,
    sheet_metal_paths: &[String],
) -> std::result::Result<(PathBuf, usize, Vec<String>), String> {
    run_solidworks_operation(|sw_app| {
        sw_app.export_sheet_metal_dxf(assembly_path, sheet_metal_paths)
    })
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
        let mut assembly_export_tab_id = None;
        let mut assembly_dxf_export_tab_id = None;
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
                let actions = solidworks_properties::show(ui, self, is_processing);
                browse_clicked = actions.browse_clicked;
                process_clicked = actions.process_clicked;
                update_clicked = actions.update_clicked;
                file_path_changed = actions.file_path_changed;
            }
            TabKind::AssemblyParts => {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == self.active_tab_id)
                {
                    let (actions, preview) =
                        assembly_parts::show(ui, tab, is_processing, processing_tab_id);
                    if actions.browse_clicked {
                        assembly_browse_tab_id = Some(tab.id);
                    }
                    if actions.process_clicked {
                        assembly_process_tab_id = Some(tab.id);
                    }
                    if actions.export_clicked {
                        assembly_export_tab_id = Some(tab.id);
                    }
                    if actions.export_dxf_clicked {
                        assembly_dxf_export_tab_id = Some(tab.id);
                    }
                    if actions.path_changed {
                        assembly_path_changed_tab_id = Some(tab.id);
                    }
                    hovered_thumbnail = preview;
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
        if let Some(tab_id) = assembly_export_tab_id {
            if let Some(path) = FileDialog::new()
                .add_filter("Classeur Excel", &["xlsx"])
                .set_file_name("liste_assemblage.xlsx")
                .save_file()
            {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    match assembly_parts::export_to_xlsx(&tab.assembly_parts, &path) {
                        Ok(()) => {
                            tab.assembly_status =
                                Some(format!("Export Excel créé : {}", path.display()));
                            tab.assembly_status_is_error = false;
                        }
                        Err(error) => {
                            tab.assembly_status =
                                Some(format!("Échec de l'export Excel : {error}"));
                            tab.assembly_status_is_error = true;
                        }
                    }
                }
            }
        }
        if let Some(tab_id) = assembly_dxf_export_tab_id {
            self.export_assembly_sheet_metal(tab_id, context);
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
