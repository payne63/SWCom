use windows::core::{w, ComInterface, IUnknown, Result};
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, DISPATCH_METHOD,
};
use windows::Win32::System::Ole::GetActiveObject;

use super::{
    dispatch,
    properties::{AssemblyItem, CustomProperty},
};

/// Représente l'initialisation COM sur le thread courant.
///
/// L'appartement COM est automatiquement libéré à la destruction de cette valeur.
pub struct ComApartment;

impl ComApartment {
    /// Initialise COM en mode appartement à thread unique (STA).
    ///
    /// # Errors
    /// Renvoie l'erreur retournée par `CoInitializeEx` si l'initialisation échoue.
    pub fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED)? };
        Ok(Self)
    }
}

impl Drop for ComApartment {
    /// Libère l'initialisation COM associée à cette valeur.
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Représente l'instance SolidWorks connectée par l'application.
pub struct SolidWorksApp {
    dispatch: IDispatch,
    started_by_program: bool,
}

impl SolidWorksApp {
    /// Se connecte à SolidWorks s'il est déjà actif, ou le démarre sinon.
    ///
    /// Cette méthode doit être appelée après l'initialisation de COM sur le thread courant.
    /// L'instance est mémorisée afin que [`shutdown`](Self::shutdown) ne ferme que celle
    /// démarrée par ce programme.
    ///
    /// # Errors
    /// Renvoie une erreur si le ProgID ne peut pas être résolu, si SolidWorks ne peut pas
    /// être démarré, ou si l'interface `IDispatch` ne peut pas être obtenue.
    pub fn connect() -> Result<Self> {
        let clsid = unsafe { CLSIDFromProgID(w!("SldWorks.Application"))? };
        let mut running_instance: Option<IUnknown> = None;

        if unsafe { GetActiveObject(&clsid, None, &mut running_instance) }.is_ok() {
            if let Some(running_instance) = running_instance {
                return Ok(Self {
                    dispatch: running_instance.cast()?,
                    started_by_program: false,
                });
            }
        }

        let dispatch =
            unsafe { CoCreateInstance::<_, IDispatch>(&clsid, None, CLSCTX_LOCAL_SERVER)? };
        Ok(Self {
            dispatch,
            started_by_program: true,
        })
    }

    /// Récupère les propriétés personnalisées générales d'une pièce ou d'un assemblage.
    ///
    /// # Arguments
    /// * `file_path` — chemin d'un fichier `.sldprt` ou `.sldasm`.
    ///
    /// # Errors
    /// Renvoie une erreur si le format du fichier est invalide ou si un appel SolidWorks échoue.
    pub fn get_file_properties(&self, file_path: &str) -> Result<Vec<CustomProperty>> {
        super::properties::get_file_properties(&self.dispatch, file_path)
    }

    /// Met à jour les propriétés personnalisées existantes et sauvegarde le document.
    ///
    /// # Arguments
    /// * `file_path` — chemin d'un fichier `.sldprt` ou `.sldasm`.
    /// * `properties` — propriétés modifiées à écrire.
    ///
    /// # Errors
    /// Renvoie une erreur si une propriété ne peut pas être modifiée ou si la sauvegarde échoue.
    pub fn update_file_properties(
        &self,
        file_path: &str,
        properties: &[CustomProperty],
    ) -> Result<()> {
        super::properties::update_file_properties(&self.dispatch, file_path, properties)
    }

    /// Retourne les pièces, en distinguant la tôlerie, et les assemblages avec leurs quantités.
    ///
    /// # Arguments
    /// * `file_path` — chemin d'un fichier `.sldasm`.
    ///
    /// # Errors
    /// Renvoie une erreur si le document n'est pas un assemblage ou si un appel SolidWorks échoue.
    pub fn get_assembly_components(&self, file_path: &str) -> Result<Vec<AssemblyItem>> {
        super::properties::get_assembly_components(&self.dispatch, file_path)
    }

    /// Ferme SolidWorks uniquement si [`connect`](Self::connect) l'a démarré.
    ///
    /// Ne fait rien si l'instance était déjà active avant la connexion.
    ///
    /// # Errors
    /// Renvoie l'erreur COM si l'appel à `ExitApp` échoue.
    pub fn shutdown(&self) -> Result<()> {
        if self.started_by_program {
            dispatch::invoke(&self.dispatch, "ExitApp", DISPATCH_METHOD, &mut [], false)?;
        }
        Ok(())
    }
}
