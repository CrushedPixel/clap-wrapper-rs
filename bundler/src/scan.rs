use crate::util::AUv2Id;
use anyhow::Result;
use clap_sys::{
    entry::clap_plugin_entry,
    factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory},
};
use std::{
    ffi::{CStr, CString, OsString, c_void},
    path::Path,
};

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PluginLibrary {
    pub plugins: Vec<PluginInfo>,
    pub has_vst3_entry: bool,
    pub has_auv2_entry: bool,
}

/// Information about a CLAP plugin.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PluginInfo {
    pub clap_id: String,
    pub clap_name: String,
    pub clap_vendor: Option<String>,
    pub clap_version: Option<String>,
    pub clap_description: Option<String>,
    pub auv2_id: Option<AUv2Id>,
    /// Additional component identities retained for existing host sessions.
    pub auv2_legacy_ids: Vec<AUv2Id>,
}

/// Balances successful entry initialization before the loaded library is dropped.
struct InitializedEntry {
    /// Deinitializes the plugin entry on both successful and failed scans.
    deinit: unsafe extern "C" fn(),
}

impl Drop for InitializedEntry {
    /// Releases the initialized entry while its library is still loaded.
    fn drop(&mut self) {
        unsafe { (self.deinit)() };
    }
}

impl PluginLibrary {
    /// Scan the given path for CLAP plugins and return their information.
    pub fn scan(path: &Path) -> Result<Self> {
        unsafe {
            let library = libloading::Library::new(path)?;

            let clap_entry = &**library
                .get::<*const clap_plugin_entry>(c"clap_entry")
                .map_err(|_| anyhow::anyhow!("Failed to find clap_entry symbol"))?;

            let has_vst3_entry = library.get::<*const c_void>(c"GetPluginFactory").is_ok();
            let has_auv2_entry = library
                .get::<*const c_void>(c"GetPluginFactoryAUV2")
                .is_ok();

            let clap_init = clap_entry
                .init
                .ok_or_else(|| anyhow::anyhow!("clap_entry missing init function"))?;

            let clap_deinit = clap_entry
                .deinit
                .ok_or_else(|| anyhow::anyhow!("clap_entry missing deinit function"))?;

            let clap_get_factory = clap_entry
                .get_factory
                .ok_or_else(|| anyhow::anyhow!("clap_entry missing get_factory function"))?;

            if !clap_init(CString::new(OsString::from(path).as_encoded_bytes())?.as_ptr()) {
                anyhow::bail!("Plugin entry initialization failed");
            }
            let _entry = InitializedEntry {
                deinit: clap_deinit,
            };

            let plugin_factory =
                clap_get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;

            let plugin_factory = if plugin_factory.is_null() {
                return Err(anyhow::anyhow!(
                    "clap_entry::get_factory returned null for CLAP_PLUGIN_FACTORY_ID"
                ));
            } else {
                &*plugin_factory
            };

            let plugin_factory_as_auv2 =
                clap_get_factory(sys::CLAP_PLUGIN_FACTORY_INFO_AUV2.as_ptr())
                    as *const sys::clap_plugin_factory_as_auv2;

            let plugin_factory_as_auv2 = if plugin_factory_as_auv2.is_null() {
                None
            } else {
                Some(&*plugin_factory_as_auv2)
            };

            let legacy_factory =
                clap_get_factory(sys::CLAP_PLUGIN_FACTORY_INFO_AUV2_LEGACY.as_ptr())
                    as *const sys::LegacyFactory;

            let clap_get_plugin_count = plugin_factory.get_plugin_count.ok_or_else(|| {
                anyhow::anyhow!("clap_plugin_factory missing get_plugin_count function")
            })?;

            let clap_get_plugin_descriptor =
                plugin_factory.get_plugin_descriptor.ok_or_else(|| {
                    anyhow::anyhow!("clap_plugin_factory missing get_plugin_descriptor function")
                })?;

            let plugin_count = clap_get_plugin_count(plugin_factory as *const _);
            if plugin_count == 0 {
                return Err(anyhow::anyhow!(
                    "clap_plugin_factory::get_plugin_count returned 0"
                ));
            }

            let plugins = (0..plugin_count)
                .map(|i| {
                    let descriptor = clap_get_plugin_descriptor(plugin_factory as *const _, i);
                    let descriptor = if descriptor.is_null() {
                        return Err(anyhow::anyhow!(
                            "clap_plugin_factory::get_plugin_descriptor returned null for index {i}"
                        ));
                    } else {
                        &*descriptor
                    };

                    let auv2_info = match plugin_factory_as_auv2 {
                        Some(factory) => {
                            let get_auv2_info = factory.get_auv2_info.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "clap_plugin_factory_as_auv2 missing get_auv2_info function"
                                )
                            })?;

                            let mut info = std::mem::zeroed();
                            if get_auv2_info(factory as *const _ as *mut _, i, &mut info) {
                                Some(info)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    let auv2_code_subtype = auv2_info
                        .and_then(|info| parse_auv2_code(info.au_subt.as_ptr()).transpose())
                        .transpose()?;
                    let auv2_code_type = auv2_info
                        .and_then(|info| parse_auv2_code(info.au_type.as_ptr()).transpose())
                        .transpose()?;
                    let auv2_code_manu = plugin_factory_as_auv2
                        .and_then(|factory| {
                            parse_auv2_code(factory.manufacturer_code as *const _).transpose()
                        })
                        .transpose()?;

                    Ok(PluginInfo {
                        auv2_legacy_ids: read_legacy_ids(legacy_factory.as_ref(), i)?,
                        clap_id: cstr_to_string(descriptor.id)?.unwrap_or_default(),
                        clap_name: cstr_to_string(descriptor.name)?.unwrap_or_default(),
                        clap_vendor: cstr_to_string(descriptor.vendor)?,
                        clap_version: cstr_to_string(descriptor.version)?,
                        clap_description: cstr_to_string(descriptor.description)?,

                        auv2_id: match (auv2_code_manu, auv2_code_type, auv2_code_subtype) {
                            (Some(manu), Some(type_), Some(subtype)) => Some(AUv2Id {
                                manufacturer: manu,
                                type_,
                                subtype,
                            }),
                            _ => None,
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(PluginLibrary {
                plugins,
                has_auv2_entry,
                has_vst3_entry,
            })
        }
    }
}

/// Copies optional legacy factory identities before the plugin library is unloaded.
unsafe fn read_legacy_ids(
    factory: Option<&sys::LegacyFactory>,
    plugin_index: u32,
) -> Result<Vec<AUv2Id>> {
    let Some(factory) = factory else {
        return Ok(Vec::new());
    };
    let count = factory
        .count
        .ok_or_else(|| anyhow::anyhow!("AUv2 legacy factory missing count callback"))?;
    let get = factory
        .get
        .ok_or_else(|| anyhow::anyhow!("AUv2 legacy factory missing get callback"))?;
    let mut identities = Vec::new();
    for index in 0..unsafe { count(factory, plugin_index) } {
        let mut identity = sys::LegacyIdentity::default();
        if unsafe { get(factory, plugin_index, index, &mut identity) } {
            identities.push(AUv2Id {
                type_: legacy_code(identity.type_)?,
                subtype: legacy_code(identity.subtype)?,
                manufacturer: legacy_code(identity.manufacturer)?,
            });
        }
    }
    Ok(identities)
}

/// Requires a complete, printable four-character code with a trailing null.
fn legacy_code(code: [u8; 5]) -> Result<[u8; 4]> {
    if code[4] != 0 || !code[..4].iter().all(|byte| (b' '..=b'~').contains(byte)) {
        anyhow::bail!("AUv2 legacy identity must contain three four-character ASCII codes");
    }
    Ok(code[..4].try_into().unwrap())
}

/// Copies a nullable plugin descriptor string into owned storage.
unsafe fn cstr_to_string(cstr: *const i8) -> Result<Option<String>> {
    if cstr.is_null() {
        return Ok(None);
    }

    unsafe { Ok(Some(CStr::from_ptr(cstr).to_str()?.to_string())) }
}

/// Reads an optional primary AU component code.
unsafe fn parse_auv2_code(code: *const u8) -> Result<Option<[u8; 4]>> {
    if code.is_null() {
        Ok(None)
    } else {
        let len = (0..4)
            .position(|i| unsafe { *code.add(i) } == 0)
            .unwrap_or(4);
        if len == 4 {
            unsafe { Ok(Some((code as *const [u8; 4]).read())) }
        } else if len == 0 {
            Ok(None)
        } else {
            anyhow::bail!("AUv2 code contains a null byte")
        }
    }
}

#[allow(non_camel_case_types)]
mod sys {
    use std::ffi::{CStr, c_char};

    /// Optional additional component identity protocol from clapwrapper/auv2.h.
    pub const CLAP_PLUGIN_FACTORY_INFO_AUV2_LEGACY: &CStr =
        c"clap.plugin-factory-info-as-auv2-legacy/0";

    /// Retained component identity in the wrapper's C layout.
    #[repr(C)]
    #[derive(Default)]
    pub struct LegacyIdentity {
        /// Component type, including a trailing null.
        pub type_: [u8; 5],
        /// Component subtype, including a trailing null.
        pub subtype: [u8; 5],
        /// Manufacturer code, including a trailing null.
        pub manufacturer: [u8; 5],
    }

    /// Optional legacy factory callbacks in the wrapper's C layout.
    #[repr(C)]
    pub struct LegacyFactory {
        /// Number of retained identities for a plugin index.
        pub count: Option<unsafe extern "C" fn(*const LegacyFactory, u32) -> u32>,
        /// Copies an identity, or returns false to skip it.
        pub get: Option<
            unsafe extern "C" fn(*const LegacyFactory, u32, u32, *mut LegacyIdentity) -> bool,
        >,
    }

    pub const CLAP_PLUGIN_FACTORY_INFO_AUV2: &CStr = c"clap.plugin-factory-info-as-auv2.draft0";

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct clap_plugin_info_as_auv2 {
        pub au_type: [u8; 5],
        pub au_subt: [u8; 5],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct clap_plugin_factory_as_auv2 {
        pub manufacturer_code: *const c_char,
        pub manufacturer_name: *const c_char,
        pub get_auv2_info: Option<
            unsafe extern "C" fn(
                factory: *mut clap_plugin_factory_as_auv2,
                index: u32,
                info: *mut clap_plugin_info_as_auv2,
            ) -> bool,
        >,
    }
}

#[cfg(test)]
#[path = "../tests/unit/scan.rs"]
mod tests;
