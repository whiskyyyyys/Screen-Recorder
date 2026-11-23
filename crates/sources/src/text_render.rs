use fontdue::{Font, FontSettings};
use obs_video::VideoFrame;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

static FONT_CACHE: OnceLock<parking_lot::Mutex<HashMap<String, Font>>> = OnceLock::new();

pub fn list_system_fonts() -> Vec<String> {
    let mut names = vec![
        "Segoe UI".to_string(),
        "Arial".to_string(),
        "Consolas".to_string(),
        "Impact".to_string(),
        "Calibri".to_string(),
        "Tahoma".to_string(),
        "Times New Roman".to_string(),
        "Verdana".to_string(),
    ];
    let fonts_dir = PathBuf::from(r"C:\Windows\Fonts");
    if let Ok(rd) = fs::read_dir(&fonts_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if ext.eq_ignore_ascii_case("ttf") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        if !names.iter().any(|n| n.eq_ignore_ascii_case(stem)) && names.len() < 40 {
                            names.push(stem.to_string());
                        }
                    }
                }
            }
        }
    }
    names
}

fn load_font(family: &str) -> Option<Font> {
    let cache = FONT_CACHE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    if let Some(f) = cache.lock().get(family) {
        return Some(f.clone());
    }

    let fonts_dir = PathBuf::from(r"C:\Windows\Fonts");
    let lower = family.to_ascii_lowercase();
    let candidates: Vec<&str> = match lower.as_str() {
        "segoe ui" => vec!["segoeui.ttf", "segoeuib.ttf", "arial.ttf"],
        "arial" => vec!["arial.ttf", "arialbd.ttf", "segoeui.ttf"],
        "consolas" => vec!["consola.ttf", "consolab.ttf", "cour.ttf"],
        "impact" => vec!["impact.ttf", "arialbd.ttf"],
        "calibri" => vec!["calibri.ttf", "calibrib.ttf"],
        "tahoma" => vec!["tahoma.ttf", "tahomabd.ttf"],
        "times new roman" => vec!["times.ttf", "timesbd.ttf"],
        "verdana" => vec!["verdana.ttf", "verdanab.ttf"],
        _ => vec!["segoeui.ttf", "arial.ttf", "tahoma.ttf", "calibri.ttf"],
    };

    // Direct match by stem or file name first
    let direct_path = fonts_dir.join(format!("{lower}.ttf"));
    if direct_path.exists() {
        if let Ok(bytes) = fs::read(&direct_path) {
            if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                cache.lock().insert(family.to_string(), font.clone());
                return Some(font);
            }
        }
    }

    for cand in candidates {
        let p = fonts_dir.join(cand);
        if p.exists() {
            if let Ok(bytes) = fs::read(&p) {
                if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                    cache.lock().insert(family.to_string(), font.clone());
                    return Some(font);
                }
            }
        }
    }

    // Fallback: scan first readable .ttf in C:\Windows\Fonts
    if let Ok(rd) = fs::read_dir(&fonts_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("ttf") {
                if let Ok(bytes) = fs::read(&p) {
                    if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                        cache.lock().insert(family.to_string(), font.clone());
                        return Some(font);
                    }
                }
            }
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_frame(
    text: &str,
    font_family: &str,
    font_size_px: f32,
    color_rgba: [u8; 4],
    gradient_enabled: bool,
    gradient_bottom_rgba: [u8; 4],
    outline_enabled: bool,
    outline_size: f32,
    outline_color_rgba: [u8; 4],
    drop_shadow: bool,
    align: &str,
    seq: u64,
) -> VideoFrame {
    let px = font_size_px.clamp(8.0, 256.0);
    let Some(font) = load_font(font_family) else {
        // Fallback solid bar if zero fonts exist on host
        return VideoFrame::new_solid_rgba(256, 64, color_rgba, seq);
    };

    let display_text = if text.is_empty() { " " } else { text };
    let lines: Vec<&str> = display_text.lines().collect();
    let line_height = (px * 1.25).ceil() as i32;
    let pad = (outline_size.ceil() as i32 + if drop_shadow { 6 } else { 2 }).max(6);

    let mut line_glyphs = Vec::with_capacity(lines.len());
    let mut max_line_w = 16i32;

    for line in &lines {
        let mut cursor_x = 0.0f32;
        let mut glyphs = Vec::new();
        for ch in line.chars() {
            let (metrics, bitmap) = font.rasterize(ch, px);
            glyphs.push((cursor_x, metrics, bitmap));
            cursor_x += metrics.advance_width;
        }
        let lw = cursor_x.ceil() as i32;
        max_line_w = max_line_w.max(lw);
        line_glyphs.push((lw, glyphs));
    }

    let width = (max_line_w + pad * 2).max(16) as u32;
    let height = ((lines.len() as i32) * line_height + pad * 2).max(16) as u32;
    let mut alpha_mask = vec![0u8; (width as usize) * (height as usize)];

    let baseline_offset = (px * 0.85).ceil() as i32;

    for (line_idx, (lw, glyphs)) in line_glyphs.iter().enumerate() {
        let align_x = match align {
            "center" => (max_line_w - *lw) / 2,
            "right" => max_line_w - *lw,
            _ => 0,
        };
        let baseline_y = pad + (line_idx as i32) * line_height + baseline_offset;

        for (adv_x, metrics, bitmap) in glyphs {
            let gx = pad + align_x + (*adv_x).round() as i32 + metrics.xmin;
            let gy = baseline_y - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                let py = gy + row as i32;
                if py < 0 || py >= height as i32 {
                    continue;
                }
                for col in 0..metrics.width {
                    let px_x = gx + col as i32;
                    if px_x < 0 || px_x >= width as i32 {
                        continue;
                    }
                    let cov = bitmap[row * metrics.width + col];
                    let idx = (py as usize) * (width as usize) + (px_x as usize);
                    alpha_mask[idx] = alpha_mask[idx].max(cov);
                }
            }
        }
    }

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];

    // 1. Optional drop shadow
    if drop_shadow {
        let sx_off = 3i32;
        let sy_off = 3i32;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let src_x = x - sx_off;
                let src_y = y - sy_off;
                if src_x >= 0 && src_y >= 0 && src_x < width as i32 && src_y < height as i32 {
                    let sa = alpha_mask[(src_y as usize) * (width as usize) + (src_x as usize)];
                    if sa > 0 {
                        let a = (sa as f32 / 255.0) * 0.65;
                        let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
                        rgba[idx] = 0;
                        rgba[idx + 1] = 0;
                        rgba[idx + 2] = 0;
                        rgba[idx + 3] = (a * 255.0) as u8;
                    }
                }
            }
        }
    }

    // 2. Optional outline stroke
    if outline_enabled && outline_size > 0.2 {
        let rad = outline_size.round().clamp(1.0, 8.0) as i32;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let mut max_n = 0u8;
                for dy in -rad..=rad {
                    let ny = y + dy;
                    if ny < 0 || ny >= height as i32 {
                        continue;
                    }
                    for dx in -rad..=rad {
                        if dx * dx + dy * dy > rad * rad {
                            continue;
                        }
                        let nx = x + dx;
                        if nx < 0 || nx >= width as i32 {
                            continue;
                        }
                        max_n =
                            max_n.max(alpha_mask[(ny as usize) * (width as usize) + (nx as usize)]);
                    }
                }
                if max_n > 0 {
                    let sa = (max_n as f32 / 255.0) * (outline_color_rgba[3] as f32 / 255.0);
                    let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
                    blend_over(&mut rgba[idx..idx + 4], outline_color_rgba, sa);
                }
            }
        }
    }

    // 3. Foreground glyph fill with optional vertical gradient
    for y in 0..height {
        let t = (y as f32) / (height.max(1) as f32);
        let fill = if gradient_enabled {
            [
                lerp_u8(color_rgba[0], gradient_bottom_rgba[0], t),
                lerp_u8(color_rgba[1], gradient_bottom_rgba[1], t),
                lerp_u8(color_rgba[2], gradient_bottom_rgba[2], t),
                lerp_u8(color_rgba[3], gradient_bottom_rgba[3], t),
            ]
        } else {
            color_rgba
        };
        for x in 0..width {
            let cov = alpha_mask[(y as usize) * (width as usize) + (x as usize)];
            if cov > 0 {
                let sa = (cov as f32 / 255.0) * (fill[3] as f32 / 255.0);
                let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
                blend_over(&mut rgba[idx..idx + 4], fill, sa);
            }
        }
    }

    VideoFrame::new_rgba(width, height, rgba, 0, seq)
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t).clamp(0.0, 255.0) as u8
}

#[inline]
fn blend_over(dst: &mut [u8], src: [u8; 4], src_alpha: f32) {
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_alpha + dst_a * (1.0 - src_alpha);
    if out_a <= 1e-5 {
        dst.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for c in 0..3 {
        let sc = src[c] as f32 / 255.0;
        let dc = dst[c] as f32 / 255.0;
        let oc = (sc * src_alpha + dc * dst_a * (1.0 - src_alpha)) / out_a;
        dst[c] = (oc * 255.0).clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).clamp(0.0, 255.0) as u8;
}
