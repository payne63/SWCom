use windows::Win32::Graphics::Printing::{
    EnumPrintersW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_INFO_4W,
};

/// Retourne les imprimantes locales et réseau installées sur le poste.
pub(super) fn available_printers() -> windows::core::Result<Vec<String>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut required_bytes = 0_u32;
    let mut returned = 0_u32;

    // Le premier appel récupère la taille du tampon nécessaire.
    let _ = unsafe { EnumPrintersW(flags, None, 4, None, &mut required_bytes, &mut returned) };
    if required_bytes == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = vec![0_u8; required_bytes as usize];
    unsafe {
        EnumPrintersW(
            flags,
            None,
            4,
            Some(&mut buffer),
            &mut required_bytes,
            &mut returned,
        )?;
    }

    let printer_infos = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<PRINTER_INFO_4W>(), returned as usize)
    };
    let mut printers = Vec::with_capacity(printer_infos.len());
    for printer in printer_infos {
        let name = unsafe { printer.pPrinterName.to_string()? };
        if !name.is_empty() && !printers.contains(&name) {
            printers.push(name);
        }
    }
    printers.shrink_to_fit();
    Ok(printers)
}
