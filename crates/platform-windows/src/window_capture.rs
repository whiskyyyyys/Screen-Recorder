use crate::display_capture::draw_cursor_on_dc;
use obs_core::{ObsError, Result};
use obs_video::{bgra_to_rgba_vec, VideoFrame};
use serde::{Deserialize, Serialize};
use std::mem::size_of;
use std::path::Path;
use std::ptr;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC,
    DeleteObject, GetDC, ReleaseDC, SelectObject, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HBRUSH, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetClientRect, GetMessageW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, IsWindowVisible, PostMessageW, PostQuitMessage, RegisterClassExW, ShowWindow,
    TranslateMessage, UnregisterClassW, CS_HREDRAW, CS_VREDRAW, HCURSOR, HICON, HMENU, MSG,
    SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub hwnd_raw: isize,
    pub pid: u32,
    pub title: String,
    pub process_name: String,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
}

impl WindowInfo {
    pub fn display_label(&self) -> String {
        if self.process_name.is_empty() {
            self.title.clone()
        } else {
            format!(
                "[{}] {} ({}x{})",
                self.process_name, self.title, self.width, self.height
            )
        }
    }
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return BOOL(1);
    }

    let mut cloaked: u32 = 0;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return BOOL(1);
    }

    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return BOOL(1);
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    let copied = GetWindowTextW(hwnd, &mut buf);
    if copied <= 0 {
        return BOOL(1);
    }
    let title = String::from_utf16_lossy(&buf[..copied as usize]);
    if title.trim().is_empty() {
        return BOOL(1);
    }

    let mut rect = RECT::default();
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut rect as *mut RECT as *mut _,
        size_of::<RECT>() as u32,
    )
    .is_err()
    {
        let _ = GetWindowRect(hwnd, &mut rect);
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    if w < 16 || h < 16 {
        return BOOL(1);
    }

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let process_name = query_process_exe_name(pid);

    let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    list.push(WindowInfo {
        hwnd_raw: hwnd.0 as isize,
        pid,
        title,
        process_name,
        width: w,
        height: h,
        left: rect.left,
        top: rect.top,
    });
    BOOL(1)
}

pub(crate) fn query_process_exe_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    unsafe {
        if let Ok(proc_h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = vec![0u16; 512];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                proc_h,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            );
            let _ = windows::Win32::Foundation::CloseHandle(proc_h);
            if ok.is_ok() && size > 0 {
                let path = String::from_utf16_lossy(&buf[..size as usize]);
                return Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    String::new()
}

pub fn enumerate_windows() -> Vec<WindowInfo> {
    let mut list = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_cb),
            LPARAM(&mut list as *mut Vec<WindowInfo> as isize),
        );
    }
    list
}

pub fn window_content_rect(hwnd: HWND, client_area_only: bool) -> Option<RECT> {
    unsafe {
        if client_area_only {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_ok() {
                let mut tl = POINT { x: 0, y: 0 };
                if ClientToScreen(hwnd, &mut tl).as_bool() {
                    let cw = (client.right - client.left).max(0);
                    let ch = (client.bottom - client.top).max(0);
                    if cw >= 8 && ch >= 8 {
                        return Some(RECT {
                            left: tl.x,
                            top: tl.y,
                            right: tl.x + cw,
                            bottom: tl.y + ch,
                        });
                    }
                }
            }
        }
        let mut rect = RECT::default();
        let got_rect = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(hwnd, &mut rect).is_ok();
        if got_rect && rect.right - rect.left >= 8 && rect.bottom - rect.top >= 8 {
            return Some(rect);
        }
    }
    None
}

pub(crate) fn is_frame_all_black(rgba: &[u8]) -> bool {
    let pixel_count = rgba.len() / 4;
    if pixel_count == 0 {
        return true;
    }
    let step = (pixel_count / 64).max(1);
    for p in (0..pixel_count).step_by(step) {
        let i = p * 4;
        if rgba[i] > 2 || rgba[i + 1] > 2 || rgba[i + 2] > 2 {
            return false;
        }
    }
    true
}

pub struct WindowCapturer {
    hwnd: Option<HWND>,
    target_title: String,
    target_process: String,
    capture_cursor: bool,
    client_area_only: bool,
    capture_method: String,
    match_priority: String,
    last_valid_frame: Option<VideoFrame>,
    seq: u64,
}

