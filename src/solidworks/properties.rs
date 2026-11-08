use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use windows::core::{Error, Result, BSTR, HRESULT};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{IDispatch, DISPATCH_METHOD, DISPATCH_PROPERTYGET};
use windows::Win32::System::Variant::VARIANT;

use super::dispatch::{
    bool_from_variant, byref_bool, byref_bstr, byref_i32, dispatch_array_from_variant,
    dispatch_from_variant, i32_from_variant, invoke, invoke_string, optional_dispatch_from_variant,
    string_array_from_variant, variant_bool, variant_bstr, variant_i32,
};

static PREVIEW_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Vignette d'aperçu d'un document SolidWorks, au format RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssemblyThumbnail {
    /// Largeur de l'image en pixels.
    pub(crate) width: usize,
    /// Hauteur de l'image en pixels.
    pub(crate) height: usize,
    /// Pixels RGBA, ligne par ligne de haut en bas.
    pub(crate) rgba: Vec<u8>,
}

/// Un composant référencé par un assemblage et son nombre total d'occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyItem {
    /// Catégorie du composant : pièce ou assemblage.
    pub kind: AssemblyItemKind,
    /// Nom du fichier du composant.
    pub name: String,
    /// Chemin du fichier référencé par l'assemblage.
    pub path: String,
    /// Nombre total d'occurrences dans l'assemblage.
    pub quantity: usize,
    /// Image miniature du fichier, si SolidWorks fournit un aperçu.
    pub(crate) thumbnail: Option<AssemblyThumbnail>,
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

/// Une propriété personnalisée lue dans un document SolidWorks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomProperty {
    /// Nom de la propriété dans le document.
    pub name: String,
    /// Valeur résolue de la propriété, ou sa valeur brute si aucune résolution n'est disponible.
    pub value: String,
}

