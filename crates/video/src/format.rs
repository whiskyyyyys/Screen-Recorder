use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VideoFormat {
    #[default]
    Rgba8,
    Bgra8,
    Nv12,
    I420,
}

impl VideoFormat {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Rgba8 => "RGBA (8-bit)",
            Self::Bgra8 => "BGRA (8-bit)",
            Self::Nv12 => "NV12 (4:2:0)",
            Self::I420 => "I420 (4:2:0)",
        }
    }

    pub fn ffmpeg_pix_fmt(self) -> &'static str {
        match self {
            Self::Rgba8 => "rgba",
            Self::Bgra8 => "bgra",
            Self::Nv12 => "nv12",
            Self::I420 => "yuv420p",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ColorSpace {
    #[default]
    Rec709,
    Rec601,
    Rec2100Pq,
    Rec2100Hlg,
}

impl ColorSpace {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Rec709 => "Rec. 709",
            Self::Rec601 => "Rec. 601",
            Self::Rec2100Pq => "Rec. 2100 (PQ / HDR)",
            Self::Rec2100Hlg => "Rec. 2100 (HLG / HDR)",
        }
    }

    pub fn is_hdr(self) -> bool {
        matches!(self, Self::Rec2100Pq | Self::Rec2100Hlg)
    }

    pub fn ffmpeg_colorspace(self) -> &'static str {
        match self {
            Self::Rec709 => "bt709",
            Self::Rec601 => "smpte170m",
            Self::Rec2100Pq | Self::Rec2100Hlg => "bt2020nc",
        }
    }

    pub fn ffmpeg_color_trc(self) -> &'static str {
        match self {
            Self::Rec709 => "bt709",
            Self::Rec601 => "smpte170m",
            Self::Rec2100Pq => "smpte2084",
            Self::Rec2100Hlg => "arib-std-b67",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ColorRange {
    #[default]
    Partial,
    Full,
}

impl ColorRange {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Partial => "Limited (16-235)",
            Self::Full => "Full (0-255)",
        }
    }

    pub fn ffmpeg_color_range(self) -> &'static str {
        match self {
            Self::Partial => "tv",
            Self::Full => "pc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ScaleFilter {
    Point,
    Bilinear,
    #[default]
    Bicubic,
    Lanczos,
    Area,
}

impl ScaleFilter {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Point => "Point (Nearest)",
            Self::Bilinear => "Bilinear",
            Self::Bicubic => "Bicubic (Sharpened 16-tap)",
            Self::Lanczos => "Lanczos (Sharpened 36-tap)",
            Self::Area => "Area (Weighted sum)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub output_format: VideoFormat,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub scale_filter: ScaleFilter,
    pub sdr_white_level_nits: f32,
    pub hdr_nominal_peak_nits: f32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            canvas_width: 1920,
            canvas_height: 1080,
            output_width: 1920,
            output_height: 1080,
            fps_num: 60,
            fps_den: 1,
            output_format: VideoFormat::Nv12,
            color_space: ColorSpace::Rec709,
            color_range: ColorRange::Partial,
            scale_filter: ScaleFilter::Bicubic,
            sdr_white_level_nits: 300.0,
            hdr_nominal_peak_nits: 1000.0,
        }
    }
}

impl VideoConfig {
    pub fn fps_f64(&self) -> f64 {
        let den = self.fps_den.max(1) as f64;
        (self.fps_num.max(1) as f64) / den
    }

    pub fn frame_interval_us(&self) -> u64 {
        let den = self.fps_den.max(1) as u64;
        let num = self.fps_num.max(1) as u64;
        (1_000_000 * den) / num
    }

    /// Ensure even dimensions for YUV 4:2:0 encoders (NV12/I420).
    pub fn sanitized_output_size(&self) -> (u32, u32) {
        let w = (self.output_width.max(16) / 2) * 2;
        let h = (self.output_height.max(16) / 2) * 2;
        (w, h)
    }

    pub fn sanitized_canvas_size(&self) -> (u32, u32) {
        let w = (self.canvas_width.max(16) / 2) * 2;
        let h = (self.canvas_height.max(16) / 2) * 2;
        (w, h)
    }
}
