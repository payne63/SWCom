use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use windows::core::{Error, Result, BSTR, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, VARIANT_BOOL};
use windows::Win32::Storage::Xps::{DeviceCapabilitiesW, DC_PAPERNAMES, DC_PAPERS};
use windows::Win32::System::Com::{IDispatch, DISPATCH_METHOD, DISPATCH_PROPERTYGET};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};

use super::dispatch::{
    bool_from_variant, byref_bool, byref_bstr, byref_f64, byref_i32, dispatch_array_from_variant,
    dispatch_from_variant, f64_from_variant, i32_from_variant, i64_from_variant, invoke,
    invoke_string, optional_dispatch_from_variant, set_property, string_array_from_variant,
    variant_bool, variant_bstr, variant_dispatch, variant_i32, variant_null_dispatch,
};

/// Un composant référencé par un assemblage et son nombre total d'occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyItem {
    /// Catégorie du composant : pièce ou assemblage.
    pub kind: AssemblyItemKind,
    /// Nom du fichier du composant.
    pub name: String,
    /// Chemin du fichier référencé par l'assemblage.
    pub path: String,
    /// Épaisseur nominale de la tôlerie en millimètres, arrondie à l'entier le plus proche.
    pub thickness: Option<u8>,
    /// Nombre total d'occurrences dans l'assemblage.
    pub quantity: usize,
}

/// Type de document SolidWorks référencé dans la liste d'un assemblage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssemblyItemKind {
    /// Document pièce standard (`.sldprt`).
    Part,
    /// Document pièce contenant au moins un corps de tôlerie.
    SheetMetal,
    /// Document assemblage (`.sldasm`).
    Assembly,
}

/// Nœud hiérarchique d'un assemblage, avec ses enfants regroupés par chemin de fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyTreeNode {
    /// Catégorie du document référencé.
    pub kind: AssemblyItemKind,
    /// Nom du fichier du composant.
    pub name: String,
    /// Chemin complet du document référencé.
    pub path: String,
    /// Épaisseur nominale de la tôlerie en millimètres, arrondie à l'entier le plus proche.
    pub thickness: Option<u8>,
    /// Nombre d'occurrences directes sous le parent.
    pub quantity: usize,
    /// Enfants directs de l'assemblage, ou liste vide pour une pièce.
    pub children: Vec<AssemblyTreeNode>,
}

/// Une propriété personnalisée lue dans un document SolidWorks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomProperty {
    /// Nom de la propriété dans le document.
    pub name: String,
    /// Valeur résolue de la propriété, ou sa valeur brute si aucune résolution n'est disponible.
    pub value: String,
    /// Indique si la propriété appartient à la configuration active.
    pub configuration: bool,
}

/// Lit un assemblage en arbre et retourne également sa liste aplatie pour les exports.
///
/// Les composants de même chemin sont regroupés au sein de leur assemblage parent, avec leur
/// quantité directe. Les quantités aplaties tiennent compte du nombre d'occurrences des
/// sous-assemblages.
///
/// # Errors
/// Renvoie une erreur si le chemin ne désigne pas un assemblage ou si un appel COM échoue.
pub(super) fn get_assembly_tree(
    sw_app: &IDispatch,
    file_path: &str,
) -> Result<(AssemblyTreeNode, Vec<AssemblyItem>)> {
    if document_type(file_path)? != 2 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "L'arborescence nécessite un fichier .sldasm".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, 2, 3)?;
    let tree_result = read_assembly_tree(sw_app, &document, file_path);
    if !opened_by_this_call {
        return tree_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    tree_result.and_then(|tree| {
        close_result?;
        Ok(tree)
    })
}

/// Exporte les mises à plat des pièces de tôlerie vers le dossier `dxf auto` de l'assemblage.
///
/// Chaque chemin de pièce fourni est exporté une seule fois. Les noms de fichiers DXF sont
/// rendus uniques lorsque plusieurs pièces portent le même nom de fichier.
///
/// # Errors
/// Renvoie une erreur si le chemin de l'assemblage est invalide ou si le dossier de sortie ne
/// peut pas être créé. Les erreurs propres à une pièce sont retournées dans la liste de résultats.
pub(super) fn export_sheet_metal_dxf(
    sw_app: &IDispatch,
    assembly_path: &str,
    sheet_metal_paths: &[String],
) -> Result<(PathBuf, usize, Vec<String>)> {
    if document_type(assembly_path)? != 2 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "L'export DXF nécessite un fichier .sldasm".into(),
        ));
    }

    let output_directory = Path::new(assembly_path)
        .parent()
        .ok_or_else(|| {
            Error::new(
                HRESULT(0x80070057_u32 as i32),
                "Impossible de déterminer le répertoire de l'assemblage maître".into(),
            )
        })?
        .join("dxf auto");

    if sheet_metal_paths.is_empty() {
        return Ok((output_directory, 0, Vec::new()));
    }

    fs::create_dir_all(&output_directory).map_err(|error| {
        Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!(
                "Impossible de créer le répertoire '{}': {error}",
                output_directory.display()
            )
            .into(),
        )
    })?;

    let mut used_names = HashSet::new();
    let mut exported_count = 0;
    let mut failures = Vec::new();

    for part_path in sheet_metal_paths {
        let stem = Path::new(part_path)
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("piece");
        let mut suffix = 1;
        let file_name = loop {
            let candidate = if suffix == 1 {
                format!("{stem}.dxf")
            } else {
                format!("{stem}_{suffix}.dxf")
            };
            if used_names.insert(candidate.to_lowercase()) {
                break candidate;
            }
            suffix += 1;
        };
        let output_path = output_directory.join(file_name);

        match export_sheet_metal_part(sw_app, part_path, &output_path) {
            Ok(()) => exported_count += 1,
            Err(error) => failures.push(format!("{} : {error}", Path::new(part_path).display())),
        }
    }

    Ok((output_directory, exported_count, failures))
}

