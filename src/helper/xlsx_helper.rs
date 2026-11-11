use std::path::Path;

use rust_xlsxwriter::Workbook;

use crate::solidworks::{AssemblyItem, AssemblyItemKind};

/// Utilitaire de génération de classeurs Excel.
pub(crate) struct XlsxHelper;

impl XlsxHelper {
    /// Exporte la liste des composants d'un assemblage dans un fichier `.xlsx`.
    ///
    /// Les colonnes exportées sont le type, le nom, le chemin et la quantité totale.
    ///
    /// # Errors
    /// Retourne une erreur si une cellule ou le classeur ne peut pas être écrit.
    pub(crate) fn export_assembly_parts(items: &[AssemblyItem], path: &Path) -> Result<(), String> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        worksheet
            .write_string(0, 0, "Type")
            .map_err(|error| error.to_string())?;
        worksheet
            .write_string(0, 1, "Nom")
            .map_err(|error| error.to_string())?;
        worksheet
            .write_string(0, 2, "Chemin")
            .map_err(|error| error.to_string())?;
        worksheet
            .write_string(0, 3, "Quantité totale")
            .map_err(|error| error.to_string())?;

        for (index, item) in items.iter().enumerate() {
            let row = u32::try_from(index + 1).map_err(|_| {
                "La liste contient trop de lignes pour un classeur Excel.".to_owned()
            })?;
            let kind = match item.kind {
                AssemblyItemKind::Part => "Pièce",
                AssemblyItemKind::SheetMetal => "Tôlerie",
                AssemblyItemKind::Assembly => "Assemblage",
            };

            worksheet
                .write_string(row, 0, kind)
                .map_err(|error| error.to_string())?;
            worksheet
                .write_string(row, 1, &item.name)
                .map_err(|error| error.to_string())?;
            worksheet
                .write_string(row, 2, &item.path)
                .map_err(|error| error.to_string())?;
            worksheet
                .write_number(row, 3, item.quantity as f64)
                .map_err(|error| error.to_string())?;
        }

        workbook.save(path).map_err(|error| error.to_string())
    }
}
