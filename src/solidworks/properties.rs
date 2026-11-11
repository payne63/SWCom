use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
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
    let parts_result = read_assembly_components(&document, file_path);
    if !opened_by_this_call {
        return parts_result;
    }

    let close_result = close_document(sw_app, &document, file_path);
    parts_result.and_then(|parts| {
        close_result?;
        Ok(parts)
    })
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
fn read_assembly_components(document: &IDispatch, master_path: &str) -> Result<Vec<AssemblyItem>> {
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

    let master_name = Path::new(master_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(master_path)
        .to_owned();

    totals.insert(
        master_path.to_lowercase(),
        AssemblyItem {
            kind: AssemblyItemKind::Assembly,
            name: master_name,
            path: master_path.to_owned(),
            quantity: 1,
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
        let entry = totals.entry(key).or_insert(AssemblyItem {
            kind,
            name,
            path,
            quantity: 0,
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
    let mut grouped: HashMap<String, (IDispatch, String, AssemblyItemKind, usize)> = HashMap::new();
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

        if let Some((_, _, _, quantity)) = grouped.get_mut(&key) {
            *quantity += 1;
        } else {
            grouped.insert(key, (component, path, kind, 1));
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
        .map(|(component, path, kind, quantity)| {
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
        quantity: 0,
    });
    item.quantity = item.quantity.saturating_add(total_quantity);

    for child in &node.children {
        accumulate_tree_quantities(child, total_quantity, totals);
    }
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

    use super::{accumulate_tree_quantities, AssemblyItemKind, AssemblyTreeNode};

    #[test]
    fn flattens_tree_quantities_through_repeated_subassemblies() {
        let tree = AssemblyTreeNode {
            kind: AssemblyItemKind::Assembly,
            name: "master.sldasm".to_owned(),
            path: "C:/master.sldasm".to_owned(),
            quantity: 1,
            children: vec![AssemblyTreeNode {
                kind: AssemblyItemKind::Assembly,
                name: "sub.sldasm".to_owned(),
                path: "C:/sub.sldasm".to_owned(),
                quantity: 2,
                children: vec![AssemblyTreeNode {
                    kind: AssemblyItemKind::Part,
                    name: "part.sldprt".to_owned(),
                    path: "C:/part.sldprt".to_owned(),
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
    }
}