/// Ouvre ou réutilise une pièce, exporte sa mise à plat, puis ferme uniquement les documents
/// qui ont été ouverts par cette opération.
fn export_sheet_metal_part(sw_app: &IDispatch, part_path: &str, output_path: &Path) -> Result<()> {
    if document_type(part_path)? != 1 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "Le composant n'est pas une pièce .sldprt".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, part_path, 1, 1)?;
    let export_result = export_flat_pattern(&document, part_path, output_path);
    if !opened_by_this_call {
        return export_result;
    }

    let close_result = close_document(sw_app, &document, part_path);
    export_result.and_then(|()| close_result)
}

/// Demande à SolidWorks de générer un DXF contenant la géométrie plane et les lignes de pliage.
fn export_flat_pattern(document: &IDispatch, model_path: &str, output_path: &Path) -> Result<()> {
    let output_path = output_path.to_string_lossy();
    let mut arguments = [
        VARIANT::default(), // Views (inutilisé pour l'export de tôlerie)
        variant_i32(5),     // Géométrie de mise à plat + lignes de pliage
        variant_bool(false),
        variant_bool(false),
        VARIANT::default(), // Alignment (alignement par défaut de SolidWorks)
        variant_bool(true),
        variant_i32(1), // swExportToDWG_ExportSheetMetal
        variant_bstr(model_path),
        variant_bstr(&output_path),
    ];
    let exported = invoke(
        document,
        "ExportToDWG2",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )?;
    if !bool_from_variant(&exported)? {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("SolidWorks a refusé l'export vers '{}'.", output_path).into(),
        ));
    }
    Ok(())
}

/// Ouvre ou réutilise un fichier SolidWorks et lit ses propriétés personnalisées générales.
///
/// Les documents `.sldprt` et `.sldasm` sont pris en charge. Un document déjà ouvert est
/// laissé ouvert; un document ouvert par cette fonction est fermé après lecture.
///
/// # Arguments
/// * `sw_app` — interface de l'application SolidWorks.
/// * `file_path` — chemin du fichier à lire.
///
/// # Errors
/// Renvoie une erreur si l'extension n'est pas prise en charge ou si une opération COM échoue.
pub(super) fn get_file_properties(
    sw_app: &IDispatch,
    file_path: &str,
) -> Result<Vec<CustomProperty>> {
    let document_type = document_type(file_path)?;
    let (document, opened_by_this_call) =
        open_or_reuse_document(sw_app, file_path, document_type, 3)?;

    let properties_result = read_document_and_configuration_properties(&document);
    // let properties_result = read_custom_properties(&document);
    if !opened_by_this_call {
        return properties_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    properties_result.and_then(|properties| {
        close_result?;
        Ok(properties)
    })
}

/// Ouvre ou réutilise un document, l'active et place la fenêtre SolidWorks au premier plan.
///
/// Le document reste ouvert après l'appel.
///
/// # Errors
/// Renvoie une erreur si le chemin est invalide ou si l'ouverture, l'activation ou la mise
/// au premier plan échoue.
/// Exporte chaque plan au format PDF à côté du fichier source.
pub(super) fn export_drawing_pdfs(
    sw_app: &IDispatch,
    drawing_paths: &[String],
) -> Result<(usize, Vec<String>)> {
    let mut exported_count = 0;
    let mut failures = Vec::new();

    for drawing_path in drawing_paths {
        let output_path = Path::new(drawing_path).with_extension("pdf");
        match export_drawing_pdf(sw_app, drawing_path, &output_path) {
            Ok(()) => exported_count += 1,
            Err(error) => {
                failures.push(format!("{} : {error:?}", Path::new(drawing_path).display()))
            }
        }
    }

    Ok((exported_count, failures))
}

fn export_drawing_pdf(sw_app: &IDispatch, drawing_path: &str, output_path: &Path) -> Result<()> {
    if document_type(drawing_path)? != 3 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "L'export PDF nécessite un fichier .slddrw".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, drawing_path, 3, 1)?;
    let export_result = export_drawing_pdf_document(sw_app, &document, output_path);
    if !opened_by_this_call {
        return export_result;
    }

    let close_result = close_document(sw_app, &document, drawing_path);
    export_result.and_then(|()| close_result)
}

fn export_drawing_pdf_document(
    sw_app: &IDispatch,
    document: &IDispatch,
    output_path: &Path,
) -> Result<()> {
    let extension = invoke(document, "Extension", DISPATCH_PROPERTYGET, &mut [], true)?;
    let extension = dispatch_from_variant(&extension)?;
    let mut export_data_arguments = [variant_i32(1)]; // swExportPdfData
    let export_data = invoke(
        sw_app,
        "GetExportFileData",
        DISPATCH_METHOD,
        &mut export_data_arguments,
        true,
    )?;
    let export_data = dispatch_from_variant(&export_data)?;

    let mut export_as_3d = variant_bool(false);
    set_property(&export_data, "ExportAs3D", &mut export_as_3d)?;
    let mut view_pdf_after_saving = variant_bool(false);
    set_property(
        &export_data,
        "ViewPdfAfterSaving",
        &mut view_pdf_after_saving,
    )?;

    let sheet_names = invoke(document, "GetSheetNames", DISPATCH_METHOD, &mut [], true)?;
    let mut set_sheets_arguments = [sheet_names, variant_i32(3)]; // ExportSpecifiedSheets
    let set_sheets_result = invoke(
        &export_data,
        "SetSheets",
        DISPATCH_METHOD,
        &mut set_sheets_arguments,
        true,
    )?;
    if !bool_from_variant(&set_sheets_result)? {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            "SolidWorks n'a pas accepté l'export de toutes les feuilles.".into(),
        ));
    }

    let mut errors = 0_i32;
    let mut warnings = 0_i32;
    let output_path = output_path.to_string_lossy();
    let mut save_arguments = [
        byref_i32(&mut warnings),
        byref_i32(&mut errors),
        variant_dispatch(&export_data),
        variant_i32(1), // swSaveAsOptions_Silent
        variant_i32(0), // swSaveAsCurrentVersion
        variant_bstr(&output_path),
    ];
    let saved = invoke(
        &extension,
        "SaveAs",
        DISPATCH_METHOD,
        &mut save_arguments,
        true,
    )?;
    if !bool_from_variant(&saved)? || errors != 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Échec de l'enregistrement PDF (erreur SolidWorks {errors}).").into(),
        ));
    }
    Ok(())
}

