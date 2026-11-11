#![windows_subsystem = "windows"]

mod gui;
mod helper;
mod solidworks;

/// Lance la fenêtre native de l'application.
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 540.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "SolidWorks AI Connector",
        options,
        Box::new(|_creation_context| Ok(Box::new(gui::PropertiesGui::new()))),
    )
}
