use obs_settings::HotkeyBinding;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10, VK_F11,
    VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F2, VK_F20, VK_F21, VK_F22,
    VK_F23, VK_F24, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT, VK_LEFT,
    VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5,
    VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP,
};

#[derive(Debug, Clone)]
pub struct KeyTriggerEvent {
    pub action: String,
}

pub struct HotkeyListener {
    bindings: Arc<Mutex<Vec<HotkeyBinding>>>,
    enabled: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HotkeyListener {
    pub fn start<F>(bindings: Vec<HotkeyBinding>, enabled: bool, on_trigger: F) -> Self
    where
        F: Fn(String) + Send + 'static,
    {
        let bindings_arc = Arc::new(Mutex::new(bindings));
        let enabled_arc = Arc::new(AtomicBool::new(enabled));
        let running = Arc::new(AtomicBool::new(true));

        let b_clone = Arc::clone(&bindings_arc);
        let e_clone = Arc::clone(&enabled_arc);
        let r_clone = Arc::clone(&running);

        let handle = thread::Builder::new()
            .name("obs-hotkeys".to_string())
            .spawn(move || {
                let mut prev_active: HashSet<usize> = HashSet::new();
                while r_clone.load(Ordering::Relaxed) {
                    if e_clone.load(Ordering::Relaxed) {
                        let list = b_clone.lock().clone();
                        let ctrl = is_vk_down(VK_CONTROL.0 as i32);
                        let alt = is_vk_down(VK_MENU.0 as i32);
                        let shift = is_vk_down(VK_SHIFT.0 as i32);

                        let mut now_active = HashSet::new();
                        for (idx, bind) in list.iter().enumerate() {
                            if let Some(vk) = key_name_to_vk(&bind.key) {
                                let main_down = is_vk_down(vk);
                                if main_down
                                    && bind.ctrl == ctrl
                                    && bind.alt == alt
                                    && bind.shift == shift
                                {
                                    now_active.insert(idx);
                                    if !prev_active.contains(&idx) {
                                        on_trigger(bind.action.clone());
                                    }
                                }
                            }
                        }
                        prev_active = now_active;
                    } else {
                        prev_active.clear();
                    }
                    thread::sleep(Duration::from_millis(16));
                }
            })
            .ok();

        Self {
            bindings: bindings_arc,
            enabled: enabled_arc,
            running,
            handle,
        }
    }

    pub fn update(&self, bindings: Vec<HotkeyBinding>, enabled: bool) {
        *self.bindings.lock() = bindings;
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[inline]
fn is_vk_down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

pub fn key_name_to_vk(name: &str) -> Option<i32> {
    let up = name.trim().to_ascii_uppercase();
    if up.len() == 1 {
        let ch = up.as_bytes()[0];
        if ch.is_ascii_alphanumeric() {
            return Some(ch as i32);
        }
    }
    let vk = match up.as_str() {
        "F1" => VK_F1.0 as i32,
        "F2" => VK_F2.0 as i32,
        "F3" => VK_F3.0 as i32,
        "F4" => VK_F4.0 as i32,
        "F5" => VK_F5.0 as i32,
        "F6" => VK_F6.0 as i32,
        "F7" => VK_F7.0 as i32,
        "F8" => VK_F8.0 as i32,
        "F9" => VK_F9.0 as i32,
        "F10" => VK_F10.0 as i32,
        "F11" => VK_F11.0 as i32,
        "F12" => VK_F12.0 as i32,
        "F13" => VK_F13.0 as i32,
        "F14" => VK_F14.0 as i32,
        "F15" => VK_F15.0 as i32,
        "F16" => VK_F16.0 as i32,
        "F17" => VK_F17.0 as i32,
        "F18" => VK_F18.0 as i32,
        "F19" => VK_F19.0 as i32,
        "F20" => VK_F20.0 as i32,
        "F21" => VK_F21.0 as i32,
        "F22" => VK_F22.0 as i32,
        "F23" => VK_F23.0 as i32,
        "F24" => VK_F24.0 as i32,
        "SPACE" => VK_SPACE.0 as i32,
        "ENTER" | "RETURN" => VK_RETURN.0 as i32,
        "ESC" | "ESCAPE" => VK_ESCAPE.0 as i32,
        "TAB" => VK_TAB.0 as i32,
        "INSERT" => VK_INSERT.0 as i32,
        "DELETE" | "DEL" => VK_DELETE.0 as i32,
        "HOME" => VK_HOME.0 as i32,
        "END" => VK_END.0 as i32,
        "PAGEUP" => VK_PRIOR.0 as i32,
        "PAGEDOWN" => VK_NEXT.0 as i32,
        "UP" => VK_UP.0 as i32,
        "DOWN" => VK_DOWN.0 as i32,
        "LEFT" => VK_LEFT.0 as i32,
        "RIGHT" => VK_RIGHT.0 as i32,
        "NUMPAD0" | "NUM0" => VK_NUMPAD0.0 as i32,
        "NUMPAD1" | "NUM1" => VK_NUMPAD1.0 as i32,
        "NUMPAD2" | "NUM2" => VK_NUMPAD2.0 as i32,
        "NUMPAD3" | "NUM3" => VK_NUMPAD3.0 as i32,
        "NUMPAD4" | "NUM4" => VK_NUMPAD4.0 as i32,
        "NUMPAD5" | "NUM5" => VK_NUMPAD5.0 as i32,
        "NUMPAD6" | "NUM6" => VK_NUMPAD6.0 as i32,
        "NUMPAD7" | "NUM7" => VK_NUMPAD7.0 as i32,
        "NUMPAD8" | "NUM8" => VK_NUMPAD8.0 as i32,
        "NUMPAD9" | "NUM9" => VK_NUMPAD9.0 as i32,
        _ => return None,
    };
    Some(vk)
}
