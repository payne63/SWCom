use std::{mem::ManuallyDrop, ptr};
use windows::core::{Result, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{IDispatch, DISPATCH_FLAGS, DISPPARAMS, SAFEARRAY};
use windows::Win32::System::Ole::{SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VT_ARRAY, VT_BOOL, VT_BSTR, VT_BYREF, VT_DISPATCH, VT_I4,
};

/// Appelle un membre COM identifié par son nom via `IDispatch`.
///
/// Les arguments doivent être fournis dans l'ordre COM, donc du dernier paramètre au premier.
/// Si `return_value` vaut `false`, la valeur renvoyée par COM est ignorée.
///
/// # Errors
/// Renvoie une erreur si le membre ne peut pas être résolu ou si son invocation échoue.
pub(super) fn invoke(
    object: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
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

/// Appelle un membre COM et convertit son résultat `BSTR` en `String`.
///
/// # Errors
/// Renvoie une erreur si l'appel COM échoue ou si le résultat n'est pas un `BSTR`.
pub(super) fn invoke_string(
    object: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
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

/// Convertit un `VARIANT` contenant un tableau `SAFEARRAY` de `BSTR` en chaînes Rust.
///
/// Renvoie un vecteur vide si le variant ne contient pas un tableau de chaînes.
///
/// # Errors
/// Renvoie une erreur si les limites du tableau ou la lecture d'un élément échouent.
pub(super) fn string_array_from_variant(variant: &mut VARIANT) -> Result<Vec<String>> {
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        if raw.vt != VARENUM(VT_ARRAY.0 | VT_BSTR.0) {
            return Ok(Vec::new());
        }

        let array: *mut SAFEARRAY = raw.Anonymous.parray;
        let lower_bound = SafeArrayGetLBound(array, 1)?;
        let upper_bound = SafeArrayGetUBound(array, 1)?;

        let mut values = Vec::new();
        for index in lower_bound..=upper_bound {
            let mut value = BSTR::new();
            SafeArrayGetElement(array, &index, &mut value as *mut BSTR as *mut _)?;
            values.push(value.to_string());
        }
        Ok(values)
    }
}

/// Clone l'interface `IDispatch` d'un variant, ou renvoie `None` si elle est absente.
pub(super) fn optional_dispatch_from_variant(variant: &VARIANT) -> Option<IDispatch> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        if raw.vt != VT_DISPATCH {
            return None;
        }
        raw.Anonymous.pdispVal.as_ref().map(IDispatch::clone)
    }
}

/// Extrait une interface `IDispatch` d'un variant et en conserve une référence COM.
///
/// # Errors
/// Renvoie une erreur si le variant ne contient pas une interface `IDispatch` valide.
pub(super) fn dispatch_from_variant(variant: &VARIANT) -> Result<IDispatch> {
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

/// Construit un `VARIANT` propriétaire contenant une chaîne `BSTR`.
/// Lit un entier signé de 32 bits depuis un `VARIANT`.
///
/// # Errors
/// Renvoie une erreur si le variant n'est pas de type `VT_I4`.
pub(super) fn i32_from_variant(variant: &VARIANT) -> Result<i32> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        if raw.vt != VT_I4 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(raw.Anonymous.lVal)
    }
}

/// Lit un booléen depuis un `VARIANT`.
///
/// # Errors
/// Renvoie une erreur si le variant n'est pas de type `VT_BOOL`.
pub(super) fn bool_from_variant(variant: &VARIANT) -> Result<bool> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        if raw.vt != VT_BOOL {
            return Err(windows::core::Error::from_win32());
        }
        Ok(raw.Anonymous.boolVal.0 != 0)
    }
}

pub(super) fn variant_bstr(value: &str) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VT_BSTR;
        raw.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
    }
    variant
}

/// Construit un `VARIANT` contenant un entier COM signé de 32 bits (`VT_I4`).
pub(super) fn variant_i32(value: i32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VT_I4;
        raw.Anonymous.lVal = value;
    }
    variant
}

/// Construit un `VARIANT` contenant un booléen COM (`VT_BOOL`).
pub(super) fn variant_bool(value: bool) -> VARIANT {
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

/// Construit un `VARIANT` `VT_BSTR | VT_BYREF` vers une chaîne de sortie.
///
/// La chaîne pointée doit rester vivante et mutable jusqu'à la fin de l'appel COM.
pub(super) fn byref_bstr(value: &mut BSTR) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_BSTR.0 | VT_BYREF.0);
        raw.Anonymous.pbstrVal = value;
    }
    variant
}

/// Construit un `VARIANT` `VT_BOOL | VT_BYREF` vers un booléen de sortie.
///
/// La valeur pointée doit rester vivante et mutable jusqu'à la fin de l'appel COM.
pub(super) fn byref_bool(value: &mut VARIANT_BOOL) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_BOOL.0 | VT_BYREF.0);
        raw.Anonymous.pboolVal = value;
    }
    variant
}

/// Construit un `VARIANT` `VT_I4 | VT_BYREF` vers un entier de sortie.
///
/// La valeur pointée doit rester vivante et mutable jusqu'à la fin de l'appel COM.
pub(super) fn byref_i32(value: &mut i32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        let raw = &mut variant.Anonymous.Anonymous;
        raw.vt = VARENUM(VT_I4.0 | VT_BYREF.0);
        raw.Anonymous.plVal = value;
    }
    variant
}
