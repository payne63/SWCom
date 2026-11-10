#![windows_subsystem = "windows"]

mod gui;
mod solidworks;

/// Lance la fenêtre native de l'application.
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 540.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SolidWorks Connector",
        options,
        Box::new(|_creation_context| Ok(Box::new(gui::PropertiesGui::new()))),
    )
}