/// Exporte des pièces et assemblages en STEP AP214 à côté de leur fichier source.
pub(super) fn export_step_files(
    sw_app: &IDispatch,
    file_paths: &[String],
) -> Result<(usize, Vec<String>)> {
    const STEP_AP_PREFERENCE: i32 = 75;

    let mut preference_arguments = [variant_i32(STEP_AP_PREFERENCE)];
    let original_preference = invoke(
        sw_app,
        "GetUserPreferenceIntegerValue",
        DISPATCH_METHOD,
        &mut preference_arguments,
        true,
    )?;
    let original_preference = i32_from_variant(&original_preference)?;

    let mut set_preference_arguments = [variant_i32(214), variant_i32(STEP_AP_PREFERENCE)];
    let preference_set = invoke(
        sw_app,
        "SetUserPreferenceIntegerValue",
        DISPATCH_METHOD,
        &mut set_preference_arguments,
        true,
    )
    .and_then(|result| {
        if bool_from_variant(&result)? {
            Ok(())
        } else {
            Err(Error::new(
                HRESULT(0x80004005_u32 as i32),
                "SolidWorks n'a pas accepté le format STEP AP214.".into(),
            ))
        }
    });
    if let Err(error) = preference_set {
        let _ = set_step_ap_preference(sw_app, original_preference);
        return Err(error);
    }

    let mut exported_count = 0;
    let mut failures = Vec::new();
    for file_path in file_paths {
        let output_path = Path::new(file_path).with_extension("step");
        match export_step_file(sw_app, file_path, &output_path) {
            Ok(()) => exported_count += 1,
            Err(error) => failures.push(format!("{} : {error:?}", Path::new(file_path).display())),
        }
    }

    set_step_ap_preference(sw_app, original_preference)?;
    Ok((exported_count, failures))
}

fn set_step_ap_preference(sw_app: &IDispatch, value: i32) -> Result<()> {
    let mut arguments = [variant_i32(value), variant_i32(75)];
    let result = invoke(
        sw_app,
        "SetUserPreferenceIntegerValue",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )?;
    if !bool_from_variant(&result)? {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            "Impossible de restaurer la préférence STEP de SolidWorks.".into(),
        ));
    }
    Ok(())
}

fn export_step_file(sw_app: &IDispatch, file_path: &str, output_path: &Path) -> Result<()> {
    let model_type = document_type(file_path)?;
    if model_type != 1 && model_type != 2 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "L'export STEP nécessite un fichier .sldprt ou .sldasm".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, model_type, 1)?;
    let export_result = save_model_as_step(&document, output_path);
    if !opened_by_this_call {
        return export_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    export_result.and_then(|()| close_result)
}

fn save_model_as_step(document: &IDispatch, output_path: &Path) -> Result<()> {
    let extension = invoke(document, "Extension", DISPATCH_PROPERTYGET, &mut [], true)?;
    let extension = dispatch_from_variant(&extension)?;
    let mut errors = 0_i32;
    let mut warnings = 0_i32;
    let output_path = output_path.to_string_lossy();
    let mut arguments = [
        byref_i32(&mut warnings),
        byref_i32(&mut errors),
        variant_null_dispatch(), // AdvancedSaveAsOptions
        variant_null_dispatch(), // ExportData
        variant_i32(1),          // swSaveAsOptions_Silent
        variant_i32(0),          // swSaveAsCurrentVersion
        variant_bstr(&output_path),
    ];
    let saved = invoke(&extension, "SaveAs3", DISPATCH_METHOD, &mut arguments, true)?;
    if !bool_from_variant(&saved)? || errors != 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Échec SolidWorks lors de l'export STEP (erreurs {errors}, avertissements {warnings}).")
                .into(),
        ));
    }
    Ok(())
}

fn active_drawing_sheet_info(document: &IDispatch) -> Result<(String, i32)> {
    let sheet = invoke(document, "GetCurrentSheet", DISPATCH_METHOD, &mut [], true)?;
    let sheet = dispatch_from_variant(&sheet)?;
    let mut width = 0.0;
    let mut height = 0.0;
    let mut arguments = [byref_f64(&mut height), byref_f64(&mut width)];
    let paper_size = invoke(&sheet, "GetSize", DISPATCH_METHOD, &mut arguments, true)?;
    let paper_size = i32_from_variant(&paper_size)?;
    let format = match paper_size {
        6 | 7 => "A4",
        8 => "A3",
        9 => "A2",
        10 => "A1",
        11 => "A0",
        _ => "Autre",
    };
    let orientation = if width > height { 2 } else { 1 };
    Ok((format.to_owned(), orientation))
}

pub(super) fn drawing_paper_size(sw_app: &IDispatch, file_path: &str) -> Result<String> {
    if document_type(file_path)? != 3 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "La lecture du format nécessite un fichier .slddrw".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, 3, 1)?;
    let sheet_info = active_drawing_sheet_info(&document);
    let close_result = if opened_by_this_call {
        close_document(sw_app, &document, file_path)
    } else {
        Ok(())
    };
    let (format, _) = sheet_info?;
    close_result?;
    Ok(format)
}

