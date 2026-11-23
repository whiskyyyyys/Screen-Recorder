use std::ffi::c_char;

pub const OBS_RUST_PLUGIN_API_VERSION: u32 = 1;

/// C-ABI v1 plugin descriptor returned by `obs_rust_plugin_describe`.
#[repr(C)]
pub struct PluginDescriptorC {
    pub api_version: u32,
    pub id: *const c_char,
    pub name: *const c_char,
    pub version: *const c_char,
    pub author: *const c_char,
    pub description: *const c_char,
    pub filter_id: *const c_char,
    pub filter_name: *const c_char,
}

pub type FnPluginDescribe = unsafe extern "C" fn() -> PluginDescriptorC;
pub type FnPluginInit = unsafe extern "C" fn(config_json: *const c_char) -> i32;
pub type FnPluginProcessRgba =
    unsafe extern "C" fn(rgba_ptr: *mut u8, width: u32, height: u32, strength: f32, radius: f32);
pub type FnPluginOnEvent =
    unsafe extern "C" fn(event_name: *const c_char, payload_json: *const c_char);
pub type FnPluginShutdown = unsafe extern "C" fn();
