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
mod print_plans;
mod printers;
mod solidworks_properties;
mod step_export;

type OperationResult = std::result::Result<WorkerOutcome, String>;
type TaskResult = (u64, OperationResult);

const MAIN_TAB_ID: u64 = 0;

#[derive(Clone, Copy)]
enum TabKind {
    Home,
    SolidWorksProperties,
    AssemblyParts,
    PrintPlans,
    StepExport,
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
    print_plan_paths: Vec<String>,
    print_plan_rows: Vec<(String, String)>,
    print_plans_visible: bool,
    step_file_paths: Vec<String>,
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
    /// Plans chargés avec leur format papier.
    PrintPlansLoaded(Vec<(String, String)>),
    /// Résultats de l'export STEP des pièces et assemblages sélectionnés.
    StepFilesExported {
        exported_count: usize,
        failures: Vec<String>,
    },
    /// Résultats de l'export PDF des plans sélectionnés.
    PdfExported {
        exported_count: usize,
        failures: Vec<String>,
    },
    /// Nombre de plans envoyés successivement à l'imprimante.
    PlansPrinted(usize),
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
    available_printers: Vec<String>,
    printer_a4_a3: String,
    printer_a2_a0: String,
}

impl PropertiesGui {
    /// Crée l'interface graphique avec un état initial vide.
    pub fn new() -> Self {
        let available_printers = printers::available_printers().unwrap_or_default();
        let default_printer = available_printers.first().cloned().unwrap_or_default();
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
                    print_plan_paths: Vec::new(),
                    print_plan_rows: Vec::new(),
                    print_plans_visible: false,
                    step_file_paths: Vec::new(),
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
                    print_plan_paths: Vec::new(),
                    print_plan_rows: Vec::new(),
                    print_plans_visible: false,
                    step_file_paths: Vec::new(),
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
                    print_plan_paths: Vec::new(),
                    print_plan_rows: Vec::new(),
                    print_plans_visible: false,
                    step_file_paths: Vec::new(),
                },
                AppTab {
                    id: MAIN_TAB_ID + 3,
                    title: "Impression Plans".to_owned(),
                    kind: TabKind::PrintPlans,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_tree: None,
                    assembly_tree_view: false,
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                    print_plan_paths: Vec::new(),
                    print_plan_rows: Vec::new(),
                    print_plans_visible: false,
                    step_file_paths: Vec::new(),
                },
                AppTab {
                    id: MAIN_TAB_ID + 4,
                    title: "Export STEP".to_owned(),
                    kind: TabKind::StepExport,
                    assembly_file_path: String::new(),
                    assembly_parts: Vec::new(),
                    assembly_tree: None,
                    assembly_tree_view: false,
                    assembly_loaded: false,
                    assembly_status: None,
                    assembly_status_is_error: false,
                    print_plan_paths: Vec::new(),
                    print_plan_rows: Vec::new(),
                    print_plans_visible: false,
                    step_file_paths: Vec::new(),
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
            available_printers,
            printer_a4_a3: default_printer.clone(),
            printer_a2_a0: default_printer,
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

    /// Exporte les dessins sélectionnés en PDF dans leurs répertoires d'origine.
    fn export_print_plan_pdfs(&mut self, tab_id: u64, context: &egui::Context) {
        let Some(paths) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.print_plan_paths.clone())
        else {
            return;
        };
        if paths.is_empty() {
            return;
        }

        self.start_worker(tab_id, context, move || {
            let (exported_count, failures) =
                run_solidworks_operation_without_shutdown(|solidworks| {
                    solidworks.export_drawing_pdfs(&paths)
                })?;
            Ok(WorkerOutcome::PdfExported {
                exported_count,
                failures,
            })
        });
    }

    /// Exporte les pièces et assemblages sélectionnés au format STEP AP214.
    fn export_step_files(&mut self, tab_id: u64, context: &egui::Context) {
        let Some(paths) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.step_file_paths.clone())
        else {
            return;
        };
        if paths.is_empty() {
            return;
        }