fn printer_paper_code(printer_name: &str, paper_format: &str) -> Result<i32> {
    let printer_wide: Vec<u16> = printer_name.encode_utf16().chain(Some(0)).collect();
    let printer = PCWSTR(printer_wide.as_ptr());
    let empty_port = [0_u16];
    let port = PCWSTR(empty_port.as_ptr());
    let paper_count = unsafe { DeviceCapabilitiesW(printer, port, DC_PAPERS, PWSTR::null(), None) };
    if paper_count <= 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Impossible de récupérer les formats papier du pilote '{printer_name}'.")
                .into(),
        ));
    }

    let mut paper_codes = vec![0_u16; paper_count as usize];
    let returned_codes = unsafe {
        DeviceCapabilitiesW(
            printer,
            port,
            DC_PAPERS,
            PWSTR(paper_codes.as_mut_ptr()),
            None,
        )
    };
    if returned_codes <= 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Impossible de lire les codes papier du pilote '{printer_name}'.").into(),
        ));
    }

    let name_count =
        unsafe { DeviceCapabilitiesW(printer, port, DC_PAPERNAMES, PWSTR::null(), None) };
    if name_count <= 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Le pilote '{printer_name}' ne fournit pas les noms de ses formats papier.")
                .into(),
        ));
    }

    const PAPER_NAME_LENGTH: usize = 64;
    let mut paper_names = vec![0_u16; name_count as usize * PAPER_NAME_LENGTH];
    let returned_names = unsafe {
        DeviceCapabilitiesW(
            printer,
            port,
            DC_PAPERNAMES,
            PWSTR(paper_names.as_mut_ptr()),
            None,
        )
    };
    if returned_names <= 0 {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Impossible de lire les noms papier du pilote '{printer_name}'.").into(),
        ));
    }

    let entry_count = (returned_codes as usize)
        .min(returned_names as usize)
        .min(paper_count as usize)
        .min(name_count as usize);
    let mut compatible_code = None;
    for index in 0..entry_count {
        let start = index * PAPER_NAME_LENGTH;
        let name_slice = &paper_names[start..start + PAPER_NAME_LENGTH];
        let name_length = name_slice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(PAPER_NAME_LENGTH);
        let name = String::from_utf16_lossy(&name_slice[..name_length]);
        if name.trim().eq_ignore_ascii_case(paper_format) {
            return Ok(paper_codes[index] as i32);
        }
        if compatible_code.is_none() && paper_name_matches_format(&name, paper_format) {
            compatible_code = Some(paper_codes[index] as i32);
        }
    }

    compatible_code.ok_or_else(|| {
        Error::new(
            HRESULT(0x80070057_u32 as i32),
            format!(
                "Le pilote de l'imprimante '{printer_name}' ne propose pas de format {paper_format}."
            )
            .into(),
        )
    })
}

fn paper_name_matches_format(paper_name: &str, paper_format: &str) -> bool {
    let paper_name = paper_name.trim();
    let paper_name = ["ISO ", "ISO-"]
        .iter()
        .find_map(|prefix| {
            paper_name
                .get(..prefix.len())
                .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
                .map(|_| &paper_name[prefix.len()..])
        })
        .unwrap_or(paper_name);
    if paper_name.eq_ignore_ascii_case(paper_format) {
        return true;
    }

    let Some(suffix) = paper_name
        .get(paper_format.len()..)
        .filter(|_| paper_name[..paper_format.len()].eq_ignore_ascii_case(paper_format))
    else {
        return false;
    };
    suffix
        .chars()
        .next()
        .is_some_and(|character| !character.is_ascii_alphanumeric())
        || suffix.eq_ignore_ascii_case("R")
        || suffix.eq_ignore_ascii_case("ROTATED")
}

fn set_i32_property(object: &IDispatch, name: &str, value: i32) -> Result<()> {
    let mut value = variant_i32(value);
    set_property(object, name, &mut value)
}

struct OriginalDrawingPrintSettings {
    printer_name: String,
    paper_size: i32,
    orientation: i32,
    page_setup_mode: i32,
}

fn restore_drawing_print_settings(
    document: &IDispatch,
    extension: &IDispatch,
    page_setup: &IDispatch,
    settings: &OriginalDrawingPrintSettings,
) -> Result<()> {
    let mut first_error = None;
    let mut printer = variant_bstr(&settings.printer_name);
    if let Err(error) = set_property(document, "Printer", &mut printer) {
        first_error = Some(error);
    }
    if let Err(error) = set_i32_property(page_setup, "PrinterPaperSize", settings.paper_size) {
        first_error.get_or_insert(error);
    }
    if let Err(error) = set_i32_property(page_setup, "Orientation", settings.orientation) {
        first_error.get_or_insert(error);
    }
    if let Err(error) = set_i32_property(extension, "UsePageSetup", settings.page_setup_mode) {
        first_error.get_or_insert(error);
    }
    first_error.map_or(Ok(()), Err)
}

fn combine_with_cleanup(primary: Result<()>, cleanup: Result<()>, context: &str) -> Result<()> {
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(Error::new(
            error.code(),
            format!("{error:?}; {context}: {cleanup_error:?}").into(),
        )),
    }
}

