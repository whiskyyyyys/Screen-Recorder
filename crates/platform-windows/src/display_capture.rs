use crate::monitors::{enumerate_monitors, MonitorInfo};
use obs_core::{ObsError, Result};
use obs_video::{bgra_to_rgba_vec, VideoFrame};
use std::mem::size_of;
use std::ptr;
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC,
    SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, ICONINFO,
};

struct DxgiState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    dupl: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    staging_w: u32,
    staging_h: u32,
    staging_format: DXGI_FORMAT,
}

pub struct DisplayCapturer {
    monitor: MonitorInfo,
    capture_cursor: bool,
    capture_method: String,
    force_gdi: bool,
    force_sdr: bool,
    dxgi: Option<DxgiState>,
    dxgi_unsupported_hdr: bool,
    seq: u64,
    last_rgba: Option<VideoFrame>,
}

impl DisplayCapturer {
    pub fn new(monitor_index: usize, capture_cursor: bool, force_gdi: bool) -> Result<Self> {
        Self::new_with_options(
            monitor_index,
            capture_cursor,
            if force_gdi { "GDI" } else { "Auto" },
            true,
        )
    }

    pub fn new_with_options(
        monitor_index: usize,
        capture_cursor: bool,
        capture_method: &str,
        force_sdr: bool,
    ) -> Result<Self> {
        let monitors = enumerate_monitors();
        let monitor = monitors
            .get(monitor_index)
            .cloned()
            .or_else(|| monitors.into_iter().next())
            .ok_or_else(|| ObsError::Capture("No monitors found".to_string()))?;

        let force_gdi = capture_method.eq_ignore_ascii_case("GDI");
        let dxgi = if !force_gdi {
            Self::init_dxgi(&monitor).ok()
        } else {
            None
        };

        Ok(Self {
            monitor,
            capture_cursor,
            capture_method: capture_method.to_string(),
            force_gdi,
            force_sdr,
            dxgi,
            dxgi_unsupported_hdr: false,
            seq: 0,
            last_rgba: None,
        })
    }

    pub fn monitor(&self) -> &MonitorInfo {
        &self.monitor
    }

    pub fn last_frame(&self) -> Option<VideoFrame> {
        self.last_rgba.clone()
    }

