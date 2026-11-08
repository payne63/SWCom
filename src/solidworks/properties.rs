use std::path::Path;
use windows::core::{Result, BSTR};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{IDispatch, DISPATCH_METHOD, DISPATCH_PROPERTYGET};

use super::dispatch::{
    byref_bool, byref_bstr, byref_i32, dispatch_from_variant, invoke, invoke_string,
    optional_dispatch_from_variant, string_array_from_variant, variant_bool, variant_bstr,
    variant_i32,
};

/// Une propriété personnalisée lue dans un document SolidWorks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomProperty {
    /// Nom de la propriété dans le document.
    pub name: String,
    /// Valeur résolue de la propriété, ou sa valeur brute si aucune résolution n'est disponible.
    pub value: String,
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
    let extension = Path::new(file_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let document_type = if extension.eq_ignore_ascii_case("sldprt") {
        1 // swDocPART
    } else if extension.eq_ignore_ascii_case("sldasm") {
        2 // swDocASSEMBLY
    } else {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x80070057_u32 as i32),
            "Le fichier doit être un .sldprt ou un .sldasm".into(),
        ));
    };

    let mut lookup_arguments = [variant_bstr(file_path)];
    let existing_document = invoke(
        sw_app,
        "GetOpenDocumentByName",
        DISPATCH_METHOD,
        &mut lookup_arguments,
        true,
    )?;

    let (document, opened_by_this_call) = if let Some(document) =
        optional_dispatch_from_variant(&existing_document)
    {
        (document, false)
    } else {
        let mut errors = 0_i32;
        let mut warnings = 0_i32;
        let mut arguments = vec![
            byref_i32(&mut warnings),
            byref_i32(&mut errors),
            variant_bstr(""),
            variant_i32(3), // swOpenDocOptions_Silent | swOpenDocOptions_ReadOnly
            variant_i32(document_type),
            variant_bstr(file_path),
        ];
        let opened_document = invoke(sw_app, "OpenDoc6", DISPATCH_METHOD, &mut arguments, true)?;
        (dispatch_from_variant(&opened_document)?, true)
    };

    let properties_result = read_custom_properties(&document);
    if !opened_by_this_call {
        return properties_result;
    }

    let title =
        invoke_string(&document, "GetTitle", DISPATCH_METHOD, &mut []).unwrap_or_else(|_| {
            Path::new(file_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(file_path)
                .to_owned()
        });
    let mut close_arguments = [variant_bstr(&title)];
    let close_result = invoke(
        sw_app,
        "CloseDoc",
        DISPATCH_METHOD,
        &mut close_arguments,
        false,
    );

    properties_result.and_then(|properties| {
        close_result?;
        Ok(properties)
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
    let mut no_arguments = [];
    let extension_variant = invoke(
        document,
        "Extension",
        DISPATCH_PROPERTYGET,
        &mut no_arguments,
        true,
    )?;
    let extension = dispatch_from_variant(&extension_variant)?;

    let mut manager_arguments = [variant_bstr("")];
    let manager_variant = invoke(
        &extension,
        "CustomPropertyManager",
        DISPATCH_PROPERTYGET,
        &mut manager_arguments,
        true,
    )?;
    let manager = dispatch_from_variant(&manager_variant)?;

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
