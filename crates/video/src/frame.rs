use crate::format::{ColorRange, ColorSpace, VideoFormat};
use std::sync::Arc;

/// Zero-copy shared video frame carried between capture, sources, compositor, and encoders.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: VideoFormat,
    pub pts_us: u64,
    pub seq: u64,
}

impl VideoFrame {
    pub fn new_rgba(width: u32, height: u32, data: Vec<u8>, pts_us: u64, seq: u64) -> Self {
        Self {
            data: Arc::new(data),
            width,
            height,
            stride: width * 4,
            format: VideoFormat::Rgba8,
            pts_us,
            seq,
        }
    }

    pub fn new_solid_rgba(width: u32, height: u32, rgba: [u8; 4], seq: u64) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        let mut buf = Vec::with_capacity(pixel_count * 4);
        for _ in 0..pixel_count {
            buf.extend_from_slice(&rgba);
        }
        Self::new_rgba(width, height, buf, 0, seq)
    }
}

/// Converts tight or padded BGRA8 scanlines to tightly packed RGBA8.
pub fn bgra_to_rgba_vec(src: &[u8], width: u32, height: u32, src_stride: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let stride = src_stride as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row_src = &src[y * stride..y * stride + w * 4];
        let row_dst = &mut out[y * w * 4..(y + 1) * w * 4];
        for (s, d) in row_src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(row_dst.as_chunks_mut::<4>().0.iter_mut())
        {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    out
}

/// CPU fallback conversion from tightly-packed RGBA8 to NV12 (4:2:0 semi-planar).
/// Matches GPU shader equations for Rec.709 / Rec.601 and Partial / Full ranges.
pub fn rgba_to_nv12(
    rgba: &[u8],
    width: u32,
    height: u32,
    color_space: ColorSpace,
    color_range: ColorRange,
) -> Vec<u8> {
    let w = (width as usize) & !1;
    let h = (height as usize) & !1;
    let y_size = w * h;
    let uv_size = y_size / 2;
    let mut nv12 = vec![0u8; y_size + uv_size];

    let (kr, kb) = match color_space {
        ColorSpace::Rec601 => (0.299f32, 0.114f32),
        ColorSpace::Rec709 => (0.2126f32, 0.0722f32),
        ColorSpace::Rec2100Pq | ColorSpace::Rec2100Hlg => (0.2627f32, 0.0593f32),
    };
    let kg = 1.0 - kr - kb;

    let (y_off, y_scale, uv_scale) = match color_range {
        ColorRange::Partial => (16.0f32, 219.0f32, 224.0f32),
        ColorRange::Full => (0.0f32, 255.0f32, 255.0f32),
    };

    let stride = (width as usize) * 4;
    let (y_plane, uv_plane) = nv12.split_at_mut(y_size);

    for y in 0..h {
        let row = &rgba[y * stride..y * stride + w * 4];
        for x in 0..w {
            let px = &row[x * 4..x * 4 + 4];
            let r = px[0] as f32 / 255.0;
            let g = px[1] as f32 / 255.0;
            let b = px[2] as f32 / 255.0;
            let luma = kr * r + kg * g + kb * b;
            y_plane[y * w + x] = (y_off + luma * y_scale).round().clamp(0.0, 255.0) as u8;
        }
    }

    for y_pair in 0..(h / 2) {
        let r0 = &rgba[(y_pair * 2) * stride..];
        let r1 = &rgba[(y_pair * 2 + 1) * stride..];
        for x_pair in 0..(w / 2) {
            let x = x_pair * 2;
            let p00 = &r0[x * 4..x * 4 + 4];
            let p01 = &r0[(x + 1) * 4..(x + 1) * 4 + 4];
            let p10 = &r1[x * 4..x * 4 + 4];
            let p11 = &r1[(x + 1) * 4..(x + 1) * 4 + 4];

            let r = (p00[0] as f32 + p01[0] as f32 + p10[0] as f32 + p11[0] as f32) / 1020.0;
            let g = (p00[1] as f32 + p01[1] as f32 + p10[1] as f32 + p11[1] as f32) / 1020.0;
            let b = (p00[2] as f32 + p01[2] as f32 + p10[2] as f32 + p11[2] as f32) / 1020.0;

            let luma = kr * r + kg * g + kb * b;
            let cb = 0.5 * ((b - luma) / (1.0 - kb));
            let cr = 0.5 * ((r - luma) / (1.0 - kr));

            let u = (128.0 + cb * uv_scale).round().clamp(0.0, 255.0) as u8;
            let v = (128.0 + cr * uv_scale).round().clamp(0.0, 255.0) as u8;
            let idx = y_pair * w + x;
            uv_plane[idx] = u;
            uv_plane[idx + 1] = v;
        }
    }

    nv12
}