pub(super) fn print_drawing(
    sw_app: &IDispatch,
    file_path: &str,
    expected_format: &str,
    printer_name: &str,
) -> Result<()> {
    if document_type(file_path)? != 3 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "L'impression nécessite un fichier .slddrw".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, 3, 1)?;
    let print_result: Result<()> = (|| {
        let (paper_format, orientation) = active_drawing_sheet_info(&document)?;
        if !paper_format.eq_ignore_ascii_case(expected_format) {
            return Err(Error::new(
                HRESULT(0x80070057_u32 as i32),
                format!(
                    "Le format de '{file_path}' a changé ({paper_format}, attendu {expected_format}). Relancez la recherche du format."
                )
                .into(),
            ));
        }
        if paper_format == "Autre" {
            return Err(Error::new(
                HRESULT(0x80070057_u32 as i32),
                format!("Le format papier de '{file_path}' n'est pas pris en charge.").into(),
            ));
        }
        let paper_code = printer_paper_code(printer_name, &paper_format)?;
        let extension = invoke(&document, "Extension", DISPATCH_PROPERTYGET, &mut [], true)?;
        let extension = dispatch_from_variant(&extension)?;
        let page_setup = invoke(&document, "PageSetup", DISPATCH_PROPERTYGET, &mut [], true)?;
        let page_setup = dispatch_from_variant(&page_setup)?;
        let page_setup_mode = i32_from_variant(&invoke(
            &extension,
            "UsePageSetup",
            DISPATCH_PROPERTYGET,
            &mut [],
            true,
        )?)?;
        let printer_name_before =
            invoke_string(&document, "Printer", DISPATCH_PROPERTYGET, &mut [])?;

        if let Err(error) = set_i32_property(&extension, "UsePageSetup", 1) {
            let restore_result = set_i32_property(&extension, "UsePageSetup", page_setup_mode);
            return combine_with_cleanup(
                Err(error),
                restore_result,
                "restauration du mode de page",
            );
        }
        let previous_page_settings = (|| {
            let paper_size = i32_from_variant(&invoke(
                &page_setup,
                "PrinterPaperSize",
                DISPATCH_PROPERTYGET,
                &mut [],
                true,
            )?)?;
            let previous_orientation = i32_from_variant(&invoke(
                &page_setup,
                "Orientation",
                DISPATCH_PROPERTYGET,
                &mut [],
                true,
            )?)?;
            Ok((paper_size, previous_orientation))
        })();
        let (previous_paper_size, previous_orientation) = match previous_page_settings {
            Ok(settings) => settings,
            Err(error) => {
                let restore_result = set_i32_property(&extension, "UsePageSetup", page_setup_mode);
                return combine_with_cleanup(
                    Err(error),
                    restore_result,
                    "restauration du mode de page",
                );
            }
        };
        let original_settings = OriginalDrawingPrintSettings {
            printer_name: printer_name_before,
            paper_size: previous_paper_size,
            orientation: previous_orientation,
            page_setup_mode,
        };

        let print_operation = (|| {
            let mut printer = variant_bstr(printer_name);
            set_property(&document, "Printer", &mut printer)?;
            set_i32_property(&page_setup, "PrinterPaperSize", paper_code)?;
            set_i32_property(&page_setup, "Orientation", orientation)?;

            let print_specification = invoke(
                &extension,
                "GetPrintSpecification",
                DISPATCH_METHOD,
                &mut [],
                true,
            )?;
            let print_specification = dispatch_from_variant(&print_specification)?;
            let mut queue = variant_bstr(printer_name);
            set_property(&print_specification, "PrinterQueue", &mut queue)?;
            let mut arguments = [
                variant_dispatch(&print_specification),
                variant_bstr(""),
                variant_bstr(printer_name),
            ];
            invoke(
                &extension,
                "PrintOut4",
                DISPATCH_METHOD,
                &mut arguments,
                false,
            )?;
            Ok(())
        })();
        let restore_result =
            restore_drawing_print_settings(&document, &extension, &page_setup, &original_settings);
        combine_with_cleanup(
            print_operation,
            restore_result,
            "restauration des réglages d'impression",
        )
    })();
    let close_result = if opened_by_this_call {
        close_document(sw_app, &document, file_path)
    } else {
        Ok(())
    };
    combine_with_cleanup(print_result, close_result, "fermeture du document")
}

pub(super) fn open_file_and_activate(sw_app: &IDispatch, file_path: &str) -> Result<()> {
    let document_type = document_type(file_path)?;
    let (document, _) = open_or_reuse_document(sw_app, file_path, document_type, 1)?;
    let title = invoke_string(&document, "GetTitle", DISPATCH_METHOD, &mut [])?;
    let mut errors = 0_i32;
    let mut activate_arguments = [
        byref_i32(&mut errors),
        variant_i32(0),
        variant_bool(false),
        variant_bstr(&title),
    ];
    invoke(
        sw_app,
        "ActivateDoc3",
        DISPATCH_METHOD,
        &mut activate_arguments,
        true,
    )?;

    let frame_variant = invoke(sw_app, "Frame", DISPATCH_PROPERTYGET, &mut [], true)?;
    let frame = dispatch_from_variant(&frame_variant)?;
    let window_handle = invoke(&frame, "GetHWndx64", DISPATCH_METHOD, &mut [], true)
        .and_then(|value| i64_from_variant(&value).map(|handle| handle as isize))
        .or_else(|_| {
            invoke(&frame, "GetHWnd", DISPATCH_METHOD, &mut [], true)
                .and_then(|value| i32_from_variant(&value).map(|handle| handle as isize))
        })?;
    let window = HWND(window_handle);
    unsafe {
        ShowWindow(window, SW_RESTORE);
        SetForegroundWindow(window);
    }
    Ok(())
}

/// Met à jour les propriétés indiquées puis sauvegarde le document.
///
/// Le document est ouvert en lecture-écriture si nécessaire. Un document qui était déjà ouvert
/// reste ouvert après l'opération.
///
/// # Errors
/// Renvoie une erreur si une propriété n'existe pas, ne peut pas être modifiée, ou si la
/// sauvegarde échoue.
pub(super) fn update_file_properties(
    sw_app: &IDispatch,
    file_path: &str,
    properties: &[CustomProperty],
) -> Result<()> {
    if properties.is_empty() {
        return Ok(());
    }

    let document_type = document_type(file_path)?;
    let (document, opened_by_this_call) =
        open_or_reuse_document(sw_app, file_path, document_type, 1)?;

    let update_result = write_document_and_configuration_properties(&document, properties)
        .and_then(|()| save_document(&document));
    // let update_result =
    // write_custom_properties(&document, properties).and_then(|()| save_document(&document));

    if !opened_by_this_call {
        return update_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    update_result.and_then(|()| close_result)
}

/// Détermine le type SolidWorks à partir de l'extension du fichier.
///
/// # Errors
/// Renvoie `E_INVALIDARG` si le fichier n'est pas une pièce, un assemblage ou un dessin.
fn document_type(file_path: &str) -> Result<i32> {
    let extension = Path::new(file_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    if extension.eq_ignore_ascii_case("sldprt") {
        Ok(1) // swDocPART
    } else if extension.eq_ignore_ascii_case("sldasm") {
        Ok(2) // swDocASSEMBLY
    } else if extension.eq_ignore_ascii_case("slddrw") {
        Ok(3) // swDocDRAWING
    } else {
        Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "Le fichier doit être un .sldprt, un .sldasm ou un .slddrw".into(),
        ))
    }
}