impl WindowCapturer {
    pub fn new(
        target_title: String,
        target_process: String,
        capture_cursor: bool,
        client_area_only: bool,
    ) -> Self {
        Self::new_with_options(
            target_title,
            target_process,
            capture_cursor,
            client_area_only,
            "Auto".to_string(),
            "TitleThenExe".to_string(),
        )
    }

    pub fn new_with_options(
        target_title: String,
        target_process: String,
        capture_cursor: bool,
        client_area_only: bool,
        capture_method: String,
        match_priority: String,
    ) -> Self {
        let mut cap = Self {
            hwnd: None,
            target_title,
            target_process,
            capture_cursor,
            client_area_only,
            capture_method,
            match_priority,
            last_valid_frame: None,
            seq: 0,
        };
        cap.resolve_hwnd();
        cap
    }

    pub fn resolve_hwnd(&mut self) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            if unsafe { IsWindow(h).as_bool() && IsWindowVisible(h).as_bool() } {
                return Some(h);
            }
        }
        let wins = enumerate_windows();
        let matched = match self.match_priority.as_str() {
            "ExactTitle" => wins
                .iter()
                .find(|w| !self.target_title.is_empty() && w.title == self.target_title),
            "ExeOnly" => wins.iter().find(|w| {
                !self.target_process.is_empty()
                    && w.process_name.eq_ignore_ascii_case(&self.target_process)
            }),
            _ => wins
                .iter()
                .find(|w| !self.target_title.is_empty() && w.title == self.target_title)
                .or_else(|| {
                    wins.iter().find(|w| {
                        !self.target_process.is_empty()
                            && w.process_name.eq_ignore_ascii_case(&self.target_process)
                    })
                })
                .or_else(|| {
                    wins.iter().find(|w| {
                        !self.target_title.is_empty()
                            && w.title
                                .to_ascii_lowercase()
                                .contains(&self.target_title.to_ascii_lowercase())
                    })
                }),
        };
        self.hwnd = matched.map(|w| HWND(w.hwnd_raw as *mut std::ffi::c_void));
        self.hwnd
    }

    pub fn capture_frame(&mut self, pts_us: u64) -> Result<Option<VideoFrame>> {
        let Some(hwnd) = self.resolve_hwnd() else {
            return Ok(self.last_valid_frame.clone());
        };

        unsafe {
            if IsIconic(hwnd).as_bool() {
                return Ok(self.last_valid_frame.clone());
            }

            let mut rect = RECT::default();
            let mut origin_pt = POINT::default();
            if self.client_area_only {
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = ClientToScreen(hwnd, &mut origin_pt);
            } else {
                let _ = GetWindowRect(hwnd, &mut rect);
                origin_pt = POINT {
                    x: rect.left,
                    y: rect.top,
                };
            }
            let w = (rect.right - rect.left).max(0);
            let h = (rect.bottom - rect.top).max(0);
            if w < 8 || h < 8 {
                return Ok(self.last_valid_frame.clone());
            }

            let win_dc = GetDC(hwnd);
            if win_dc.is_invalid() {
                return Err(ObsError::Capture("GetDC(hwnd) failed".to_string()));
            }
            let mem_dc = CreateCompatibleDC(win_dc);
            if mem_dc.is_invalid() {
                let _ = ReleaseDC(hwnd, win_dc);
                return Err(ObsError::Capture("CreateCompatibleDC failed".to_string()));
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                bmiColors: [Default::default()],
            };

            let mut bits_ptr: *mut std::ffi::c_void = ptr::null_mut();
            let dib: HBITMAP = CreateDIBSection(
                mem_dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits_ptr,
                HANDLE::default(),
                0,
            )
            .map_err(|e| {
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(hwnd, win_dc);
                ObsError::Capture(format!("CreateDIBSection: {e}"))
            })?;

            let old = SelectObject(mem_dc, dib);

            // PW_RENDERFULLCONTENT (2) + optional PW_CLIENTONLY (1)
            let pw_flags = if self.client_area_only {
                PRINT_WINDOW_FLAGS(2 | 1)
            } else {
                PRINT_WINDOW_FLAGS(2)
            };
            let mut ok = match self.capture_method.as_str() {
                "BitBlt" => BitBlt(mem_dc, 0, 0, w, h, win_dc, 0, 0, SRCCOPY).is_ok(),
                "PrintWindow" => PrintWindow(hwnd, mem_dc, pw_flags).as_bool(),
                _ => {
                    let pw_ok = PrintWindow(hwnd, mem_dc, pw_flags).as_bool();
                    if !pw_ok {
                        BitBlt(mem_dc, 0, 0, w, h, win_dc, 0, 0, SRCCOPY).is_ok()
                    } else if !bits_ptr.is_null() {
                        let len = (w as usize) * (h as usize) * 4;
                        let bgra = std::slice::from_raw_parts(bits_ptr as *const u8, len);
                        if is_frame_all_black(bgra) {
                            let blt_ok = BitBlt(mem_dc, 0, 0, w, h, win_dc, 0, 0, SRCCOPY).is_ok();
                            blt_ok || pw_ok
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                }
            };

            if self.capture_cursor {
                draw_cursor_on_dc(mem_dc, origin_pt.x, origin_pt.y);
            }

            let frame = if ok && !bits_ptr.is_null() {
                let len = (w as usize) * (h as usize) * 4;
                let bgra = std::slice::from_raw_parts(bits_ptr as *const u8, len);
                let mut rgba = bgra_to_rgba_vec(bgra, w as u32, h as u32, (w * 4) as u32);
                for px in rgba.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                if is_frame_all_black(&rgba) && self.last_valid_frame.is_some() {
                    None
                } else {
                    self.seq += 1;
                    let vf = VideoFrame::new_rgba(w as u32, h as u32, rgba, pts_us, self.seq);
                    self.last_valid_frame = Some(vf.clone());
                    Some(vf)
                }
            } else {
                None
            };
            let _ = &mut ok;

            SelectObject(mem_dc, old);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(hwnd, win_dc);
            Ok(frame.or_else(|| self.last_valid_frame.clone()))
        }
    }
}

#[doc(hidden)]
pub struct TestWindowHandle {
    hwnd_raw: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for TestWindowHandle {
    fn drop(&mut self) {
        if self.hwnd_raw != 0 {
            unsafe {
                let hwnd = HWND(self.hwnd_raw as *mut std::ffi::c_void);
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn test_capture_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[doc(hidden)]
pub fn spawn_test_capture_window(title: &str) -> Result<TestWindowHandle> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let title_owned = title.to_string();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<isize>>(1);

    let join = std::thread::Builder::new()
        .name(format!("obs-test-win-{seq}"))
        .spawn(move || unsafe {
            let class_str = format!("OxideStudioTestCaptureWin_{seq}");
            let class_h = HSTRING::from(&class_str);
            let title_h = HSTRING::from(&title_owned);
            let hinst = GetModuleHandleW(None)
                .map(|m| HINSTANCE(m.0))
                .unwrap_or_default();
            // COLORREF is 0x00BBGGRR -> R=20, G=200, B=40
            let brush: HBRUSH = CreateSolidBrush(COLORREF(0x0028_C814));

            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(test_capture_wnd_proc),
                hInstance: hinst,
                hIcon: HICON::default(),
                hCursor: HCURSOR::default(),
                hbrBackground: brush,
                lpszClassName: PCWSTR(class_h.as_ptr()),
                ..Default::default()
            };

            if RegisterClassExW(&wc) == 0 {
                let _ = DeleteObject(brush);
                let _ = tx.send(Err(ObsError::Capture("RegisterClassExW failed".into())));
                return;
            }

            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_h.as_ptr()),
                PCWSTR(title_h.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                120,
                120,
                320,
                240,
                HWND::default(),
                HMENU::default(),
                hinst,
                None,
            );

            let hwnd = match created {
                Ok(h) if !h.is_invalid() => h,
                Ok(_) => {
                    let _ = UnregisterClassW(PCWSTR(class_h.as_ptr()), hinst);
                    let _ = DeleteObject(brush);
                    let _ = tx.send(Err(ObsError::Capture(
                        "CreateWindowExW returned null".into(),
                    )));
                    return;
                }
                Err(e) => {
                    let _ = UnregisterClassW(PCWSTR(class_h.as_ptr()), hinst);
                    let _ = DeleteObject(brush);
                    let _ = tx.send(Err(ObsError::Capture(format!("CreateWindowExW: {e}"))));
                    return;
                }
            };

            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
            let _ = tx.send(Ok(hwnd.0 as isize));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = UnregisterClassW(PCWSTR(class_h.as_ptr()), hinst);
            let _ = DeleteObject(brush);
        })
        .map_err(|e| ObsError::Capture(format!("spawn test window thread: {e}")))?;

    let hwnd_raw = rx
        .recv()
        .map_err(|e| ObsError::Capture(format!("test window channel: {e}")))??;

    Ok(TestWindowHandle {
        hwnd_raw,
        thread: Some(join),
    })
}