    pub fn active_backend_name(&self) -> &'static str {
        if self.force_gdi || self.dxgi.is_none() || self.dxgi_unsupported_hdr {
            "GDI BitBlt"
        } else {
            "DXGI Desktop Duplication"
        }
    }

    fn init_dxgi(monitor: &MonitorInfo) -> Result<DxgiState> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| ObsError::Capture(format!("CreateDXGIFactory1 failed: {e}")))?;

            let mut adapter_idx = 0u32;
            while let Ok(adapter1) = factory.EnumAdapters1(adapter_idx) {
                adapter_idx += 1;
                let mut out_idx = 0u32;
                while let Ok(output) = adapter1.EnumOutputs(out_idx) {
                    out_idx += 1;
                    if let Ok(desc) = output.GetDesc() {
                        let len = desc
                            .DeviceName
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(desc.DeviceName.len());
                        let out_name = String::from_utf16_lossy(&desc.DeviceName[..len]);
                        if out_name == monitor.device_name {
                            let adapter: IDXGIAdapter = adapter1.cast().map_err(|e| {
                                ObsError::Capture(format!("Adapter cast failed: {e}"))
                            })?;
                            let mut device = None;
                            let mut context = None;
                            D3D11CreateDevice(
                                &adapter,
                                D3D_DRIVER_TYPE_UNKNOWN,
                                HMODULE::default(),
                                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                                None,
                                D3D11_SDK_VERSION,
                                Some(&mut device),
                                None,
                                Some(&mut context),
                            )
                            .map_err(|e| {
                                ObsError::Capture(format!("D3D11CreateDevice failed: {e}"))
                            })?;
                            let device = device.ok_or_else(|| {
                                ObsError::Capture("D3D11CreateDevice returned null device".into())
                            })?;
                            let context = context.ok_or_else(|| {
                                ObsError::Capture("D3D11CreateDevice returned null context".into())
                            })?;
                            let output1: IDXGIOutput1 = output.cast().map_err(|e| {
                                ObsError::Capture(format!("IDXGIOutput1 cast failed: {e}"))
                            })?;
                            let dupl = output1.DuplicateOutput(&device).map_err(|e| {
                                ObsError::Capture(format!("DuplicateOutput failed: {e}"))
                            })?;

                            return Ok(DxgiState {
                                device,
                                context,
                                output1,
                                dupl,
                                staging: None,
                                staging_w: 0,
                                staging_h: 0,
                                staging_format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            });
                        }
                    }
                }
            }
            Err(ObsError::Capture(format!(
                "Monitor {} not matched in DXGI outputs",
                monitor.device_name
            )))
        }
    }

    /// Captures a frame as RGBA8. Returns `Ok(Some(frame))` on new or cached frame, falling back to GDI if needed.
    pub fn capture_frame(&mut self, timeout_ms: u32, pts_us: u64) -> Result<Option<VideoFrame>> {
        if !self.force_gdi && !self.dxgi_unsupported_hdr && self.dxgi.is_none() {
            self.dxgi = Self::init_dxgi(&self.monitor).ok();
        }

        if !self.force_gdi && !self.dxgi_unsupported_hdr {
            if let Some(dxgi) = self.dxgi.as_mut() {
                match Self::capture_dxgi(
                    dxgi,
                    &self.monitor,
                    self.capture_cursor,
                    self.force_sdr,
                    timeout_ms,
                    pts_us,
                    &mut self.seq,
                ) {
                    Ok(Some(frame)) => {
                        self.last_rgba = Some(frame.clone());
                        return Ok(Some(frame));
                    }
                    Ok(None) => {
                        if self.last_rgba.is_some() {
                            return Ok(None);
                        }
                    }
                    Err(AccessStatus::AccessLost) => {
                        if let Ok(new_dupl) = unsafe { dxgi.output1.DuplicateOutput(&dxgi.device) }
                        {
                            dxgi.dupl = new_dupl;
                        } else {
                            self.dxgi = None;
                        }
                    }
                    Err(AccessStatus::UnsupportedFormat) => {
                        self.dxgi_unsupported_hdr = true;
                        self.dxgi = None;
                    }
                    Err(AccessStatus::Other(_)) => {
                        self.dxgi = None;
                    }
                }
            } else if self.capture_method.eq_ignore_ascii_case("DXGI") && self.last_rgba.is_some() {
                return Ok(None);
            }
        }

        let frame = Self::capture_gdi(&self.monitor, self.capture_cursor, pts_us, &mut self.seq)?;
        self.last_rgba = Some(frame.clone());
        Ok(Some(frame))
    }

    fn capture_dxgi(
        dxgi: &mut DxgiState,
        monitor: &MonitorInfo,
        capture_cursor: bool,
        force_sdr: bool,
        timeout_ms: u32,
        pts_us: u64,
        seq: &mut u64,
    ) -> std::result::Result<Option<VideoFrame>, AccessStatus> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            let hr = dxgi
                .dupl
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource);
            if let Err(e) = hr {
                let code = e.code();
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(None);
                }
                if code == DXGI_ERROR_ACCESS_LOST {
                    return Err(AccessStatus::AccessLost);
                }
                return Err(AccessStatus::Other(format!("AcquireNextFrame: {e}")));
            }

            struct ReleaseGuard<'a>(&'a IDXGIOutputDuplication);
            impl<'a> Drop for ReleaseGuard<'a> {
                fn drop(&mut self) {
                    unsafe {
                        let _ = self.0.ReleaseFrame();
                    }
                }
            }
            let _guard = ReleaseGuard(&dxgi.dupl);

            let res = resource.ok_or_else(|| AccessStatus::Other("Null DXGI resource".into()))?;
            let tex: ID3D11Texture2D = res
                .cast()
                .map_err(|e| AccessStatus::Other(format!("Texture2D cast: {e}")))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);

            let is_bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
                || desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
            let is_rgba = desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM
                || desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
            if !is_bgra && !is_rgba {
                let _ = force_sdr;
                return Err(AccessStatus::UnsupportedFormat);
            }

            if dxgi.staging.is_none()
                || dxgi.staging_w != desc.Width
                || dxgi.staging_h != desc.Height
                || dxgi.staging_format != desc.Format
            {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut st = None;
                dxgi.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut st))
                    .map_err(|e| AccessStatus::Other(format!("CreateTexture2D staging: {e}")))?;
                dxgi.staging = st;
                dxgi.staging_w = desc.Width;
                dxgi.staging_h = desc.Height;
                dxgi.staging_format = desc.Format;
            }

            let staging = dxgi
                .staging
                .as_ref()
                .ok_or_else(|| AccessStatus::Other("Missing staging texture".into()))?;
            dxgi.context.CopyResource(staging, &tex);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            dxgi.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| AccessStatus::Other(format!("Map staging: {e}")))?;

            let w = desc.Width;
            let h = desc.Height;
            let total_bytes = (mapped.RowPitch as usize) * (h as usize);
            let slice = std::slice::from_raw_parts(mapped.pData as *const u8, total_bytes);
            let mut rgba = if is_bgra {
                bgra_to_rgba_vec(slice, w, h, mapped.RowPitch)
            } else {
                let row_bytes = (w as usize) * 4;
                let mut out = vec![0u8; row_bytes * (h as usize)];
                for row in 0..h as usize {
                    let s = row * (mapped.RowPitch as usize);
                    let d = row * row_bytes;
                    out[d..d + row_bytes].copy_from_slice(&slice[s..s + row_bytes]);
                }
                out
            };
            dxgi.context.Unmap(staging, 0);

            for px in rgba.as_chunks_mut::<4>().0 {
                px[3] = 255;
            }

            if capture_cursor {
                overlay_cursor_on_rgba(&mut rgba, w, h, monitor.left, monitor.top);
            }

            *seq += 1;
            Ok(Some(VideoFrame::new_rgba(w, h, rgba, pts_us, *seq)))
        }
    }

    pub fn capture_gdi(
        monitor: &MonitorInfo,
        capture_cursor: bool,
        pts_us: u64,
        seq: &mut u64,
    ) -> Result<VideoFrame> {
        unsafe {
            let screen_dc = GetDC(HWND::default());
            if screen_dc.is_invalid() {
                return Err(ObsError::Capture("GetDC(NULL) failed".to_string()));
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_invalid() {
                let _ = ReleaseDC(HWND::default(), screen_dc);
                return Err(ObsError::Capture("CreateCompatibleDC failed".to_string()));
            }

            let w = monitor.width as i32;
            let h = monitor.height as i32;
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
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
                let _ = ReleaseDC(HWND::default(), screen_dc);
                ObsError::Capture(format!("CreateDIBSection failed: {e}"))
            })?;

            let old_obj = SelectObject(mem_dc, dib);
            let blt_res = BitBlt(
                mem_dc,
                0,
                0,
                w,
                h,
                screen_dc,
                monitor.left,
                monitor.top,
                SRCCOPY | CAPTUREBLT,
            );

            if capture_cursor {
                draw_cursor_on_dc(mem_dc, monitor.left, monitor.top);
            }

            let rgba = if blt_res.is_ok() && !bits_ptr.is_null() {
                let len = (w as usize) * (h as usize) * 4;
                let bgra = std::slice::from_raw_parts(bits_ptr as *const u8, len);
                let mut conv = bgra_to_rgba_vec(bgra, w as u32, h as u32, (w * 4) as u32);
                // Ensure opaque alpha on GDI desktop captures
                for px in conv.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                conv
            } else {
                vec![0u8; (w as usize) * (h as usize) * 4]
            };

            SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND::default(), screen_dc);

            *seq += 1;
            Ok(VideoFrame::new_rgba(w as u32, h as u32, rgba, pts_us, *seq))
        }
    }
}