/// Retourne un document déjà ouvert ou l'ouvre avec les options demandées.
///
/// `open_options` correspond aux options SolidWorks pour `OpenDoc6`.
fn open_or_reuse_document(
    sw_app: &IDispatch,
    file_path: &str,
    document_type: i32,
    open_options: i32,
) -> Result<(IDispatch, bool)> {
    let mut lookup_arguments = [variant_bstr(file_path)];
    let existing_document = invoke(
        sw_app,
        "GetOpenDocumentByName",
        DISPATCH_METHOD,
        &mut lookup_arguments,
        true,
    )?;

    if let Some(document) = optional_dispatch_from_variant(&existing_document) {
        return Ok((document, false));
    }

    let mut errors = 0_i32;
    let mut warnings = 0_i32;
    let mut arguments = vec![
        byref_i32(&mut warnings),
        byref_i32(&mut errors),
        variant_bstr(""),
        variant_i32(open_options),
        variant_i32(document_type),
        variant_bstr(file_path),
    ];
    let opened_document = invoke(sw_app, "OpenDoc6", DISPATCH_METHOD, &mut arguments, true)?;
    Ok((dispatch_from_variant(&opened_document)?, true))
}

/// Ferme un document en utilisant son titre, ou son nom de fichier en repli.
fn close_document(sw_app: &IDispatch, document: &IDispatch, file_path: &str) -> Result<()> {
    let title =
        invoke_string(document, "GetTitle", DISPATCH_METHOD, &mut []).unwrap_or_else(|_| {
            Path::new(file_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(file_path)
                .to_owned()
        });
    let mut arguments = [variant_bstr(&title)];
    invoke(sw_app, "CloseDoc", DISPATCH_METHOD, &mut arguments, false)?;
    Ok(())
}

/// Construit l'arbre des composants directs puis calcule en parallèle leurs quantités aplaties.
fn read_assembly_tree(
    sw_app: &IDispatch,
    document: &IDispatch,
    master_path: &str,
) -> Result<(AssemblyTreeNode, Vec<AssemblyItem>)> {
    let mut arguments = [variant_bool(true)];
    let mut components_variant = invoke(
        document,
        "GetComponents",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )?;
    let components = dispatch_array_from_variant(&mut components_variant)?;

    let mut ancestors = HashSet::new();
    ancestors.insert(master_path.to_lowercase());
    let children = read_tree_children(sw_app, components, &mut ancestors)?;
    let root = AssemblyTreeNode {
        kind: AssemblyItemKind::Assembly,
        name: Path::new(master_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(master_path)
            .to_owned(),
        path: master_path.to_owned(),
        thickness: None,
        quantity: 1,
        children,
    };

    let mut totals = HashMap::new();
    accumulate_tree_quantities(&root, 1, &mut totals);
    let mut items: Vec<AssemblyItem> = totals.into_values().collect();
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });

    Ok((root, items))
}

/// Regroupe les composants directs d'un même assemblage et descend récursivement dans ses enfants.
fn read_tree_children(
    sw_app: &IDispatch,
    components: Vec<IDispatch>,
    ancestors: &mut HashSet<String>,
) -> Result<Vec<AssemblyTreeNode>> {
    let mut grouped: HashMap<String, (IDispatch, String, AssemblyItemKind, Option<u8>, usize)> =
        HashMap::new();
    let mut sheet_metal_by_path = HashMap::new();

    for component in components {
        let suppression = invoke(&component, "GetSuppression", DISPATCH_METHOD, &mut [], true)?;
        if i32_from_variant(&suppression)? == 0 {
            continue;
        }

        let Ok(path) = invoke_string(&component, "GetPathName", DISPATCH_METHOD, &mut []) else {
            continue;
        };
        let key = path.to_lowercase();
        let kind = match Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some(extension) if extension.eq_ignore_ascii_case("sldprt") => {
                let (is_sheet_metal, _) = *sheet_metal_by_path
                    .entry(key.clone())
                    .or_insert_with(|| component_sheet_metal_info(&component));
                if is_sheet_metal {
                    AssemblyItemKind::SheetMetal
                } else {
                    AssemblyItemKind::Part
                }
            }
            Some(extension) if extension.eq_ignore_ascii_case("sldasm") => {
                AssemblyItemKind::Assembly
            }
            _ => continue,
        };

        let thickness = if kind == AssemblyItemKind::SheetMetal {
            sheet_metal_by_path
                .get(&key)
                .and_then(|(_, thickness)| *thickness)
        } else {
            None
        };

        if let Some((_, _, _, _, quantity)) = grouped.get_mut(&key) {
            *quantity += 1;
        } else {
            grouped.insert(key, (component, path, kind, thickness, 1));
        }
    }

    let mut grouped: Vec<_> = grouped.into_values().collect();
    grouped.sort_by(|left, right| {
        left.1
            .to_lowercase()
            .cmp(&right.1.to_lowercase())
            .then_with(|| left.2.cmp(&right.2))
    });

    grouped
        .into_iter()
        .map(|(component, path, kind, thickness, quantity)| {
            let key = path.to_lowercase();
            let children = if kind == AssemblyItemKind::Assembly && ancestors.insert(key.clone()) {
                let children_result = (|| {
                    let mut children_variant =
                        invoke(&component, "GetChildren", DISPATCH_METHOD, &mut [], true)?;
                    let children = dispatch_array_from_variant(&mut children_variant)?;
                    read_tree_children(sw_app, children, ancestors)
                })();
                ancestors.remove(&key);
                children_result?
            } else {
                Vec::new()
            };
            let name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&path)
                .to_owned();

            Ok(AssemblyTreeNode {
                kind,
                name,
                path,
                thickness,
                quantity,
                children,
            })
        })
        .collect()
}

