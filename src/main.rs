mod solidworks;

use solidworks::{ComApartment, SolidWorksApp};
use std::env;

/// Lance la connexion SolidWorks et affiche les propriétés du fichier fourni en argument.
///
/// Si aucun chemin n'est fourni, affiche le format d'utilisation. Une instance SolidWorks
/// déjà ouverte est laissée active à la fin du programme.
fn main() -> std::result::Result<(), windows::core::Error> {
    let _com_apartment = ComApartment::initialize()?;

    println!("Connecting to SolidWorks...");
    let sw_app = SolidWorksApp::connect()?;

    if let Some(file_path) = env::args().nth(1) {
        match sw_app.get_file_properties(&file_path) {
            Ok(properties) => {
                println!("Properties for {file_path}:");
                for property in properties {
                    println!("{}: {}", property.name, property.value);
                }
            }
            Err(error) => eprintln!("❌ Failed to read file properties: {error:?}"),
        }
    } else {
        println!("Usage: solidworks_connector <file.sldprt|file.sldasm>");
    }

    if let Err(error) = sw_app.shutdown() {
        eprintln!("❌ Failed to close SolidWorks: {error:?}");
    }

    Ok(())
}
