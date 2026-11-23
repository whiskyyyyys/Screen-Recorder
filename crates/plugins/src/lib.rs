pub mod abi;

pub use abi::{
    FnPluginDescribe, FnPluginInit, FnPluginOnEvent, FnPluginProcessRgba, FnPluginShutdown,
    PluginDescriptorC, OBS_RUST_PLUGIN_API_VERSION,
};

use libloading::Library;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub filter_id: Option<String>,
    pub filter_name: Option<String>,
    pub path: String,
    pub api_version: u32,
    pub loaded: bool,
    pub error: Option<String>,
}

struct LoadedPlugin {
    meta: PluginMetadata,
    enabled: bool,
    _lib: Library,
    fn_process_rgba: Option<FnPluginProcessRgba>,
    fn_on_event: Option<FnPluginOnEvent>,
    fn_shutdown: Option<FnPluginShutdown>,
}

pub struct PluginManager {
    plugins: Mutex<Vec<LoadedPlugin>>,
    rejected: Mutex<Vec<PluginMetadata>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Mutex::new(Vec::new()),
            rejected: Mutex::new(Vec::new()),
        }
    }

    pub fn discover_and_load(
        &self,
        extra_dirs: &[String],
        disabled_ids: &[String],
        configs: &HashMap<String, serde_json::Value>,
    ) {
        self.unload_all();

        let mut search_dirs = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                search_dirs.push(dir.join("plugins"));
            }
        }
        search_dirs.push(PathBuf::from("plugins"));
        for d in extra_dirs {
            if !d.trim().is_empty() {
                search_dirs.push(PathBuf::from(d));
            }
        }

        let mut seen_paths = std::collections::HashSet::new();
        for dir in search_dirs {
            if !dir.exists() {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("dll") {
                        let canon = p.canonicalize().unwrap_or(p.clone());
                        if seen_paths.insert(canon) {
                            self.load_dll(&p, disabled_ids, configs);
                        }
                    }
                }
            }
        }
    }

    pub fn load_dll(
        &self,
        path: &Path,
        disabled_ids: &[String],
        configs: &HashMap<String, serde_json::Value>,
    ) {
        unsafe {
            let lib = match Library::new(path) {
                Ok(l) => l,
                Err(e) => {
                    self.rejected.lock().push(PluginMetadata {
                        id: path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        version: "?".into(),
                        author: "?".into(),
                        description: "".into(),
                        filter_id: None,
                        filter_name: None,
                        path: path.to_string_lossy().into(),
                        api_version: 0,
                        loaded: false,
                        error: Some(format!("Failed loading DLL: {e}")),
                    });
                    return;
                }
            };

            let Ok(describe) = lib.get::<FnPluginDescribe>(b"obs_rust_plugin_describe") else {
                self.rejected.lock().push(PluginMetadata {
                    id: path.file_stem().unwrap_or_default().to_string_lossy().into(),
                    name: path.file_name().unwrap_or_default().to_string_lossy().into(),
                    version: "?".into(),
                    author: "?".into(),
                    description: "Not an OxideStudio plugin (missing obs_rust_plugin_describe)"
                        .into(),
                    filter_id: None,
                    filter_name: None,
                    path: path.to_string_lossy().into(),
                    api_version: 0,
                    loaded: false,
                    error: Some(
                        "Incompatible binary: missing OxideStudio C-ABI v1 symbols (upstream C/C++ OBS DLLs are not directly binary-compatible)"
                            .into(),
                    ),
                });
                return;
            };

            let desc = describe();
            let id = cstr_opt(desc.id).unwrap_or_else(|| "unknown".into());
            let name = cstr_opt(desc.name).unwrap_or_else(|| id.clone());
            let version = cstr_opt(desc.version).unwrap_or_else(|| "0.1.0".into());
            let author = cstr_opt(desc.author).unwrap_or_default();
            let description = cstr_opt(desc.description).unwrap_or_default();
            let filter_id = cstr_opt(desc.filter_id).filter(|s| !s.is_empty());
            let filter_name = cstr_opt(desc.filter_name).filter(|s| !s.is_empty());

            if desc.api_version != OBS_RUST_PLUGIN_API_VERSION {
                self.rejected.lock().push(PluginMetadata {
                    id,
                    name,
                    version,
                    author,
                    description,
                    filter_id,
                    filter_name,
                    path: path.to_string_lossy().into(),
                    api_version: desc.api_version,
                    loaded: false,
                    error: Some(format!(
                        "ABI version mismatch: plugin={}, host={}",
                        desc.api_version, OBS_RUST_PLUGIN_API_VERSION
                    )),
                });
                return;
            }

            let enabled = !disabled_ids.iter().any(|d| d == &id);
            if enabled {
                if let Ok(init_fn) = lib.get::<FnPluginInit>(b"obs_rust_plugin_init") {
                    let cfg_str = configs
                        .get(&id)
                        .and_then(|v| serde_json::to_string(v).ok())
                        .unwrap_or_else(|| "{}".to_string());
                    if let Ok(cstr) = CString::new(cfg_str) {
                        let rc = init_fn(cstr.as_ptr());
                        if rc != 0 {
                            self.rejected.lock().push(PluginMetadata {
                                id,
                                name,
                                version,
                                author,
                                description,
                                filter_id,
                                filter_name,
                                path: path.to_string_lossy().into(),
                                api_version: desc.api_version,
                                loaded: false,
                                error: Some(format!("Plugin init returned error code {rc}")),
                            });
                            return;
                        }
                    }
                }
            }

            let fn_process_rgba = lib
                .get::<FnPluginProcessRgba>(b"obs_rust_plugin_process_rgba")
                .ok()
                .map(|s| *s);
            let fn_on_event = lib
                .get::<FnPluginOnEvent>(b"obs_rust_plugin_on_event")
                .ok()
                .map(|s| *s);
            let fn_shutdown = lib
                .get::<FnPluginShutdown>(b"obs_rust_plugin_shutdown")
                .ok()
                .map(|s| *s);

            self.plugins.lock().push(LoadedPlugin {
                meta: PluginMetadata {
                    id,
                    name,
                    version,
                    author,
                    description,
                    filter_id,
                    filter_name,
                    path: path.to_string_lossy().into(),
                    api_version: desc.api_version,
                    loaded: enabled,
                    error: None,
                },
                enabled,
                _lib: lib,
                fn_process_rgba,
                fn_on_event,
                fn_shutdown,
            });
        }
    }

    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        let mut out: Vec<PluginMetadata> =
            self.plugins.lock().iter().map(|p| p.meta.clone()).collect();
        out.extend(self.rejected.lock().iter().cloned());
        out
    }

    pub fn available_video_filters(&self) -> Vec<(String, String, String)> {
        self.plugins
            .lock()
            .iter()
            .filter(|p| p.enabled)
            .filter_map(|p| {
                Some((
                    p.meta.id.clone(),
                    p.meta.filter_id.clone()?,
                    p.meta
                        .filter_name
                        .clone()
                        .unwrap_or_else(|| p.meta.name.clone()),
                ))
            })
            .collect()
    }

    pub fn process_video_filter(
        &self,
        plugin_id: &str,
        filter_id: &str,
        settings: &serde_json::Value,
        rgba: &mut [u8],
        width: u32,
        height: u32,
    ) {
        let expected = (width as usize) * (height as usize) * 4;
        if rgba.len() < expected {
            return;
        }
        let strength = settings
            .get("strength")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.55) as f32;
        let radius = settings
            .get("radius")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.75) as f32;

        let list = self.plugins.lock();
        for p in list.iter() {
            if p.enabled && p.meta.id == plugin_id && p.meta.filter_id.as_deref() == Some(filter_id)
            {
                if let Some(func) = p.fn_process_rgba {
                    unsafe {
                        func(rgba.as_mut_ptr(), width, height, strength, radius);
                    }
                }
            }
        }
    }

    pub fn dispatch_event(&self, event_name: &str, payload_json: &str) {
        let Ok(c_evt) = CString::new(event_name) else {
            return;
        };
        let Ok(c_pay) = CString::new(payload_json) else {
            return;
        };
        let list = self.plugins.lock();
        for p in list.iter() {
            if p.enabled {
                if let Some(cb) = p.fn_on_event {
                    unsafe {
                        cb(c_evt.as_ptr(), c_pay.as_ptr());
                    }
                }
            }
        }
    }

    pub fn unload_all(&self) {
        let mut list = self.plugins.lock();
        for p in list.drain(..) {
            if p.enabled {
                if let Some(sd) = p.fn_shutdown {
                    unsafe {
                        sd();
                    }
                }
            }
        }
        self.rejected.lock().clear();
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.unload_all();
    }
}

unsafe fn cstr_opt(ptr: *const std::ffi::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}
