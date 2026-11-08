use std::{collections::HashMap, path::Path};
use windows::core::{Error, Result, BSTR, HRESULT};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{IDispatch, DISPATCH_METHOD, DISPATCH_PROPERTYGET};

use super::dispatch::{
    bool_from_variant, byref_bool, byref_bstr, byref_i32, dispatch_array_from_variant,
    dispatch_from_variant, i32_from_variant, invoke, invoke_string, optional_dispatch_from_variant,
    string_array_from_variant, variant_bool, variant_bstr, variant_i32,
};

/// Une pièce référencée par un assemblage et son nombre total d'occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyPart {
    /// Nom du fichier de pièce.
    pub name: String,
    /// Chemin du fichier de pièce référencé par l'assemblage.
    pub path: String,
    /// Nombre total d'occurrences dans l'assemblage, sous-assemblages inclus.
    pub quantity: usize,
}

/// Une propriété personnalisée lue dans un document SolidWorks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomProperty {
    /// Nom de la propriété dans le document.
    pub name: String,
    /// Valeur résolue de la propriété, ou sa valeur brute si aucune résolution n'est disponible.
    pub value: String,
}

/// Ouvre ou réutilise un assemblage et retourne les pièces avec leurs quantités cumulées.
///
/// La recherche inclut les composants imbriqués dans les sous-assemblages et ignore les
/// composants supprimés ainsi que les sous-assemblages eux-mêmes.
///
/// # Arguments
/// * `sw_app` — interface de l'application SolidWorks.
/// * `file_path` — chemin d'un fichier `.sldasm`.
///
/// # Errors
/// Renvoie une erreur si le chemin ne désigne pas un assemblage ou si un appel COM échoue.
pub(super) fn get_assembly_parts(sw_app: &IDispatch, file_path: &str) -> Result<Vec<AssemblyPart>> {
    if document_type(file_path)? != 2 {
        return Err(Error::new(
            HRESULT(0x80070057_u32 as i32),
            "La liste des pièces nécessite un fichier .sldasm".into(),
        ));
    }

    let (document, opened_by_this_call) = open_or_reuse_document(sw_app, file_path, 2, 3)?;
    let parts_result = read_assembly_parts(&document);
    if !opened_by_this_call {
        return parts_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    parts_result.and_then(|parts| {
        close_result?;
        Ok(parts)
    })
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

/// Parcourt tous les composants d'un assemblage et regroupe les occurrences de pièces.
///
/// `GetComponents(false)` inclut les composants des sous-assemblages. Les composants supprimés
/// et les composants qui ne sont pas des pièces sont ignorés.
///
/// # Errors
/// Renvoie une erreur si SolidWorks ne peut pas fournir la liste ou les informations des composants.
fn read_assembly_parts(document: &IDispatch) -> Result<Vec<AssemblyPart>> {
    let mut arguments = [variant_bool(false)];
    let mut components_variant = invoke(
        document,
        "GetComponents",
        DISPATCH_METHOD,
        &mut arguments,
        true,
    )?;
    let components = dispatch_array_from_variant(&mut components_variant)?;
    let mut totals: HashMap<String, (String, String, usize)> = HashMap::new();

    for component in components {
        let suppression = invoke(&component, "GetSuppression", DISPATCH_METHOD, &mut [], true)?;
        if i32_from_variant(&suppression)? == 0 {
            continue;
        }

        let path = match invoke_string(&component, "GetPathName", DISPATCH_METHOD, &mut []) {
            Ok(path)
                if Path::new(&path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("sldprt")) =>
            {
                path
            }
            _ => continue,
        };
        let key = path.to_lowercase();
        let name = Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path)
            .to_owned();
        let entry = totals.entry(key).or_insert((name, path, 0));
        entry.2 += 1;
    }

    let mut parts: Vec<AssemblyPart> = totals
        .into_values()
        .map(|(name, path, quantity)| AssemblyPart {
            name,
            path,
            quantity,
        })
        .collect();
    parts.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });
    Ok(parts)
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
