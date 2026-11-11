use std::thread;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

use egui::{Align, CentralPanel, Layout, ScrollArea, TopBottomPanel};
use rfd::FileDialog;

use crate::solidworks::{
    AssemblyItem, AssemblyTreeNode, ComApartment, CustomProperty, SolidWorksApp,
};

mod assembly_parts;
mod solidworks_properties;

type OperationResult = std::result::Result<WorkerOutcome, String>;
type TaskResult = (u64, OperationResult);

const MAIN_TAB_ID: u64 = 0;

#[derive(Clone, Copy)]
enum TabKind {
    Home,
    SolidWorksProperties,
    AssemblyParts,
}

struct AppTab {
    id: u64,
    title: String,
    kind: TabKind,
    assembly_file_path: String,
    assembly_parts: Vec<AssemblyItem>,
    assembly_tree: Option<AssemblyTreeNode>,
    assembly_tree_view: bool,
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
    /// Document ouvert et activé dans SolidWorks.
    SolidWorksDocumentActivated(String),

    /// Arborescence et liste aplatie chargées pour le chemin indiqué.
    AssemblyTreeLoaded(String, AssemblyTreeNode, Vec<AssemblyItem>),
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
    home_texture: Option<egui::TextureHandle>,
    result_receiver: Option<Receiver<TaskResult>>,
}