enum AccessStatus {
    AccessLost,
    UnsupportedFormat,
    Other(#[allow(dead_code)] String),
}

pub(crate) unsafe fn draw_cursor_on_dc(dc: HDC, origin_x: i32, origin_y: i32) {
    let mut ci = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if GetCursorInfo(&mut ci).is_ok() && (ci.flags.0 & CURSOR_SHOWING.0) != 0 {
        let mut ii = ICONINFO::default();
        if GetIconInfo(ci.hCursor, &mut ii).is_ok() {
            let x = ci.ptScreenPos.x - origin_x - (ii.xHotspot as i32);
            let y = ci.ptScreenPos.y - origin_y - (ii.yHotspot as i32);
            let _ = DrawIconEx(dc, x, y, ci.hCursor, 0, 0, 0, None, DI_NORMAL);
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(ii.hbmColor);
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(ii.hbmMask);
            }
        }
    }
}

/// Draws a clean software cursor indicator directly onto RGBA buffer when using DXGI duplication.
unsafe fn overlay_cursor_on_rgba(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
) {
    let mut ci = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if GetCursorInfo(&mut ci).is_err() || (ci.flags.0 & CURSOR_SHOWING.0) == 0 {
        return;
    }
    let cx = ci.ptScreenPos.x - origin_x;
    let cy = ci.ptScreenPos.y - origin_y;
    if cx < 0 || cy < 0 || cx >= width as i32 || cy >= height as i32 {
        return;
    }

    // Render crisp cursor arrow via tiny 16x16 GDI DIB so custom Windows cursors work accurately
    let screen_dc = GetDC(HWND::default());
    if screen_dc.is_invalid() {
        return;
    }
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_invalid() {
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return;
    }

    let csize = 32i32;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: csize,
            biHeight: -csize,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [Default::default()],
    };
    let mut bits_ptr: *mut std::ffi::c_void = ptr::null_mut();
    if let Ok(dib) = CreateDIBSection(
        mem_dc,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits_ptr,
        HANDLE::default(),
        0,
    ) {
        let old = SelectObject(mem_dc, dib);
        let len = (csize as usize) * (csize as usize) * 4;
        // Initialize background to the captured desktop pixels so XOR/alpha cursors composite accurately!
        let mut ii = ICONINFO::default();
        let (hx, hy) = if GetIconInfo(ci.hCursor, &mut ii).is_ok() {
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(ii.hbmColor);
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(ii.hbmMask);
            }
            (ii.xHotspot as i32, ii.yHotspot as i32)
        } else {
            (0, 0)
        };
        let sx = cx - hx;
        let sy = cy - hy;
        if !bits_ptr.is_null() {
            let buf = std::slice::from_raw_parts_mut(bits_ptr as *mut u8, len);
            for py in 0..csize {
                let dy = sy + py;
                if dy < 0 || dy >= height as i32 {
                    continue;
                }
                for px in 0..csize {
                    let dx = sx + px;
                    if dx < 0 || dx >= width as i32 {
                        continue;
                    }
                    let src_idx = ((dy as usize) * (width as usize) + (dx as usize)) * 4;
                    let dst_idx = ((py as usize) * (csize as usize) + (px as usize)) * 4;
                    buf[dst_idx] = rgba[src_idx + 2];
                    buf[dst_idx + 1] = rgba[src_idx + 1];
                    buf[dst_idx + 2] = rgba[src_idx];
                    buf[dst_idx + 3] = 255;
                }
            }
            let _ = DrawIconEx(mem_dc, 0, 0, ci.hCursor, csize, csize, 0, None, DI_NORMAL);
            for py in 0..csize {
                let dy = sy + py;
                if dy < 0 || dy >= height as i32 {
                    continue;
                }
                for px in 0..csize {
                    let dx = sx + px;
                    if dx < 0 || dx >= width as i32 {
                        continue;
                    }
                    let src_idx = ((py as usize) * (csize as usize) + (px as usize)) * 4;
                    let dst_idx = ((dy as usize) * (width as usize) + (dx as usize)) * 4;
                    rgba[dst_idx] = buf[src_idx + 2];
                    rgba[dst_idx + 1] = buf[src_idx + 1];
                    rgba[dst_idx + 2] = buf[src_idx];
                    rgba[dst_idx + 3] = 255;
                }
            }
        }
        SelectObject(mem_dc, old);
        let _ = DeleteObject(dib);
    }
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(HWND::default(), screen_dc);
}

#[allow(dead_code)]
fn _rect_dummy(_r: RECT) {}
