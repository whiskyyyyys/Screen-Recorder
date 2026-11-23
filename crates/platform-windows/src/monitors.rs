use serde::{Deserialize, Serialize};
use std::mem::size_of;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

const MONITORINFOF_PRIMARY_FLAG: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub index: usize,
    pub device_name: String,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

impl MonitorInfo {
    pub fn display_label(&self) -> String {
        let primary = if self.is_primary { " [Primary]" } else { "" };
        format!(
            "Display {}: {}x{} ({}, {}){}",
            self.index + 1,
            self.width,
            self.height,
            self.left,
            self.top,
            primary
        )
    }
}

unsafe extern "system" fn enum_monitor_cb(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmonitor, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        let rc = mi.monitorInfo.rcMonitor;
        let raw_name = &mi.szDevice;
        let len = raw_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(raw_name.len());
        let device_name = String::from_utf16_lossy(&raw_name[..len]);
        let idx = list.len();
        list.push(MonitorInfo {
            index: idx,
            device_name,
            left: rc.left,
            top: rc.top,
            width: (rc.right - rc.left).max(1) as u32,
            height: (rc.bottom - rc.top).max(1) as u32,
            is_primary: (mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY_FLAG) != 0,
        });
    }
    BOOL(1)
}

pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_monitor_cb),
            LPARAM(&mut list as *mut Vec<MonitorInfo> as isize),
        );
    }
    if list.is_empty() {
        list.push(MonitorInfo {
            index: 0,
            device_name: "\\\\.\\DISPLAY1".to_string(),
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
            is_primary: true,
        });
    }
    list
}
