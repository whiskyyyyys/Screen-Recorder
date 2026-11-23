use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, Ordering};

const API_VERSION: u32 = 1;

static INITIALIZED: AtomicBool = AtomicBool::new(false);

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

static ID: &[u8] = b"oxidestudio.sample.vignette\0";
static NAME: &[u8] = b"Cinema Vignette Filter\0";
static VERSION: &[u8] = b"1.0.0\0";
static AUTHOR: &[u8] = b"OxideStudio Contributors\0";
static DESC: &[u8] =
    b"Dynamic C-ABI v1 sample plugin adding radial cinema vignette to video sources.\0";
static FILTER_ID: &[u8] = b"cinema_vignette\0";
static FILTER_NAME: &[u8] = b"Plugin: Cinema Vignette\0";

#[no_mangle]
pub extern "C" fn obs_rust_plugin_describe() -> PluginDescriptorC {
    PluginDescriptorC {
        api_version: API_VERSION,
        id: ID.as_ptr() as *const c_char,
        name: NAME.as_ptr() as *const c_char,
        version: VERSION.as_ptr() as *const c_char,
        author: AUTHOR.as_ptr() as *const c_char,
        description: DESC.as_ptr() as *const c_char,
        filter_id: FILTER_ID.as_ptr() as *const c_char,
        filter_name: FILTER_NAME.as_ptr() as *const c_char,
    }
}

/// Initializes the plugin with JSON configuration.
///
/// # Safety
/// `config_json` must be either null or a valid NUL-terminated UTF-8 C string.
#[no_mangle]
pub unsafe extern "C" fn obs_rust_plugin_init(config_json: *const c_char) -> i32 {
    if !config_json.is_null() {
        let _ = CStr::from_ptr(config_json);
    }
    INITIALIZED.store(true, Ordering::Relaxed);
    0
}

/// Applies radial cinema vignette to an RGBA8 frame buffer in place.
///
/// # Safety
/// `rgba_ptr` must be valid for writes of `width * height * 4` bytes.
#[no_mangle]
pub unsafe extern "C" fn obs_rust_plugin_process_rgba(
    rgba_ptr: *mut u8,
    width: u32,
    height: u32,
    strength: f32,
    radius: f32,
) {
    if !INITIALIZED.load(Ordering::Relaxed) || rgba_ptr.is_null() || width == 0 || height == 0 {
        return;
    }
    let len = (width as usize) * (height as usize) * 4;
    let slice = std::slice::from_raw_parts_mut(rgba_ptr, len);
    let str_c = strength.clamp(0.0, 1.0);
    let rad_c = radius.clamp(0.2, 1.5);
    let inv_w = 1.0 / (width as f32);
    let inv_h = 1.0 / (height as f32);

    for y in 0..height {
        let ny = (y as f32 * inv_h) - 0.5;
        for x in 0..width {
            let nx = (x as f32 * inv_w) - 0.5;
            let dist = (nx * nx + ny * ny).sqrt() * 2.0; // 0 at center, ~1.41 at corners
            let t = ((dist - rad_c * 0.45) / (rad_c * 0.65).max(0.05)).clamp(0.0, 1.0);
            let falloff = 1.0 - (t * t * (3.0 - 2.0 * t)) * str_c;
            let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
            slice[idx] = ((slice[idx] as f32) * falloff) as u8;
            slice[idx + 1] = ((slice[idx + 1] as f32) * falloff) as u8;
            slice[idx + 2] = ((slice[idx + 2] as f32) * falloff) as u8;
        }
    }
}

/// Receives host engine event notifications.
///
/// # Safety
/// `event_name` and `payload_json` must be valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn obs_rust_plugin_on_event(
    _event_name: *const c_char,
    _payload_json: *const c_char,
) {
}

#[no_mangle]
pub extern "C" fn obs_rust_plugin_shutdown() {
    INITIALIZED.store(false, Ordering::Relaxed);
}
