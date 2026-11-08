use std::ptr;
use windows::core::{GUID, HRESULT, Result, w};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, DISPATCH_METHOD, DISPPARAMS, IDispatch,};


fn main() -> std::result::Result<(), windows::core::Error> {

    match unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) } {
        Ok(_) => {
            println!("✅ COM initialized successfully.");
        },
        Err(e) => {
            eprintln!("❌ Failed to initialize COM: {:?}", e);
            return Err(e);
        }
    }

    println!("Attempting to create SolidWorks Application object...");
    let clsid = get_solidworks_clsid()?;     
    let sw_app: IDispatch = match unsafe {CoCreateInstance::<_,IDispatch>(&clsid, None, CLSCTX_LOCAL_SERVER)}{    
        Ok(sw_app) => {
            println!("✅ instanciation successfully.");
            sw_app
        },
        Err(e) => {
            eprintln!("❌ Failed to instanciation {:?}", e);
            panic!()
            // Err(e.into())
        }
    };
    
    println!("--- try to close SolidWorks instance ---");
    match close_solidworks(&sw_app)
    {
        Ok(_) => print!("✅ SolidWorks closed."),
        Err(e) => eprintln!("❌ Failed to close SolidWorks {:?}", e)
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

fn close_solidworks(sw_app: &IDispatch) -> Result<()> {
    let method_name = w!("ExitApp");
    let mut dispid = 0;

    unsafe {
        // 1. Récupérer le DISPID de la méthode "ExitApp"
        sw_app.GetIDsOfNames(
            &GUID::zeroed(),
            &method_name,
            1,
            0x0400, // LOCALE_USER_DEFAULT
            &mut dispid,
        )?;

        // 2. Préparer des paramètres vides (ExitApp ne prend aucun argument)
        let mut dispparams = DISPPARAMS {
            rgvarg: ptr::null_mut(),
            rgdispidNamedArgs: ptr::null_mut(),
            cArgs: 0,
            cNamedArgs: 0,
        };

        // 3. Invoquer ExitApp
        sw_app.Invoke(
            dispid,
            &GUID::zeroed(),
            0x0400,
            DISPATCH_METHOD,
            &mut dispparams,
            None,
            None,
            None,
        )?;
    }

    Ok(())
}