/// Ajoute les occurrences de chaque nœud à la liste aplatie, en tenant compte des quantités parentes.
fn accumulate_tree_quantities(
    node: &AssemblyTreeNode,
    parent_quantity: usize,
    totals: &mut HashMap<String, AssemblyItem>,
) {
    let total_quantity = parent_quantity.saturating_mul(node.quantity);
    let key = node.path.to_lowercase();
    let item = totals.entry(key).or_insert_with(|| AssemblyItem {
        kind: node.kind,
        name: node.name.clone(),
        path: node.path.clone(),
        thickness: node.thickness,
        quantity: 0,
    });
    item.quantity = item.quantity.saturating_add(total_quantity);

    for child in &node.children {
        accumulate_tree_quantities(child, total_quantity, totals);
    }
}

/// Détermine si un composant contient de la tôlerie et lit son épaisseur nominale.
///
/// Une erreur d'accès aux corps est traitée comme une absence de tôlerie afin de ne pas
/// interrompre l'extraction de la liste des composants.
fn component_sheet_metal_info(component: &IDispatch) -> (bool, Option<u8>) {
    let mut arguments = [variant_i32(0)]; // swSolidBody
    let Ok(mut bodies_variant) = invoke(
        component,
        "GetBodies2",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    ) else {
        return (false, None);
    };
    let Ok(bodies) = dispatch_array_from_variant(&mut bodies_variant) else {
        return (false, None);
    };

    let is_sheet_metal = bodies.into_iter().any(|body| {
        invoke(&body, "IsSheetMetal", DISPATCH_METHOD, &mut [], true)
            .and_then(|result| bool_from_variant(&result))
            .unwrap_or(false)
    });

    if is_sheet_metal {
        (true, component_sheet_metal_thickness(component))
    } else {
        (false, None)
    }
}

/// Lit l'épaisseur en parcourant les définitions des fonctions de tôlerie du document.
///
/// SolidWorks fournit cette valeur en mètres. Elle est convertie en millimètres et arrondie à
/// l'entier le plus proche pour correspondre au type `Option<u8>` de la nomenclature.
fn component_sheet_metal_thickness(component: &IDispatch) -> Option<u8> {
    let document_variant =
        invoke(component, "GetModelDoc2", DISPATCH_METHOD, &mut [], true).ok()?;
    let document = optional_dispatch_from_variant(&document_variant)?;
    let mut feature_variant =
        invoke(&document, "FirstFeature", DISPATCH_METHOD, &mut [], true).ok()?;
    let mut feature = optional_dispatch_from_variant(&feature_variant)?;

    loop {
        let feature_type = invoke_string(&feature, "GetTypeName2", DISPATCH_METHOD, &mut []).ok();
        if feature_type.as_deref().is_some_and(|feature_type| {
            matches!(feature_type, "SheetMetal" | "SMBaseFlange" | "BaseFlange")
        }) {
            if let Ok(definition_variant) =
                invoke(&feature, "GetDefinition", DISPATCH_METHOD, &mut [], true)
            {
                if let Some(definition) = optional_dispatch_from_variant(&definition_variant) {
                    if let Ok(thickness_variant) = invoke(
                        &definition,
                        "Thickness",
                        DISPATCH_PROPERTYGET,
                        &mut [],
                        true,
                    ) {
                        if let Ok(thickness_meters) = f64_from_variant(&thickness_variant) {
                            let thickness_mm = (thickness_meters * 1000.0).round();
                            if thickness_mm.is_finite()
                                && (1.0..=u8::MAX as f64).contains(&thickness_mm)
                            {
                                return Some(thickness_mm as u8);
                            }
                        }
                    }
                }
            }
        }

        feature_variant =
            invoke(&feature, "GetNextFeature", DISPATCH_METHOD, &mut [], true).ok()?;
        feature = optional_dispatch_from_variant(&feature_variant)?;
    }
}

/// Lit les propriétés personnalisées générales d'un document `ModelDoc2`.
///
/// Pour chaque propriété, conserve sa valeur résolue lorsqu'elle est disponible, sinon sa
/// valeur non résolue.
///
/// # Errors
/// Renvoie une erreur si SolidWorks ne fournit pas l'extension, le gestionnaire de propriétés
/// ou si la lecture d'une propriété échoue.
fn read_document_and_configuration_properties(document: &IDispatch) -> Result<Vec<CustomProperty>> {
    let mut properties = read_custom_properties(&custom_property_manager(document)?, false)?;
    let configuration_manager = active_configuration_property_manager(document)?;
    properties.extend(read_custom_properties(&configuration_manager, true)?);
    Ok(properties)
}

fn read_custom_properties(manager: &IDispatch, configuration: bool) -> Result<Vec<CustomProperty>> {
    let mut no_arguments = [];
    let mut names_variant = invoke(
        &manager,
        "GetNames",
        DISPATCH_METHOD,
        &mut no_arguments,
        true,
    )?;
    let names = string_array_from_variant(&mut names_variant)?;

    names
        .into_iter()
        .map(|name| {
            let mut value = BSTR::new();
            let mut resolved_value = BSTR::new();
            let mut was_resolved = VARIANT_BOOL(0);
            let mut linked_to_property = VARIANT_BOOL(0);
            let mut arguments = vec![
                byref_bool(&mut linked_to_property),
                byref_bool(&mut was_resolved),
                byref_bstr(&mut resolved_value),
                byref_bstr(&mut value),
                variant_bool(false),
                variant_bstr(&name),
            ];

            invoke(&manager, "Get6", DISPATCH_METHOD, &mut arguments, false)?;
            let resolved = resolved_value.to_string();
            let value = if resolved.is_empty() {
                value.to_string()
            } else {
                resolved
            };
            Ok(CustomProperty {
                name,
                value,
                configuration,
            })
        })
        .collect()
}

