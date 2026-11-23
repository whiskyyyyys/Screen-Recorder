use crate::display_capture::DisplayCapturer;
use crate::monitors::{enumerate_monitors, monitor_index_for_rect};
use crate::window_capture::{is_frame_all_black, window_content_rect, WindowCapturer};
use obs_core::Result;
use obs_video::VideoFrame;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Non-invasive high-FPS Game Capture using monitor-matched DXGI Output Duplication for
/// fullscreen/foreground games with cached-frame retention on DXGI timeout and automatic
/// `PrintWindow` / `BitBlt` fallback when occluded. Avoids anti-cheat DLL injection hooks.
pub struct GameCapturer {
    mode: String,
    capture_method: String,
    capture_cursor: bool,
    force_sdr: bool,
    force_scaling: bool,
    scale_width: u32,
    scale_height: u32,
    window_capturer: WindowCapturer,
    dxgi: Option<(usize, DisplayCapturer)>,
    last_valid_frame: Option<VideoFrame>,
    seq: u64,
}

impl GameCapturer {
    pub fn new(
        mode: String,
        window_title: String,
        process_name: String,
        capture_cursor: bool,
    ) -> Self {
        Self::new_with_options(
            mode,
            window_title,
            process_name,
            capture_cursor,
            "Auto".to_string(),
            false,
            1920,
            1080,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_options(
        mode: String,
        window_title: String,
        process_name: String,
        capture_cursor: bool,
        capture_method: String,
        force_scaling: bool,
        scale_width: u32,
        scale_height: u32,
        force_sdr: bool,
    ) -> Self {
        let window_capturer = WindowCapturer::new(window_title, process_name, capture_cursor, true);
        Self {
            mode,
            capture_method,
            capture_cursor,
            force_sdr,
            force_scaling,
            scale_width: scale_width.clamp(160, 3840),
            scale_height: scale_height.clamp(120, 2160),
            window_capturer,
            dxgi: None,
            last_valid_frame: None,
            seq: 0,
        }
    }

    fn ensure_dxgi(&mut self, mon_idx: usize) -> Option<&mut DisplayCapturer> {
        let matches = self.dxgi.as_ref().is_some_and(|(idx, _)| *idx == mon_idx);
        if !matches {
            let method = if self.capture_method == "DXGI" {
                "DXGI"
            } else {
                "Auto"
            };
            self.dxgi = DisplayCapturer::new_with_options(
                mon_idx,
                self.capture_cursor,
                method,
                self.force_sdr,
            )
            .ok()
            .map(|cap| (mon_idx, cap));
        }
        self.dxgi.as_mut().map(|(_, cap)| cap)
    }

    fn capture_dxgi_rect(&mut self, mon_idx: usize, rect: RECT, pts_us: u64) -> Option<VideoFrame> {
        let dxgi = self.ensure_dxgi(mon_idx)?;
        let mon_left = dxgi.monitor().left;
        let mon_top = dxgi.monitor().top;
        let mon_frame = match dxgi.capture_frame(8, pts_us) {
            Ok(Some(frame)) => frame,
            Ok(None) => dxgi.last_frame()?,
            Err(_) => return dxgi.last_frame(),
        };
        let cropped = crop_frame_to_rect(&mon_frame, mon_left, mon_top, rect, &mut self.seq);
        if is_frame_all_black(&cropped.data) {
            None
        } else {
            Some(cropped)
        }
    }

    fn maybe_scale(&mut self, frame: VideoFrame) -> VideoFrame {
        if !self.force_scaling
            || (frame.width == self.scale_width && frame.height == self.scale_height)
            || frame.width == 0
            || frame.height == 0
        {
            return frame;
        }
        scale_rgba_bilinear(&frame, self.scale_width, self.scale_height, &mut self.seq)
    }

    pub fn capture_frame(&mut self, pts_us: u64) -> Result<Option<VideoFrame>> {
        let self_pid = std::process::id();

        if self.mode == "AnyFullscreen" {
            if let Some((full_rect, mon_idx)) = detect_fullscreen_foreground(self_pid) {
                if let Some(frame) = self.capture_dxgi_rect(mon_idx, full_rect, pts_us) {
                    self.last_valid_frame = Some(frame.clone());
                    return Ok(Some(self.maybe_scale(frame)));
                }
            }
            // Do not capture OxideStudio itself when user alt-tabs back to OBS; retain last valid game frame
            return Ok(self.last_valid_frame.clone().map(|f| self.maybe_scale(f)));
        }

        // SpecificWindow mode: use monitor-matched DXGI when target is foreground (or forced DXGI),
        // and fall back cleanly to PrintWindow / BitBlt when occluded behind other windows.
        if let Some(hwnd) = self.window_capturer.resolve_hwnd() {
            let fg = unsafe { GetForegroundWindow() };
            let try_dxgi = self.capture_method == "DXGI"
                || (self.capture_method != "WindowCapture" && fg == hwnd);
            if try_dxgi {
                if let Some(rect) = window_content_rect(hwnd, true) {
                    let mon_idx =
                        monitor_index_for_rect(rect.left, rect.top, rect.right, rect.bottom);
                    if let Some(frame) = self.capture_dxgi_rect(mon_idx, rect, pts_us) {
                        self.last_valid_frame = Some(frame.clone());
                        return Ok(Some(self.maybe_scale(frame)));
                    }
                }
            }
        }

        if let Some(frame) = self.window_capturer.capture_frame(pts_us)? {
            if !is_frame_all_black(&frame.data) || self.last_valid_frame.is_none() {
                self.last_valid_frame = Some(frame.clone());
                return Ok(Some(self.maybe_scale(frame)));
            }
        }

        Ok(self.last_valid_frame.clone().map(|f| self.maybe_scale(f)))
    }
}

fn detect_fullscreen_foreground(exclude_pid: u32) -> Option<(RECT, usize)> {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(fg, Some(&mut pid));
    }
    if pid == 0 || pid == exclude_pid {
        return None;
    }
    let rect = window_content_rect(fg, false)?;
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    let monitors = enumerate_monitors();
    let mon_idx = monitor_index_for_rect(rect.left, rect.top, rect.right, rect.bottom);
    let mon = monitors.get(mon_idx).or_else(|| monitors.first())?;
    let is_fullscreen = w >= (mon.width as i32) - 4 && h >= (mon.height as i32) - 4;
    if is_fullscreen {
        Some((rect, mon.index))
    } else {
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
    let rx = (rect.left - mon_left).clamp(0, src.width.saturating_sub(1) as i32) as u32;
    let ry = (rect.top - mon_top).clamp(0, src.height.saturating_sub(1) as i32) as u32;
    let rw = ((rect.right - rect.left).max(1) as u32).min(src.width.saturating_sub(rx).max(1));
    let rh = ((rect.bottom - rect.top).max(1) as u32).min(src.height.saturating_sub(ry).max(1));

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

fn scale_rgba_bilinear(src: &VideoFrame, dst_w: u32, dst_h: u32, seq: &mut u64) -> VideoFrame {
    let sw = src.width.max(1) as usize;
    let sh = src.height.max(1) as usize;
    let dw = dst_w.max(2) as usize;
    let dh = dst_h.max(2) as usize;
    let mut out = vec![0u8; dw * dh * 4];

    for y in 0..dh {
        let gy = ((y as f32 + 0.5) * (sh as f32) / (dh as f32) - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = gy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let fy = gy - (y0 as f32);

        for x in 0..dw {
            let gx =
                ((x as f32 + 0.5) * (sw as f32) / (dw as f32) - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = gx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let fx = gx - (x0 as f32);

            let i00 = (y0 * sw + x0) * 4;
            let i10 = (y0 * sw + x1) * 4;
            let i01 = (y1 * sw + x0) * 4;
            let i11 = (y1 * sw + x1) * 4;
            let dst_idx = (y * dw + x) * 4;

            for c in 0..3 {
                let top = src.data[i00 + c] as f32 * (1.0 - fx) + src.data[i10 + c] as f32 * fx;
                let bot = src.data[i01 + c] as f32 * (1.0 - fx) + src.data[i11 + c] as f32 * fx;
                out[dst_idx + c] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
            }
            out[dst_idx + 3] = 255;
        }
    }

    *seq += 1;
    VideoFrame::new_rgba(dst_w, dst_h, out, src.pts_us, *seq)
}