impl PropertiesGui {
    /// Crée l'interface graphique avec un état initial vide.
    pub fn new() -> Self {
        Self {
            tabs: vec![
                AppTab {
                    id: MAIN_TAB_ID,
                    title: "Home".to_owned(),
                    kind: TabKind::Home,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_tree: None,
                    assembly_tree_view: false,
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                },
                AppTab {
                    id: MAIN_TAB_ID + 1,
                    title: "Solidworks property".to_owned(),
                    kind: TabKind::SolidWorksProperties,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_tree: None,
                    assembly_tree_view: false,
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                },
                AppTab {
                    id: MAIN_TAB_ID + 2,
                    title: "Liste assemblage".to_owned(),
                    kind: TabKind::AssemblyParts,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_tree: None,
                    assembly_tree_view: false,
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                },
            ],
            active_tab_id: MAIN_TAB_ID,
            file_path: String::new(),
            loaded_file_path: None,
            properties: Vec::new(),
            original_properties: Vec::new(),
            status: None,
            status_is_error: false,
            dark_mode: false,
            is_processing: false,
            processing_tab_id: None,
            has_processed: false,
            fonts_scaled: false,
            home_texture: None,
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

    /// Charge l'arbre et la liste aplatie des composants d'un assemblage.
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
        tab.assembly_tree = None;
        tab.assembly_tree_view = false;
        tab.assembly_loaded = false;
        tab.assembly_status = None;
        self.start_worker(tab_id, context, move || {
            let (tree, parts) = read_assembly_tree_on_worker(&file_path)?;
            Ok(WorkerOutcome::AssemblyTreeLoaded(file_path, tree, parts))
        });
    }

    /// Ouvre un composant dans SolidWorks sans fermer l'application à la fin de l'opération.
    fn open_component_in_solidworks(
        &mut self,
        tab_id: u64,
        file_path: String,
        context: &egui::Context,
    ) {
        if self.is_processing {
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.assembly_status = None;
            tab.assembly_status_is_error = false;
        }

        self.start_worker(tab_id, context, move || {
            open_document_in_solidworks_on_worker(&file_path)?;
            Ok(WorkerOutcome::SolidWorksDocumentActivated(file_path))
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
            tab.assembly_tree = None;
            tab.assembly_tree_view = false;
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
    fn poll_worker_result(&mut self) {
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
                Ok(WorkerOutcome::SolidWorksDocumentActivated(file_path)) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.assembly_status =
                            Some(format!("Document activé dans SolidWorks : {file_path}"));
                        tab.assembly_status_is_error = false;
                    }
                }

                Ok(WorkerOutcome::AssemblyTreeLoaded(file_path, tree, parts)) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.assembly_file_path = file_path;
                        tab.assembly_parts = parts;
                        tab.assembly_tree = Some(tree);
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

/// Ouvre un document et met SolidWorks au premier plan sur le thread COM.
fn open_document_in_solidworks_on_worker(file_path: &str) -> std::result::Result<(), String> {
    run_solidworks_operation_without_shutdown(|sw_app| sw_app.open_file_and_activate(file_path))
}

/// Initialise COM et lit l'arbre et la liste aplatie sur le thread de travail.
fn read_assembly_tree_on_worker(
    file_path: &str,
) -> std::result::Result<(AssemblyTreeNode, Vec<AssemblyItem>), String> {
    run_solidworks_operation(|sw_app| sw_app.get_assembly_tree(file_path))
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
    run_solidworks_operation_with_shutdown(operation, true)
}

/// Exécute une opération SolidWorks sans fermer l'instance après son exécution.
fn run_solidworks_operation_without_shutdown<T>(
    operation: impl FnOnce(&SolidWorksApp) -> windows::core::Result<T>,
) -> std::result::Result<T, String> {
    run_solidworks_operation_with_shutdown(operation, false)
}

/// Initialise COM et choisit si l'instance SolidWorks doit être arrêtée après l'opération.
fn run_solidworks_operation_with_shutdown<T>(
    operation: impl FnOnce(&SolidWorksApp) -> windows::core::Result<T>,
    shutdown_after: bool,
) -> std::result::Result<T, String> {
    let com_apartment = ComApartment::initialize()
        .map_err(|error| format!("Initialisation COM impossible : {error:?}"))?;
    let sw_app = SolidWorksApp::connect()
        .map_err(|error| format!("Connexion à SolidWorks impossible : {error:?}"))?;

    let operation_result = operation(&sw_app).map_err(|error| format!("{error:?}"));
    let shutdown_result = if shutdown_after {
        sw_app
            .shutdown()
            .map_err(|error| format!("Fermeture de SolidWorks impossible : {error:?}"))
    } else {
        Ok(())
    };

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

/// Renvoie un fichier déposé dans la fenêtre si son extension est acceptée.
///
/// egui ne fournit pas toujours la position du pointeur pendant un dépôt natif; l'appelant
/// n'invoque cette fonction que pour le champ de chemin de l'onglet actif.
pub(super) fn dropped_file_path(ui: &egui::Ui, accepted_extensions: &[&str]) -> Option<PathBuf> {
    ui.input(|input| {
        input.raw.dropped_files.iter().find_map(|file| {
            let path = file.path.as_ref()?;
            let extension = Path::new(path).extension()?.to_str()?;
            accepted_extensions
                .iter()
                .any(|accepted| extension.eq_ignore_ascii_case(accepted))
                .then(|| path.clone())
        })
    })
}

/// Décode l'image PNG intégrée et la charge une fois comme texture egui.
fn load_home_texture(context: &egui::Context) -> egui::TextureHandle {
    let image = image::load_from_memory(include_bytes!("../images/evolution.png"))
        .expect("L'image intégrée de l'onglet Home doit être un PNG valide")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    context.load_texture("home-evolution", color_image, egui::TextureOptions::LINEAR)
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

        self.poll_worker_result();
        if self.home_texture.is_none() {
            self.home_texture = Some(load_home_texture(context));
        }
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
        let mut assembly_browse_tab_id = None;
        let mut assembly_process_tab_id = None;
        let mut open_component = None;
        let mut show_component_properties = None;
        let mut assembly_export_tab_id = None;
        let mut assembly_dxf_export_tab_id = None;
        let mut assembly_path_changed_tab_id = None;

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
                        });
                    });
            });
        });

        if let Some(tab_id) = selected_tab_id {
            self.active_tab_id = tab_id;
        }

        let active_tab = self.tabs.iter().find(|tab| tab.id == self.active_tab_id);
        let active_tab_kind = active_tab.map_or(TabKind::SolidWorksProperties, |tab| tab.kind);

        let is_processing = self.is_processing;
        let active_tab_id = self.active_tab_id;
        let processing_active_tab = is_processing && self.processing_tab_id == Some(active_tab_id);
        let processing_other_tab = is_processing && self.processing_tab_id != Some(active_tab_id);
        let status: Option<(String, Option<egui::Color32>)> = match active_tab_kind {
            TabKind::Home => None,
            TabKind::SolidWorksProperties => self
                .status
                .as_ref()
                .map(|status| {
                    (
                        status.clone(),
                        Some(if self.status_is_error {
                            egui::Color32::LIGHT_RED
                        } else {
                            egui::Color32::LIGHT_GREEN
                        }),
                    )
                })
                .or_else(|| {
                    self.has_changes()
                        .then(|| ("Modifications non enregistrées.".to_owned(), None))
                }),
            TabKind::AssemblyParts => self
                .tabs
                .iter()
                .find(|tab| tab.id == active_tab_id)
                .and_then(|tab| {
                    tab.assembly_status.as_ref().map(|status| {
                        (
                            status.clone(),
                            Some(if tab.assembly_status_is_error {
                                egui::Color32::LIGHT_RED
                            } else {
                                egui::Color32::LIGHT_GREEN
                            }),
                        )
                    })
                }),
        };

        TopBottomPanel::bottom("operation_status")
            .exact_height(30.0)
            .show(context, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if processing_active_tab {
                        ui.spinner();
                        ui.label(
                            if matches!(active_tab_kind, TabKind::SolidWorksProperties) {
                                "Traitement des propriétés en cours…"
                            } else {
                                "Opération SolidWorks en cours…"
                            },
                        );
                    } else if processing_other_tab {
                        ui.label("Une autre opération est en cours…");
                    } else if let Some((status, Some(color))) = &status {
                        ui.colored_label(*color, status);
                    } else if let Some((status, None)) = &status {
                        ui.label(status);
                    }
                });
            });

        CentralPanel::default().show(context, |ui| match active_tab_kind {
            TabKind::Home => {
                if let Some(texture) = &self.home_texture {
                    let title = "SolidWorks Connector";
                    let subtitle = "Application Vibe Coder avec Gpt 6.0 Luna";
                    let joke = "Pour tous problèmes, merci de vous rapprocher d'une IA";
                    let image_size = texture.size_vec2();
                    let available_size = ui.available_size();
                    let title_height = ui.text_style_height(&egui::TextStyle::Heading);
                    let spacing = ui.spacing().item_spacing.y;
                    let image_height_available =
                        (available_size.y - title_height - spacing).max(0.0);
                    let scale = (available_size.x / image_size.x)
                        .min(image_height_available / image_size.y)
                        .min(1.0);
                    let displayed_image_size = image_size * scale;
                    let group_height = title_height + spacing + displayed_image_size.y;
                    ui.add_space(((available_size.y - group_height) * 0.5).max(0.0));
                    ui.vertical_centered(|ui| {
                        ui.heading(egui::RichText::new(title).italics().size(40.0));
                        ui.add_space(spacing);
                        ui.add(
                            egui::Image::new((texture.id(), displayed_image_size))
                                .corner_radius(egui::CornerRadius::same(10)),
                        );
                        ui.add_space(spacing);
                        ui.label(subtitle);
                        ui.add_space(spacing);
                        ui.label(joke);
                        ui.hyperlink_to("github link", "https://github.com/payne63/SWCom");
                    });
                }
            }
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
                    let actions = assembly_parts::show(ui, tab, is_processing);
                    if actions.browse_clicked {
                        assembly_browse_tab_id = Some(tab.id);
                    }
                    if actions.process_clicked {
                        assembly_process_tab_id = Some(tab.id);
                    }
                    if let Some(file_path) = actions.open_in_solidworks {
                        open_component = Some((tab.id, file_path));
                    }
                    if let Some(file_path) = actions.show_properties {
                        show_component_properties = Some(file_path);
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
                }
            }
        });

        if file_path_changed {
            self.clear_loaded_properties();
        }
        if let Some(tab_id) = assembly_path_changed_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.assembly_parts.clear();
                tab.assembly_tree = None;
                tab.assembly_tree_view = false;
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
                    match crate::helper::XlsxHelper::export_assembly_parts(
                        &tab.assembly_parts,
                        &path,
                    ) {
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

        if let Some((tab_id, file_path)) = open_component {
            self.open_component_in_solidworks(tab_id, file_path, context);
        } else if let Some(file_path) = show_component_properties {
            self.active_tab_id = MAIN_TAB_ID + 1;
            self.file_path = file_path;
            self.clear_loaded_properties();
            self.process_file(context);
        } else if let Some(tab_id) = assembly_dxf_export_tab_id {
            self.export_assembly_sheet_metal(tab_id, context);
        } else if process_clicked {
            self.process_file(context);
        } else if update_clicked {
            self.update_file(context);
        } else if let Some(tab_id) = assembly_process_tab_id {
            self.process_assembly_tab(tab_id, context);
        }
    }
}