/// Écrit les propriétés au bon niveau : document ou configuration active.
fn write_document_and_configuration_properties(
    document: &IDispatch,
    properties: &[CustomProperty],
) -> Result<()> {
    let document_properties: Vec<_> = properties
        .iter()
        .filter(|property| !property.configuration)
        .cloned()
        .collect();
    let configuration_properties: Vec<_> = properties
        .iter()
        .filter(|property| property.configuration)
        .cloned()
        .collect();

    if !document_properties.is_empty() {
        let manager = custom_property_manager(document)?;
        write_custom_properties(&manager, &document_properties)?;
    }
    if !configuration_properties.is_empty() {
        let manager = active_configuration_property_manager(document)?;
        write_custom_properties(&manager, &configuration_properties)?;
    }
    Ok(())
}

/// Remplace les valeurs des propriétés et vérifie le code retour de `Set2`.
fn write_custom_properties(manager: &IDispatch, properties: &[CustomProperty]) -> Result<()> {
    for property in properties {
        let mut arguments = [variant_bstr(&property.value), variant_bstr(&property.name)];
        let result = invoke(&manager, "Set2", DISPATCH_METHOD, &mut arguments, true)?;
        let result_code = i32_from_variant(&result)?;
        if result_code != 0 {
            return Err(Error::new(
                HRESULT(0x80004005_u32 as i32),
                format!(
                    "SolidWorks n'a pas pu modifier la propriété '{}' (code {result_code}).",
                    property.name
                )
                .into(),
            ));
        }
    }

    Ok(())
}

/// Sauvegarde un document SolidWorks en mode silencieux.
fn save_document(document: &IDispatch) -> Result<()> {
    let mut errors = 0_i32;
    let mut warnings = 0_i32;
    let mut arguments = [
        byref_i32(&mut warnings),
        byref_i32(&mut errors),
        variant_i32(1), // swSaveAsOptions_Silent
    ];
    let result = invoke(document, "Save3", DISPATCH_METHOD, &mut arguments, true)?;

    if !bool_from_variant(&result)? {
        return Err(Error::new(
            HRESULT(0x80004005_u32 as i32),
            format!("Échec de sauvegarde (erreurs {errors}, avertissements {warnings}).").into(),
        ));
    }
    Ok(())
}

/// Récupère le gestionnaire des propriétés générales du document.
fn custom_property_manager(document: &IDispatch) -> Result<IDispatch> {
    let mut no_arguments = [];
    let extension_variant = invoke(
        document,
        "Extension",
        DISPATCH_PROPERTYGET,
        &mut no_arguments,
        true,
    )?;
    let extension = dispatch_from_variant(&extension_variant)?;

    let mut arguments = [variant_bstr("")];
    let manager_variant = invoke(
        &extension,
        "CustomPropertyManager",
        DISPATCH_PROPERTYGET,
        &mut arguments,
        true,
    )?;
    dispatch_from_variant(&manager_variant)
}

/// Récupère le gestionnaire des propriétés de la configuration active.
fn active_configuration_property_manager(document: &IDispatch) -> Result<IDispatch> {
    let mut no_arguments = [];
    let configuration_variant = invoke(
        document,
        "ConfigurationManager",
        DISPATCH_PROPERTYGET,
        &mut no_arguments,
        true,
    )?;
    let configuration_manager = dispatch_from_variant(&configuration_variant)?;
    let mut no_arguments = [];
    let active_configuration_variant = invoke(
        &configuration_manager,
        "ActiveConfiguration",
        DISPATCH_PROPERTYGET,
        &mut no_arguments,
        true,
    )?;
    let active_configuration = dispatch_from_variant(&active_configuration_variant)?;
    let mut no_arguments = [];
    let property_manager_variant = invoke(
        &active_configuration,
        "CustomPropertyManager",
        DISPATCH_PROPERTYGET,
        &mut no_arguments,
        true,
    )?;
    dispatch_from_variant(&property_manager_variant)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        accumulate_tree_quantities, paper_name_matches_format, AssemblyItemKind, AssemblyTreeNode,
    };

    #[test]
    fn matches_printer_paper_names_to_a_format() {
        assert!(paper_name_matches_format("A4", "A4"));
        assert!(paper_name_matches_format("A4 (210 x 297 mm)", "A4"));
        assert!(paper_name_matches_format("iso a4", "A4"));
        assert!(!paper_name_matches_format("A40", "A4"));
    }

    #[test]
    fn flattens_tree_quantities_through_repeated_subassemblies() {
        let tree = AssemblyTreeNode {
            kind: AssemblyItemKind::Assembly,
            name: "master.sldasm".to_owned(),
            path: "C:/master.sldasm".to_owned(),
            thickness: None,
            quantity: 1,
            children: vec![AssemblyTreeNode {
                kind: AssemblyItemKind::Assembly,
                name: "sub.sldasm".to_owned(),
                path: "C:/sub.sldasm".to_owned(),
                thickness: None,
                quantity: 2,
                children: vec![AssemblyTreeNode {
                    kind: AssemblyItemKind::Part,
                    name: "part.sldprt".to_owned(),
                    path: "C:/part.sldprt".to_owned(),
                    thickness: Some(2),
                    quantity: 3,
                    children: Vec::new(),
                }],
            }],
        };
        let mut totals = HashMap::new();

        accumulate_tree_quantities(&tree, 1, &mut totals);

        assert_eq!(totals["c:/master.sldasm"].quantity, 1);
        assert_eq!(totals["c:/sub.sldasm"].quantity, 2);
        assert_eq!(totals["c:/part.sldprt"].quantity, 6);
        assert_eq!(totals["c:/part.sldprt"].thickness, Some(2));
    }
}
