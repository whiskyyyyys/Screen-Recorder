use obs_core::FilterId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChromaKeyPreset {
    #[default]
    Green,
    Blue,
    Magenta,
    Custom,
}

impl ChromaKeyPreset {
    pub fn rgba(self, custom: [u8; 4]) -> [u8; 4] {
        match self {
            Self::Green => [0, 255, 0, 255],
            Self::Blue => [0, 0, 255, 255],
            Self::Magenta => [255, 0, 255, 255],
            Self::Custom => custom,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Magenta => "Magenta",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MaskBlendType {
    #[default]
    AlphaMaskColorChannel,
    AlphaMaskAlphaChannel,
    BlendMultiply,
    BlendAddition,
    BlendSubtraction,
}

impl MaskBlendType {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::AlphaMaskColorChannel => "Alpha Mask (Color Channel)",
            Self::AlphaMaskAlphaChannel => "Alpha Mask (Alpha Channel)",
            Self::BlendMultiply => "Blend (Multiply)",
            Self::BlendAddition => "Blend (Addition)",
            Self::BlendSubtraction => "Blend (Subtraction)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFilterInstance {
    pub id: FilterId,
    pub name: String,
    pub enabled: bool,
    pub kind: VideoFilterKind,
}

impl VideoFilterInstance {
    pub fn new(name: impl Into<String>, kind: VideoFilterKind) -> Self {
        Self {
            id: FilterId::new(),
            name: name.into(),
            enabled: true,
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VideoFilterKind {
    ChromaKey {
        preset: ChromaKeyPreset,
        custom_color_rgba: [u8; 4],
        /// 1..=1000 (OBS default 400 -> 0.400)
        similarity: f32,
        /// 1..=1000 (OBS default 80 -> 0.080)
        smoothness: f32,
        /// 1..=1000 (OBS default 100 -> 0.100)
        key_color_spill_reduction: f32,
        opacity: f32,
        contrast: f32,
        brightness: f32,
        gamma: f32,
    },
    ColorKey {
        key_color_rgba: [u8; 4],
        similarity: f32,
        smoothness: f32,
        opacity: f32,
        contrast: f32,
        brightness: f32,
        gamma: f32,
    },
    LumaKey {
        luma_max: f32,
        luma_max_smooth: f32,
        luma_min: f32,
        luma_min_smooth: f32,
    },
    ColorCorrection {
        gamma: f32,
        contrast: f32,
        brightness: f32,
        saturation: f32,
        hue_shift_deg: f32,
        opacity: f32,
        color_multiply_rgba: [u8; 4],
        color_add_rgba: [u8; 4],
    },
    CropPad {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        relative: bool,
    },
    ScalingAspect {
        target_width: u32,
        target_height: u32,
        keep_aspect: bool,
        scale_filter: String,
    },
    Scroll {
        speed_x: f32,
        speed_y: f32,
        limit_width: bool,
        width: u32,
        limit_height: bool,
        height: u32,
        loop_scroll: bool,
    },
    Sharpen {
        sharpness: f32,
    },
    ImageMaskBlend {
        mask_type: MaskBlendType,
        image_path: String,
        color_rgba: [u8; 4],
        opacity: f32,
        stretch: bool,
    },
    RenderDelay {
        delay_ms: u32,
    },
    CustomWgslShader {
        shader_name: String,
        wgsl_source: String,
        param_a: f32,
        param_b: f32,
        param_c: f32,
        param_d: f32,
    },
    PluginVideoFilter {
        plugin_id: String,
        filter_id: String,
        settings: serde_json::Value,
    },
}

impl VideoFilterKind {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::ChromaKey { .. } => "Chroma Key",
            Self::ColorKey { .. } => "Color Key",
            Self::LumaKey { .. } => "Luma Key",
            Self::ColorCorrection { .. } => "Color Correction",
            Self::CropPad { .. } => "Crop/Pad",
            Self::ScalingAspect { .. } => "Scaling/Aspect Ratio",
            Self::Scroll { .. } => "Scroll",
            Self::Sharpen { .. } => "Sharpen",
            Self::ImageMaskBlend { .. } => "Image Mask/Blend",
            Self::RenderDelay { .. } => "Render Delay",
            Self::CustomWgslShader { .. } => "Custom WGSL Shader",
            Self::PluginVideoFilter { .. } => "Plugin Filter",
        }
    }

    pub fn default_chroma_key() -> Self {
        Self::ChromaKey {
            preset: ChromaKeyPreset::Green,
            custom_color_rgba: [0, 255, 0, 255],
            similarity: 0.40,
            smoothness: 0.08,
            key_color_spill_reduction: 0.15,
            opacity: 1.0,
            contrast: 0.0,
            brightness: 0.0,
            gamma: 0.0,
        }
    }

    pub fn default_color_key() -> Self {
        Self::ColorKey {
            key_color_rgba: [0, 255, 0, 255],
            similarity: 0.08,
            smoothness: 0.05,
            opacity: 1.0,
            contrast: 0.0,
            brightness: 0.0,
            gamma: 0.0,
        }
    }

    pub fn default_luma_key() -> Self {
        Self::LumaKey {
            luma_max: 1.0,
            luma_max_smooth: 0.0,
            luma_min: 0.0,
            luma_min_smooth: 0.0,
        }
    }

    pub fn default_color_correction() -> Self {
        Self::ColorCorrection {
            gamma: 0.0,
            contrast: 0.0,
            brightness: 0.0,
            saturation: 0.0,
            hue_shift_deg: 0.0,
            opacity: 1.0,
            color_multiply_rgba: [255, 255, 255, 255],
            color_add_rgba: [0, 0, 0, 0],
        }
    }

    pub fn default_crop_pad() -> Self {
        Self::CropPad {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            relative: true,
        }
    }

    pub fn default_scaling_aspect() -> Self {
        Self::ScalingAspect {
            target_width: 1920,
            target_height: 1080,
            keep_aspect: true,
            scale_filter: "Bicubic".to_string(),
        }
    }

    pub fn default_scroll() -> Self {
        Self::Scroll {
            speed_x: 50.0,
            speed_y: 0.0,
            limit_width: false,
            width: 1920,
            limit_height: false,
            height: 1080,
            loop_scroll: true,
        }
    }

    pub fn default_sharpen() -> Self {
        Self::Sharpen { sharpness: 0.08 }
    }

    pub fn default_image_mask() -> Self {
        Self::ImageMaskBlend {
            mask_type: MaskBlendType::AlphaMaskColorChannel,
            image_path: String::new(),
            color_rgba: [255, 255, 255, 255],
            opacity: 1.0,
            stretch: true,
        }
    }

    pub fn default_render_delay() -> Self {
        Self::RenderDelay { delay_ms: 100 }
    }

    pub fn default_custom_wgsl() -> Self {
        Self::CustomWgslShader {
            shader_name: "Vignette / Scanline".to_string(),
            wgsl_source: DEFAULT_CUSTOM_WGSL_SHADER.to_string(),
            param_a: 0.45,
            param_b: 0.75,
            param_c: 0.0,
            param_d: 1.0,
        }
    }
}

pub const DEFAULT_CUSTOM_WGSL_SHADER: &str = r#"// Custom OxideStudio WGSL Fragment Filter
// Uniforms: u.size_time = vec4(width, height, time_sec, param_a)
//           u.params    = vec4(param_a, param_b, param_c, param_d)
struct CustomUniforms {
    size_time: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> u: CustomUniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let col = textureSample(src_tex, src_samp, uv);
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    let vig = 1.0 - smoothstep(u.params.y * 0.5, u.params.y * 0.95, d) * u.params.x;
    return vec4<f32>(col.rgb * vig, col.a);
}
"#;
