use std::{env, path::Path, ptr};
use windows::core::{w, ComInterface, IUnknown, Result, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPPARAMS, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    GetActiveObject, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VT_ARRAY, VT_BOOL, VT_BSTR, VT_BYREF, VT_DISPATCH, VT_I4,
};

fn main() -> std::result::Result<(), windows::core::Error> {
    match unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) } {
        Ok(_) => {
            println!("✅ COM initialized successfully.");
        }
        Err(e) => {
            eprintln!("❌ Failed to initialize COM: {:?}", e);
            return Err(e);
        }
    }

    println!("Connecting to SolidWorks...");
    let (sw_app, started_by_program) = get_or_start_solidworks()?;

    if let Some(file_path) = env::args().nth(1) {
        match get_file_properties(&sw_app, &file_path) {
            Ok(properties) => {
                println!("Properties for {file_path}:");
                for (name, value) in properties {
                    println!("{name}: {value}");
                }
            }
            Err(error) => eprintln!("❌ Failed to read file properties: {error:?}"),
        }
    } else {
        println!("Usage: solidworks_connector <file.sldprt|file.sldasm>");
    }

    if started_by_program {
        println!("--- try to close SolidWorks instance ---");
        match close_solidworks(&sw_app) {
            Ok(_) => print!("✅ SolidWorks closed."),
            Err(e) => eprintln!("❌ Failed to close SolidWorks {:?}", e),
        }
    } else {
        println!("SolidWorks was already running; leaving it open.");
    }

    unsafe { CoUninitialize() };
    println!("✅ COM uninitialized.");

    Ok(())
}

fn get_solidworks_clsid() -> Result<GUID> {
    unsafe {
        // Récupère le CLSID de la version par défaut installée sur la machine
        CLSIDFromProgID(w!("SldWorks.Application"))
    }
}

fn get_or_start_solidworks() -> Result<(IDispatch, bool)> {
    let clsid = get_solidworks_clsid()?;
    let mut running_instance: Option<IUnknown> = None;

    if unsafe { GetActiveObject(&clsid, None, &mut running_instance) }.is_ok() {
        if let Some(running_instance) = running_instance {
            return Ok((running_instance.cast()?, false));
        }
    }

    let sw_app = unsafe { CoCreateInstance::<_, IDispatch>(&clsid, None, CLSCTX_LOCAL_SERVER) }?;
    Ok((sw_app, true))
}

fn get_file_properties(sw_app: &IDispatch, file_path: &str) -> Result<Vec<(String, String)>> {
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

    // CloseDoc uses the document title, including its extension.
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

fn read_custom_properties(document: &IDispatch) -> Result<Vec<(String, String)>> {
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
            let property_value = if resolved.is_empty() {
                value.to_string()
            } else {
                resolved
            };
            Ok((name, property_value))
        })
        .collect()
}

fn string_array_from_variant(variant: &mut VARIANT) -> Result<Vec<String>> {
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        if raw.vt != VARENUM(VT_ARRAY.0 | VT_BSTR.0) {
            return Ok(Vec::new());
        }

        let array: *mut SAFEARRAY = raw.Anonymous.parray;
        let lower_bound = SafeArrayGetLBound(array, 1)?;
        let upper_bound = SafeArrayGetUBound(array, 1)?;

        let mut names = Vec::new();
        for index in lower_bound..=upper_bound {
            let mut name = BSTR::new();
            SafeArrayGetElement(array, &index, &mut name as *mut BSTR as *mut _)?;
            names.push(name.to_string());
        }
        Ok(names)
    }
}

fn invoke_string(
    object: &IDispatch,
    name: &str,
    flags: windows::Win32::System::Com::DISPATCH_FLAGS,
    arguments: &mut [VARIANT],
) -> Result<String> {
    let result = invoke(object, name, flags, arguments, true)?;
    unsafe {
        let raw = &result.Anonymous.Anonymous;
        if raw.vt != VT_BSTR {
            return Err(windows::core::Error::from_win32());
        }
        Ok(raw.Anonymous.bstrVal.to_string())
    }
}

fn invoke(
    object: &IDispatch,
    name: &str,
    flags: windows::Win32::System::Com::DISPATCH_FLAGS,
    arguments: &mut [VARIANT],
    return_value: bool,
) -> Result<VARIANT> {
    let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let name_pointer = PCWSTR(wide_name.as_ptr());
    let mut dispid = 0;
    unsafe {
        object.GetIDsOfNames(&GUID::zeroed(), &name_pointer, 1, 0x0400, &mut dispid)?;

        let mut parameters = DISPPARAMS {
            rgvarg: if arguments.is_empty() {
                ptr::null_mut()
            } else {
                arguments.as_mut_ptr()
            },
            rgdispidNamedArgs: ptr::null_mut(),
            cArgs: arguments.len() as u32,
            cNamedArgs: 0,
        };
        let mut result = VARIANT::default();
        object.Invoke(
            dispid,
            &GUID::zeroed(),
            0x0400,
            flags,
            &mut parameters,
            if return_value {
                Some(&mut result)
            } else {
                None
            },
            None,
            None,
        )?;
        Ok(result)
    }
}

fn optional_dispatch_from_variant(variant: &VARIANT) -> Option<IDispatch> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        if raw.vt != VT_DISPATCH {
            return None;
        }
        raw.Anonymous.pdispVal.as_ref().map(IDispatch::clone)
    }
}

fn dispatch_from_variant(variant: &VARIANT) -> Result<IDispatch> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        if raw.vt != VT_DISPATCH {
            return Err(windows::core::Error::from_win32());
        }
        raw.Anonymous
            .pdispVal
            .as_ref()
            .map(IDispatch::clone)
            .ok_or_else(windows::core::Error::from_win32)
    }
}

fn variant_bstr(value: &str) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VT_BSTR;
        raw.Anonymous.bstrVal = std::mem::ManuallyDrop::new(BSTR::from(value));
    }
    variant
}

fn variant_i32(value: i32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VT_I4;
        raw.Anonymous.lVal = value;
    }
    variant
}

fn variant_bool(value: bool) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VT_BOOL;
        raw.Anonymous.boolVal = if value {
            VARIANT_BOOL(-1)
        } else {
            VARIANT_BOOL(0)
        };
    }
    variant
}

fn byref_bstr(value: &mut BSTR) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_BSTR.0 | VT_BYREF.0);
        raw.Anonymous.pbstrVal = value;
    }
    variant
}

fn byref_bool(value: &mut VARIANT_BOOL) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_BOOL.0 | VT_BYREF.0);
        raw.Anonymous.pboolVal = value;
    }
    variant
}

fn byref_i32(value: &mut i32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_I4.0 | VT_BYREF.0);
        raw.Anonymous.plVal = value;
    }
    variant
}

fn close_solidworks(sw_app: &IDispatch) -> Result<()> {
    let mut no_arguments = [];
    invoke(sw_app, "ExitApp", DISPATCH_METHOD, &mut no_arguments, false)?;
    Ok(())
}
