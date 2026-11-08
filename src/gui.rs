use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use egui::{Align, CentralPanel, Color32, Grid, Layout, ScrollArea, TextEdit, TopBottomPanel};
use rfd::FileDialog;

use crate::solidworks::{ComApartment, CustomProperty, SolidWorksApp};

/// Interface de sélection d'un fichier et d'affichage de ses propriétés SolidWorks.
pub struct PropertiesGui {
    file_path: String,
    properties: Vec<CustomProperty>,
    status: Option<String>,
    dark_mode: bool,
    is_processing: bool,
    has_processed: bool,
    result_receiver: Option<Receiver<std::result::Result<Vec<CustomProperty>, String>>>,
}

impl PropertiesGui {
    /// Crée l'interface graphique avec un état initial vide.
    pub fn new() -> Self {
        Self {
            file_path: String::new(),
            properties: Vec::new(),
            status: None,
            dark_mode: false,
            is_processing: false,
            has_processed: false,
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
            self.properties.clear();
            self.status = None;
            self.has_processed = false;
        }
    }

    /// Démarre la lecture des propriétés sur un thread de travail et rend la main à l'interface.
    fn process_file(&mut self, context: &egui::Context) {
        if self.is_processing {
            return;
        }
        if self.file_path.trim().is_empty() {
            self.status = Some("Sélectionnez d'abord un fichier SolidWorks.".to_owned());
            return;
        }

        self.properties.clear();
        self.status = None;
        self.is_processing = true;
        self.has_processed = false;

        let file_path = self.file_path.clone();
        let repaint_context = context.clone();
        let (sender, receiver) = mpsc::channel();
        self.result_receiver = Some(receiver);

        let worker = thread::Builder::new()
            .name("solidworks-properties".to_owned())
            .spawn(move || {
                let result = read_properties_on_worker(&file_path);
                let _ = sender.send(result);
                repaint_context.request_repaint();
            });

        if let Err(error) = worker {
            self.result_receiver = None;
            self.is_processing = false;
            self.status = Some(format!("Impossible de démarrer la tâche : {error}"));
        }
    }

    /// Transfère à l'interface le résultat de la tâche lorsqu'il est disponible.
    fn poll_worker_result(&mut self) {
        let result = self
            .result_receiver
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("La tâche de lecture s'est interrompue.".to_owned()))
                }
            });

        if let Some(result) = result {
            self.result_receiver = None;
            self.is_processing = false;
            self.has_processed = true;
            match result {
                Ok(properties) => self.properties = properties,
                Err(error) => {
                    self.properties.clear();
                    self.status = Some(error);
                }
            }
        }
    }
}

/// Initialise COM et lit les propriétés sur le thread de travail.
///
/// L'objet SolidWorks est créé et détruit sur ce thread afin de respecter son appartement STA.
///
/// # Errors
/// Renvoie un message si l'initialisation COM, la connexion à SolidWorks ou la lecture échoue.
fn read_properties_on_worker(file_path: &str) -> std::result::Result<Vec<CustomProperty>, String> {
    let com_apartment = ComApartment::initialize()
        .map_err(|error| format!("Initialisation COM impossible : {error:?}"))?;
    let sw_app = SolidWorksApp::connect()
        .map_err(|error| format!("Connexion à SolidWorks impossible : {error:?}"))?;

    let properties_result = sw_app
        .get_file_properties(file_path)
        .map_err(|error| format!("Lecture des propriétés impossible : {error:?}"));
    let shutdown_result = sw_app
        .shutdown()
        .map_err(|error| format!("Fermeture de SolidWorks impossible : {error:?}"));

    drop(sw_app);
    drop(com_apartment);

    match properties_result {
        Ok(properties) => {
            shutdown_result?;
            Ok(properties)
        }
        Err(error) => Err(error),
    }
}

impl eframe::App for PropertiesGui {
    /// Dessine la fenêtre et traite les actions de sélection et de lecture.
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker_result();
        context.set_visuals(if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        let mut browse_clicked = false;
        let mut process_clicked = false;

        TopBottomPanel::top("top_bar").show(context, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button_label = if self.dark_mode {
                    "☀ Mode clair"
                } else {
                    "🌙 Mode sombre"
                };
                if ui.button(button_label).clicked() {
                    self.dark_mode = !self.dark_mode;
                    context.set_visuals(if self.dark_mode {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                }
            });
        });

        CentralPanel::default().show(context, |ui| {
            ui.heading("Propriétés SolidWorks");
            ui.label("Choisissez une pièce (.sldprt) ou un assemblage (.sldasm).");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("Fichier :");
                ui.add_enabled(
                    !self.is_processing,
                    TextEdit::singleline(&mut self.file_path).desired_width(f32::INFINITY),
                );
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
            });

            if self.is_processing {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Lecture des propriétés en cours…");
                });
            } else if let Some(status) = &self.status {
                ui.colored_label(Color32::LIGHT_RED, status);
            }

            if !self.properties.is_empty() {
                ui.separator();
                ScrollArea::vertical().show(ui, |ui| {
                    Grid::new("custom_properties")
                        .striped(true)
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.strong("Propriété");
                            ui.strong("Valeur");
                            ui.end_row();

                            for property in &self.properties {
                                ui.label(&property.name);
                                ui.label(&property.value);
                                ui.end_row();
                            }
                        });
                });
            } else if self.has_processed && self.status.is_none() {
                ui.label("Aucune propriété personnalisée trouvée.");
            }
        });

        if browse_clicked {
            self.select_file();
        }
        if process_clicked {
            self.process_file(context);
        }
    }
}