        self.start_worker(tab_id, context, move || {
            let (exported_count, failures) =
                run_solidworks_operation_without_shutdown(|solidworks| {
                    solidworks.export_step_files(&paths)
                })?;
            Ok(WorkerOutcome::StepFilesExported {
                exported_count,
                failures,
            })
        });
    }

    /// Ouvre un sélecteur de pièces et d'assemblages SolidWorks à ajouter à la liste.
    fn select_step_files(&mut self, tab_id: u64) {
        let paths = FileDialog::new()
            .add_filter("Pièces et assemblages SolidWorks", &["sldprt", "sldasm"])
            .pick_files();
        if let Some(paths) = paths {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                for path in paths {
                    let path = path.display().to_string();
                    if !tab.step_file_paths.contains(&path) {
                        tab.step_file_paths.push(path);
                    }
                }
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
    }

    /// Ouvre un sélecteur de plusieurs dessins SolidWorks à ajouter à la liste.
    fn select_print_files(&mut self, tab_id: u64) {
        let paths = FileDialog::new()
            .add_filter("Plans SolidWorks", &["slddrw"])
            .pick_files();
        if let Some(paths) = paths {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                let mut added_path = false;
                for path in paths {
                    let path = path.display().to_string();
                    if !tab.print_plan_paths.contains(&path) {
                        tab.print_plan_paths.push(path);
                        added_path = true;
                    }
                }
                if added_path {
                    tab.print_plan_rows = tab
                        .print_plan_paths
                        .iter()
                        .map(|path| (path.clone(), String::new()))
                        .collect();
                    tab.print_plans_visible = true;
                    tab.assembly_status = None;
                    tab.assembly_status_is_error = false;
                }
            }
        }
    }

    /// Charge les formats papier des dessins sélectionnés sur le thread SolidWorks.
    fn load_print_plans(&mut self, tab_id: u64, context: &egui::Context) {
        let Some(paths) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.print_plan_paths.clone())
        else {
            return;
        };
        if paths.is_empty() {
            return;
        }

        self.start_worker(tab_id, context, move || {
            let rows = run_solidworks_operation_without_shutdown(|solidworks| {
                let mut rows = Vec::with_capacity(paths.len());
                for path in &paths {
                    let format = solidworks.drawing_paper_size(path).map_err(|error| {
                        windows::core::Error::new(
                            error.code(),
                            format!("Lecture du format impossible pour '{path}' : {error:?}")
                                .into(),
                        )
                    })?;
                    rows.push((path.clone(), format));
                }
                Ok(rows)
            })?;
            Ok(WorkerOutcome::PrintPlansLoaded(rows))
        });
    }

    /// Lance l'impression séquentielle des plans sélectionnés.
    fn print_plans(&mut self, tab_id: u64, context: &egui::Context) {
        let Some(plans) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.print_plan_rows.clone())
        else {
            return;
        };
        if plans.is_empty() {
            return;
        }
        let a4_a3_printer = self.printer_a4_a3.clone();
        let a2_a0_printer = self.printer_a2_a0.clone();

        self.start_worker(tab_id, context, move || {
            run_solidworks_operation_without_shutdown(|solidworks| {
                for (path, format) in &plans {
                    let printer = match format.as_str() {
                        "A4" | "A3" => &a4_a3_printer,
                        "A2" | "A1" | "A0" => &a2_a0_printer,
                        _ => {
                            return Err(windows::core::Error::new(
                                windows::core::HRESULT(0x80070057_u32 as i32),
                                format!(
                                    "Format papier non pris en charge pour '{path}' : {format}"
                                )
                                .into(),
                            ));
                        }
                    };
                    solidworks.print_drawing(path, printer).map_err(|error| {
                        windows::core::Error::new(
                            error.code(),
                            format!("Échec de l'impression de '{path}' : {error:?}").into(),
                        )
                    })?;
                }
                Ok(())
            })?;
            Ok(WorkerOutcome::PlansPrinted(plans.len()))
        });
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
                Ok(WorkerOutcome::PrintPlansLoaded(rows)) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.print_plan_rows = rows;
                        tab.print_plans_visible = true;
                        tab.assembly_status = None;
                        tab.assembly_status_is_error = false;
                    }
                }
                Ok(WorkerOutcome::StepFilesExported {
                    exported_count,
                    failures,
                }) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        if failures.is_empty() {
                            tab.assembly_status = Some(format!(
                                "{exported_count} fichier(s) STEP AP214 exporté(s) dans les répertoires d'origine."
                            ));
                            tab.assembly_status_is_error = false;
                        } else {
                            let first_failure = &failures[0];
                            tab.assembly_status = Some(format!(
                                "{exported_count} STEP créé(s), {} échec(s). Premier échec : {first_failure}",
                                failures.len()
                            ));
                            tab.assembly_status_is_error = true;
                        }
                    }
                }
                Ok(WorkerOutcome::PdfExported {
                    exported_count,
                    failures,
                }) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        if failures.is_empty() {
                            tab.assembly_status = Some(format!(
                                "{exported_count} PDF exporté(s) dans le répertoire d'origine des plans."
                            ));
                            tab.assembly_status_is_error = false;
                        } else {
                            let first_failure = &failures[0];
                            tab.assembly_status = Some(format!(
                                "{exported_count} PDF créé(s), {} échec(s). Premier échec : {first_failure}",
                                failures.len()
                            ));
                            tab.assembly_status_is_error = true;
                        }
                    }
                }
                Ok(WorkerOutcome::PlansPrinted(count)) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.assembly_status = Some(format!(
                            "{count} plan(s) envoyé(s) à l'impression avec les réglages de SolidWorks."
                        ));
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

