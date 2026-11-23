use crate::display_capture::DisplayCapturer;
use crate::monitors::enumerate_monitors;
use crate::window_capture::{enumerate_windows, WindowCapturer};
use obs_core::Result;
use obs_video::VideoFrame;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

/// Non-invasive high-FPS Game Capture using cropped DXGI Output Duplication for fullscreen/foreground
/// games and automatic `PrintWindow` fallback when occluded. Avoids anti-cheat DLL injection hooks.
pub struct GameCapturer {
    mode: String,
    window_capturer: WindowCapturer,
    dxgi_capturer: Option<DisplayCapturer>,
    capture_cursor: bool,
    seq: u64,
}

impl GameCapturer {
    pub fn new(
        mode: String,
        window_title: String,
        process_name: String,
        capture_cursor: bool,
    ) -> Self {
        let dxgi_capturer = DisplayCapturer::new(0, capture_cursor, false).ok();
        let window_capturer = WindowCapturer::new(window_title, process_name, capture_cursor, true);
        Self {
            mode,
            window_capturer,
            dxgi_capturer,
            capture_cursor,
            seq: 0,
        }
    }

    pub fn capture_frame(&mut self, pts_us: u64) -> Result<Option<VideoFrame>> {
        if self.mode == "AnyFullscreen" {
            if let Some(full_rect) = detect_fullscreen_foreground_rect() {
                if let Some(dxgi) = self.dxgi_capturer.as_mut() {
                    if let Ok(Some(mon_frame)) = dxgi.capture_frame(8, pts_us) {
                        let mon = dxgi.monitor();
                        return Ok(Some(crop_frame_to_rect(
                            &mon_frame,
                            mon.left,
                            mon.top,
                            full_rect,
                            &mut self.seq,
                        )));
                    }
                }
            }
            // Fallback to foreground window if not matching exact monitor size
            unsafe {
                let fg = GetForegroundWindow();
                if !fg.is_invalid() {
                    let mut rect = RECT::default();
                    if GetWindowRect(fg, &mut rect).is_ok() {
                        if let Some(dxgi) = self.dxgi_capturer.as_mut() {
                            if let Ok(Some(mon_frame)) = dxgi.capture_frame(8, pts_us) {
                                let mon = dxgi.monitor();
                                return Ok(Some(crop_frame_to_rect(
                                    &mon_frame,
                                    mon.left,
                                    mon.top,
                                    rect,
                                    &mut self.seq,
                                )));
                            }
                        }
                    }
                }
            }
            let _ = self.capture_cursor;
            return Ok(None);
        }

        // SpecificWindow mode: use PrintWindow or cropped DXGI if foreground
        if let Some(hwnd) = self.window_capturer.resolve_hwnd() {
            let fg = unsafe { GetForegroundWindow() };
            if fg == hwnd {
                if let Some(dxgi) = self.dxgi_capturer.as_mut() {
                    let mut rect = RECT::default();
                    if unsafe { GetWindowRect(hwnd, &mut rect).is_ok() } {
                        if let Ok(Some(mon_frame)) = dxgi.capture_frame(8, pts_us) {
                            let mon = dxgi.monitor();
                            return Ok(Some(crop_frame_to_rect(
                                &mon_frame,
                                mon.left,
                                mon.top,
                                rect,
                                &mut self.seq,
                            )));
                        }
                    }
                }
            }
        }
        self.window_capturer.capture_frame(pts_us)
    }
}

fn detect_fullscreen_foreground_rect() -> Option<RECT> {
    let monitors = enumerate_monitors();
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return None;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(fg, &mut rect).is_err() } {
        return None;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    let is_fullscreen = monitors
        .iter()
        .any(|m| w >= (m.width as i32) - 4 && h >= (m.height as i32) - 4);
    if is_fullscreen {
        Some(rect)
    } else {
        // Also ensure at least one visible window check links cleanly
        let _ = enumerate_windows().len();
        None
    }
}

fn crop_frame_to_rect(
    src: &VideoFrame,
    mon_left: i32,
    mon_top: i32,
    rect: RECT,
    seq: &mut u64,
) -> VideoFrame {
    let rx = (rect.left - mon_left).clamp(0, src.width as i32 - 1) as u32;
    let ry = (rect.top - mon_top).clamp(0, src.height as i32 - 1) as u32;
    let rw = ((rect.right - rect.left).max(1) as u32).min(src.width - rx);
    let rh = ((rect.bottom - rect.top).max(1) as u32).min(src.height - ry);

    if rx == 0 && ry == 0 && rw == src.width && rh == src.height {
        return src.clone();
    }

    let mut out = vec![0u8; (rw as usize) * (rh as usize) * 4];
    for row in 0..rh {
        let src_off = (((ry + row) as usize) * (src.width as usize) + (rx as usize)) * 4;
        let dst_off = (row as usize) * (rw as usize) * 4;
        let row_bytes = (rw as usize) * 4;
        out[dst_off..dst_off + row_bytes].copy_from_slice(&src.data[src_off..src_off + row_bytes]);
    }
    *seq += 1;
    VideoFrame::new_rgba(rw, rh, out, src.pts_us, *seq)
}

#[allow(dead_code)]
fn _hwnd_dummy(_h: HWND) {}