/// Ouvre ou réutilise un assemblage et retourne ses composants avec leurs quantités cumulées.
///
/// La recherche inclut les pièces et les sous-assemblages imbriqués. L'assemblage maître est
/// également inclus une fois dans le résultat. Les composants supprimés sont ignorés.
///
/// # Arguments
/// * `sw_app` — interface de l'application SolidWorks.
/// * `file_path` — chemin d'un fichier `.sldasm`.
///
/// # Errors
/// Renvoie une erreur si le chemin ne désigne pas un assemblage ou si un appel COM échoue.
pub(super) fn get_assembly_components(
    sw_app: &IDispatch,
    file_path: &str,
) -> Result<Vec<AssemblyItem>> {
    if document_type(file_path)? != 2 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "La liste des composants nécessite un fichier .sldasm".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, 2, 3)?;
    let parts_result = read_assembly_components(sw_app, &document, file_path);
    if !opened_by_this_call {
        return parts_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    parts_result.and_then(|parts| {
        close_result?;
        Ok(parts)
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

    let properties_result = read_custom_properties(&document);
    if !opened_by_this_call {
        return properties_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    properties_result.and_then(|properties| {
        close_result?;
        Ok(properties)
    })
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

    let update_result =
        write_custom_properties(&document, properties).and_then(|()| save_document(&document));

    if !opened_by_this_call {
        return update_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    update_result.and_then(|()| close_result)
}

/// Détermine le type SolidWorks à partir de l'extension du fichier.
///
/// # Errors
/// Renvoie `E_INVALIDARG` si le fichier n'est ni une pièce ni un assemblage.
fn document_type(file_path: &str) -> Result<i32> {
    let extension = Path::new(file_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    if extension.eq_ignore_ascii_case("sldprt") {
        Ok(1) // swDocPART
    } else if extension.eq_ignore_ascii_case("sldasm") {
        Ok(2) // swDocASSEMBLY
    } else {
        Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "Le fichier doit être un .sldprt ou un .sldasm".into(),
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

/// Parcourt tous les composants et regroupe les occurrences de pièces et d'assemblages.
///
/// `GetComponents(false)` inclut les composants des sous-assemblages. L'assemblage maître est
/// ajouté avec une quantité de un ; les composants supprimés ou sans chemin de fichier sont
/// ignorés.
///
/// # Errors
/// Renvoie une erreur si SolidWorks ne peut pas fournir la liste ou les informations des composants.
fn read_assembly_components(
    sw_app: &IDispatch,
    document: &IDispatch,
    master_path: &str,
) -> Result<Vec<AssemblyItem>> {
    let mut arguments = [variant_bool(false)];
    let mut components_variant = invoke(
        document,
        "GetComponents",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )?;
    let components = dispatch_array_from_variant(&mut components_variant)?;
    let mut totals: HashMap<String, AssemblyItem> = HashMap::new();
    let mut sheet_metal_by_path: HashMap<String, bool> = HashMap::new();
    let mut thumbnails_by_path: HashMap<String, Option<AssemblyThumbnail>> = HashMap::new();

    let master_name = Path::new(master_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(master_path)
        .to_owned();
    let master_configuration = invoke(
        document,
        "GetActiveConfiguration",
        DISPATCH_METHOD,
        &mut [],
        true,
    )
    .and_then(|configuration| dispatch_from_variant(&configuration))
    .and_then(|configuration| invoke_string(&configuration, "Name", DISPATCH_PROPERTYGET, &mut []))
    .unwrap_or_else(|_| "Default".to_owned());
    let master_thumbnail = get_preview_thumbnail(sw_app, master_path, &master_configuration);
    thumbnails_by_path.insert(master_path.to_lowercase(), master_thumbnail.clone());
    totals.insert(
        master_path.to_lowercase(),
        AssemblyItem {
            kind: AssemblyItemKind::Assembly,
            name: master_name,
            path: master_path.to_owned(),
            quantity: 1,
            thumbnail: master_thumbnail,
        },
    );

    for component in components {
        let suppression = invoke(&component, "GetSuppression", DISPATCH_METHOD, &mut [], true)?;
        if i32_from_variant(&suppression)? == 0 {
            continue;
        }

        let path = match invoke_string(&component, "GetPathName", DISPATCH_METHOD, &mut []) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let key = path.to_lowercase();
        let kind = match Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some(extension) if extension.eq_ignore_ascii_case("sldprt") => {
                let is_sheet_metal = *sheet_metal_by_path
                    .entry(key.clone())
                    .or_insert_with(|| component_is_sheet_metal(&component));
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
        let name = Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path)
            .to_owned();
        let thumbnail = thumbnails_by_path.get(&key).cloned().unwrap_or_else(|| {
            let configuration = invoke_string(
                &component,
                "ReferencedConfiguration",
                DISPATCH_PROPERTYGET,
                &mut [],
            )
            .unwrap_or_else(|_| "Default".to_owned());
            let thumbnail = get_preview_thumbnail(sw_app, &path, &configuration);
            thumbnails_by_path.insert(key.clone(), thumbnail.clone());
            thumbnail
        });
        let entry = totals.entry(key).or_insert(AssemblyItem {
            kind,
            name,
            path,
            quantity: 0,
            thumbnail,
        });
        entry.quantity += 1;
    }

    let mut items: Vec<AssemblyItem> = totals.into_values().collect();
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });
    Ok(items)
}

/// Récupère l'aperçu enregistré du document SolidWorks dans un BMP temporaire.
///
/// L'absence d'aperçu n'interrompt pas la lecture de la liste d'assemblage.
fn get_preview_thumbnail(
    sw_app: &IDispatch,
    file_path: &str,
    configuration: &str,
) -> Option<AssemblyThumbnail> {
    let counter = PREVIEW_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let preview_path = std::env::temp_dir().join(format!(
        "solidworks-preview-{}-{counter}.bmp",
        std::process::id()
    ));
    let _ = fs::remove_file(&preview_path);
    let preview_path_string = preview_path.to_string_lossy().into_owned();
    let mut arguments = [
        variant_bstr(&preview_path_string),
        variant_bstr(configuration),
        variant_bstr(file_path),
    ];

    let thumbnail = invoke(
        sw_app,
        "GetPreviewBitmapFile",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )
    .and_then(|result| bool_from_variant(&result))
    .ok()
    .filter(|success| *success)
    .and_then(|_| fs::read(&preview_path).ok())
    .and_then(|bytes| parse_bmp_thumbnail(&bytes));

    let _ = fs::remove_file(preview_path);
    thumbnail
}

/// Décode un BMP non compressé 24 ou 32 bits en pixels RGBA.
fn parse_bmp_thumbnail(bytes: &[u8]) -> Option<AssemblyThumbnail> {
    let read_u16 = |offset: usize| {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    let read_u32 = |offset: usize| {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
        ))
    };

    if bytes.get(0..2)? != b"BM" {
        return None;
    }
    let pixel_offset = usize::try_from(read_u32(10)?).ok()?;
    let header_size = usize::try_from(read_u32(14)?).ok()?;
    if header_size < 40
        || 14_usize.checked_add(header_size)? > bytes.len()
        || pixel_offset < 14_usize.checked_add(header_size)?
    {
        return None;
    }

    let width_signed = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?);
    let height_signed = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?);
    let width = usize::try_from(width_signed).ok()?;
    let height = usize::try_from(height_signed.unsigned_abs()).ok()?;
    let bits_per_pixel = read_u16(28)?;
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width.checked_mul(height)? > 16_000_000
        || !matches!(bits_per_pixel, 24 | 32)
        || read_u32(30)? != 0
    {
        return None;
    }

    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    let row_stride = width.checked_mul(bytes_per_pixel)?.checked_add(3)? & !3;
    let pixel_data_size = row_stride.checked_mul(height)?;
    if pixel_offset.checked_add(pixel_data_size)? > bytes.len() {
        return None;
    }

    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for y in 0..height {
        let source_y = if height_signed > 0 { height - 1 - y } else { y };
        for x in 0..width {
            let source = pixel_offset + source_y * row_stride + x * bytes_per_pixel;
            let alpha = if bytes_per_pixel == 4 && bytes[source + 3] != 0 {
                bytes[source + 3]
            } else {
                255
            };
            rgba.extend_from_slice(&[bytes[source + 2], bytes[source + 1], bytes[source], alpha]);
        }
    }

    Some(AssemblyThumbnail {
        width,
        height,
        rgba,
    })
}

/// Indique si un composant contient au moins un corps solide de tôlerie.
///
/// Une erreur d'accès aux corps est traitée comme une absence de tôlerie afin de ne pas
/// interrompre l'extraction de la liste des composants.
fn component_is_sheet_metal(component: &IDispatch) -> bool {
    let mut arguments = [variant_i32(0)]; // swSolidBody
    let Ok(mut bodies_variant) = invoke(
        component,
        "GetBodies2",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    ) else {
        return false;
    };
    let Ok(bodies) = dispatch_array_from_variant(&mut bodies_variant) else {
        return false;
    };

    bodies.into_iter().any(|body| {
        invoke(&body, "IsSheetMetal", DISPATCH_METHOD, &mut [], true)
            .and_then(|result| bool_from_variant(&result))
            .unwrap_or(false)
    })
}

/// Lit les propriétés personnalisées générales d'un document `ModelDoc2`.
///
/// Pour chaque propriété, conserve sa valeur résolue lorsqu'elle est disponible, sinon sa
/// valeur non résolue.
///
/// # Errors
/// Renvoie une erreur si SolidWorks ne fournit pas l'extension, le gestionnaire de propriétés
/// ou si la lecture d'une propriété échoue.
fn read_custom_properties(document: &IDispatch) -> Result<Vec<CustomProperty>> {
    let manager = custom_property_manager(document)?;
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
            Ok(CustomProperty { name, value })
        })
        .collect()
}

/// Remplace les valeurs des propriétés et vérifie le code retour de `Set2`.
fn write_custom_properties(document: &IDispatch, properties: &[CustomProperty]) -> Result<()> {
    let manager = custom_property_manager(document)?;

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

#[cfg(test)]
mod tests {
    use super::parse_bmp_thumbnail;

    #[test]
    fn decodes_bottom_up_24_bit_bmp_to_top_down_rgba() {
        let mut bmp = vec![0_u8; 54];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&62_u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54_u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40_u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&1_i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2_i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24_u16.to_le_bytes());
        bmp[30..34].copy_from_slice(&0_u32.to_le_bytes());
        bmp[34..38].copy_from_slice(&8_u32.to_le_bytes());
        bmp.extend_from_slice(&[0, 255, 0, 0, 0, 0, 255, 0]);

        let thumbnail = parse_bmp_thumbnail(&bmp).expect("valid BMP should decode");

        assert_eq!((thumbnail.width, thumbnail.height), (1, 2));
        assert_eq!(thumbnail.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    }
}