/// Choisit une couleur de statut lisible sur le fond du thème actif.
fn status_color(is_error: bool, dark_mode: bool) -> egui::Color32 {
    match (is_error, dark_mode) {
        (false, false) => egui::Color32::from_rgb(0, 100, 0),
        (false, true) => egui::Color32::from_rgb(130, 220, 130),
        (true, false) => egui::Color32::from_rgb(160, 0, 0),
        (true, true) => egui::Color32::from_rgb(255, 125, 125),
    }
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
        let mut print_browse_tab_id = None;
        let mut print_clear_tab_id = None;
        let mut print_show_tab_id = None;
        let mut print_tab_id = None;
        let mut print_pdf_tab_id = None;
        let mut print_paths_changed_tab_id = None;
        let mut step_browse_tab_id = None;
        let mut step_clear_tab_id = None;
        let mut step_export_tab_id = None;
        let mut step_paths_changed_tab_id = None;

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
                        Some(status_color(self.status_is_error, self.dark_mode)),
                    )
                })
                .or_else(|| {
                    self.has_changes()
                        .then(|| ("Modifications non enregistrées.".to_owned(), None))
                }),
            TabKind::AssemblyParts | TabKind::PrintPlans | TabKind::StepExport => self
                .tabs
                .iter()
                .find(|tab| tab.id == active_tab_id)
                .and_then(|tab| {
                    tab.assembly_status.as_ref().map(|status| {
                        (
                            status.clone(),
                            Some(status_color(tab.assembly_status_is_error, self.dark_mode)),
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
            TabKind::PrintPlans => {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == self.active_tab_id)
                {
                    let actions = print_plans::show(
                        ui,
                        tab,
                        &self.available_printers,
                        &mut self.printer_a4_a3,
                        &mut self.printer_a2_a0,
                        is_processing,
                    );
                    if actions.browse_clicked {
                        print_browse_tab_id = Some(tab.id);
                    }
                    if actions.clear_clicked {
                        print_clear_tab_id = Some(tab.id);
                    }
                    if actions.show_plans_clicked {
                        print_show_tab_id = Some(tab.id);
                    }
                    if actions.print_clicked {
                        print_tab_id = Some(tab.id);
                    }
                    if actions.export_pdf_clicked {
                        print_pdf_tab_id = Some(tab.id);
                    }
                    if actions.paths_changed {
                        print_paths_changed_tab_id = Some(tab.id);
                    }
                }
            }
            TabKind::StepExport => {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == self.active_tab_id)
                {
                    let actions = step_export::show(ui, tab, is_processing);
                    if actions.browse_clicked {
                        step_browse_tab_id = Some(tab.id);
                    }
                    if actions.clear_clicked {
                        step_clear_tab_id = Some(tab.id);
                    }
                    if actions.export_clicked {
                        step_export_tab_id = Some(tab.id);
                    }
                    if actions.paths_changed {
                        step_paths_changed_tab_id = Some(tab.id);
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
        if let Some(tab_id) = print_paths_changed_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.print_plan_rows = tab
                    .print_plan_paths
                    .iter()
                    .map(|path| (path.clone(), String::new()))
                    .collect();
                tab.print_plans_visible = !tab.print_plan_rows.is_empty();
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
        if let Some(tab_id) = print_clear_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.print_plan_paths.clear();
                tab.print_plan_rows.clear();
                tab.print_plans_visible = false;
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
        if let Some(tab_id) = step_paths_changed_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
        if let Some(tab_id) = step_clear_tab_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.step_file_paths.clear();
                tab.assembly_status = None;
                tab.assembly_status_is_error = false;
            }
        }
        if let Some(tab_id) = step_browse_tab_id {
            self.select_step_files(tab_id);
        }
        if let Some(tab_id) = print_browse_tab_id {
            self.select_print_files(tab_id);
        }
        if let Some(tab_id) = print_show_tab_id {
            self.load_print_plans(tab_id, context);
        }
        if let Some(tab_id) = assembly_browse_tab_id {
            self.select_assembly_file(tab_id);
        }
        if let Some(tab_id) = assembly_export_tab_id {
            let assembly_name = self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| Path::new(tab.assembly_file_path.trim()).file_stem())
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "assemblage".to_owned());
            let now = chrono::Local::now();
            let file_name = format!(
                "Extraction de {assembly_name} le {} à {}h{}.xlsx",
                now.format("%d-%m-%y"),
                now.format("%H"),
                now.format("%M")
            );

            if let Some(path) = FileDialog::new()
                .add_filter("Classeur Excel", &["xlsx"])
                .set_file_name(&file_name)
                .save_file()
            {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    match crate::helper::XlsxHelper::export_assembly_parts(
                        &tab.assembly_parts,
                        &path,
                    ) {
                        Ok(()) => match crate::helper::XlsxHelper::open_file(&path) {
                            Ok(()) => {
                                tab.assembly_status = Some(format!(
                                    "Export Excel créé et ouvert : {}",
                                    path.display()
                                ));
                                tab.assembly_status_is_error = false;
                            }
                            Err(error) => {
                                tab.assembly_status = Some(format!(
                                    "Export Excel créé, mais impossible de l'ouvrir : {error}"
                                ));
                                tab.assembly_status_is_error = true;
                            }
                        },
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
        } else if let Some(tab_id) = print_tab_id {
            self.print_plans(tab_id, context);
        } else if let Some(tab_id) = print_pdf_tab_id {
            self.export_print_plan_pdfs(tab_id, context);
        } else if let Some(tab_id) = step_export_tab_id {
            self.export_step_files(tab_id, context);
        }
    }
}
