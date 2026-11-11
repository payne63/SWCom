use std::{os::windows::ffi::OsStrExt, path::Path};

use rust_xlsxwriter::Workbook;
use windows::{
    core::PCWSTR,
    Win32::Foundation::HWND,
    Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

use crate::solidworks::{AssemblyItem, AssemblyItemKind};

/// Utilitaire de génération de classeurs Excel.
pub(crate) struct XlsxHelper;

impl XlsxHelper {
    /// Ouvre un fichier avec l'application associée par Windows à son extension.
    ///
    /// # Errors
    /// Retourne une erreur si Windows ne peut pas lancer l'application associée.
    pub(crate) fn open_file(path: &Path) -> Result<(), String> {
        let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();

        // ShellExecuteW retourne une valeur supérieure à 32 en cas de succès.
        let result = unsafe {
            ShellExecuteW(
                HWND(0),
                PCWSTR(operation.as_ptr()),
                PCWSTR(path_wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };

        if result.0 as isize <= 32 {
            Err(format!(
                "Windows n'a pas pu ouvrir le fichier (code {}).",
                result.0 as isize
            ))
        } else {
            Ok(())
        }
    }

    /// Exporte la liste des composants d'un assemblage dans un fichier `.xlsx`.
    ///
    /// Les colonnes exportées sont le type, le nom, le chemin et la quantité totale.
    ///
    /// # Errors
    /// Retourne une erreur si une cellule ou le classeur ne peut pas être écrit.
    pub(crate) fn export_assembly_parts(
        items: &[AssemblyItem],
        path: &Path,
    ) -> Result<(), String> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();
        worksheet
            .set_column_width(0, 30)
            .map_err(|error| error.to_string())?;
        worksheet
            .set_column_width(1, 30)
            .map_err(|error| error.to_string())?;
        worksheet
            .set_column_width(2, 30)
            .map_err(|error| error.to_string())?;
        worksheet
            .set_column_width(3, 20)
            .map_err(|error| error.to_string())?;

        let format = &rust_xlsxwriter::Format::new().set_bold();

        worksheet
            .write_with_format(0, 0, "Type", format)
            .map_err(|error| error.to_string())?;
        worksheet
            .write_with_format(0, 1, "Nom", format)
            .map_err(|error| error.to_string())?;
        worksheet
            .write_with_format(0, 2, "Emplacement", format)
            .map_err(|error| error.to_string())?;
        worksheet
            .write_with_format(0, 3, "Quantité totale", format)
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
