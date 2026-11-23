use obs_core::{SceneId, SceneItemId, SourceId};
use obs_filters::{AudioFilterInstance, VideoFilterInstance};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    DisplayCapture,
    WindowCapture,
    GameCapture,
    VideoCaptureDevice,
    AudioInputCapture,
    AudioOutputCapture,
    ApplicationAudioCapture,
    Image,
    ImageSlideshow,
    MediaSource,
    PlaylistSource,
    TextSource,
    ColorSource,
    BrowserSource,
    SceneSource,
    GroupSource,
    PluginSource,
}

impl SourceKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::DisplayCapture => "Display Capture",
            Self::WindowCapture => "Window Capture",
            Self::GameCapture => "Game Capture",
            Self::VideoCaptureDevice => "Video Capture Device",
            Self::AudioInputCapture => "Audio Input Capture",
            Self::AudioOutputCapture => "Audio Output Capture",
            Self::ApplicationAudioCapture => "Application Audio Capture",
            Self::Image => "Image",
            Self::ImageSlideshow => "Image Slideshow",
            Self::MediaSource => "Media Source",
            Self::PlaylistSource => "Playlist Video Source",
            Self::TextSource => "Text (FreeType/Fontdue)",
            Self::ColorSource => "Color Source",
            Self::BrowserSource => "Browser Source",
            Self::SceneSource => "Scene",
            Self::GroupSource => "Group",
            Self::PluginSource => "Plugin Source",
        }
    }

    pub fn obs_kind_id(self) -> &'static str {
        match self {
            Self::DisplayCapture => "monitor_capture",
            Self::WindowCapture => "window_capture",
            Self::GameCapture => "game_capture",
            Self::VideoCaptureDevice => "dshow_input",
            Self::AudioInputCapture => "wasapi_input_capture",
            Self::AudioOutputCapture => "wasapi_output_capture",
            Self::ApplicationAudioCapture => "wasapi_process_output_capture",
            Self::Image => "image_source",
            Self::ImageSlideshow => "slideshow",
            Self::MediaSource => "ffmpeg_source",
            Self::PlaylistSource => "vlc_source",
            Self::TextSource => "text_ft2_source_v2",
            Self::ColorSource => "color_source_v3",
            Self::BrowserSource => "browser_source",
            Self::SceneSource => "scene",
            Self::GroupSource => "group",
            Self::PluginSource => "plugin_source",
        }
    }

    pub fn from_obs_kind_id(id: &str) -> Option<Self> {
        match id {
            "monitor_capture" | "display_capture" => Some(Self::DisplayCapture),
            "window_capture" => Some(Self::WindowCapture),
            "game_capture" => Some(Self::GameCapture),
            "dshow_input" | "video_capture" => Some(Self::VideoCaptureDevice),
            "wasapi_input_capture" | "audio_input" => Some(Self::AudioInputCapture),
            "wasapi_output_capture" | "audio_output" => Some(Self::AudioOutputCapture),
            "wasapi_process_output_capture" | "application_audio" => {
                Some(Self::ApplicationAudioCapture)
            }
            "image_source" => Some(Self::Image),
            "slideshow" | "image_slideshow" => Some(Self::ImageSlideshow),
            "ffmpeg_source" | "media_source" => Some(Self::MediaSource),
            "vlc_source" | "playlist_source" => Some(Self::PlaylistSource),
            "text_ft2_source_v2" | "text_gdiplus_v2" | "text_source" => Some(Self::TextSource),
            "color_source_v3" | "color_source" => Some(Self::ColorSource),
            "browser_source" => Some(Self::BrowserSource),
            "scene" => Some(Self::SceneSource),
            "group" => Some(Self::GroupSource),
            "plugin_source" => Some(Self::PluginSource),
            _ => None,
        }
    }

    pub fn has_video(self) -> bool {
        !matches!(
            self,
            Self::AudioInputCapture | Self::AudioOutputCapture | Self::ApplicationAudioCapture
        )
    }

    pub fn has_audio(self) -> bool {
        matches!(
            self,
            Self::AudioInputCapture
                | Self::AudioOutputCapture
                | Self::ApplicationAudioCapture
                | Self::MediaSource
                | Self::PlaylistSource
                | Self::VideoCaptureDevice
                | Self::BrowserSource
        )
    }

    pub fn creatable_source_kinds() -> &'static [Self] {
        &[
            Self::DisplayCapture,
            Self::WindowCapture,
            Self::GameCapture,
            Self::VideoCaptureDevice,
            Self::AudioInputCapture,
            Self::AudioOutputCapture,
            Self::ApplicationAudioCapture,
            Self::Image,
            Self::ImageSlideshow,
            Self::MediaSource,
            Self::PlaylistSource,
            Self::TextSource,
            Self::ColorSource,
            Self::BrowserSource,
            Self::SceneSource,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SourceProperties {
    DisplayCapture {
        monitor_index: usize,
        capture_cursor: bool,
        force_gdi: bool,
    },
    WindowCapture {
        window_title: String,
        process_name: String,
        pid: u32,
        capture_cursor: bool,
        client_area_only: bool,
    },
    GameCapture {
        mode: String, // "AnyFullscreen" | "SpecificWindow"
        window_title: String,
        process_name: String,
        pid: u32,
        capture_cursor: bool,
        force_scaling: bool,
    },
    VideoCaptureDevice {
        device_id: String,
        device_name: String,
        width: u32,
        height: u32,
        fps: u32,
    },
    AudioInputCapture {
        device_id: String,
        device_name: String,
    },
    AudioOutputCapture {
        device_id: String,
        device_name: String,
    },
    ApplicationAudioCapture {
        pid: u32,
        process_name: String,
        window_title: String,
    },
    Image {
        file_path: String,
        unload_when_not_showing: bool,
    },
    ImageSlideshow {
        files: Vec<String>,
        slide_time_ms: u32,
        transition_speed_ms: u32,
        loop_slides: bool,
        randomize: bool,
        paused: bool,
    },
    MediaSource {
        local_file: String,
        is_network_stream: bool,
        loop_playback: bool,
        restart_on_activate: bool,
        speed_percent: u32,
        paused: bool,
    },
    PlaylistSource {
        files: Vec<String>,
        loop_playlist: bool,
        shuffle: bool,
        paused: bool,
    },
    TextSource {
        text: String,
        font_family: String,
        font_size_px: f32,
        color_rgba: [u8; 4],
        gradient_enabled: bool,
        gradient_bottom_rgba: [u8; 4],
        outline_enabled: bool,
        outline_size: f32,
        outline_color_rgba: [u8; 4],
        drop_shadow: bool,
        align: String, // "left" | "center" | "right"
        read_from_file: bool,
        file_path: String,
    },
    ColorSource {
        color_rgba: [u8; 4],
        width: u32,
        height: u32,
    },
    BrowserSource {
        url: String,
        width: u32,
        height: u32,
        fps: u32,
        custom_css: String,
        shutdown_when_not_visible: bool,
    },
    SceneSource {
        scene_id: SceneId,
    },
    GroupSource,
    PluginSource {
        plugin_id: String,
        type_id: String,
        settings: HashMap<String, serde_json::Value>,
    },
}

impl SourceProperties {
    pub fn default_for(kind: SourceKind) -> Self {
        match kind {
            SourceKind::DisplayCapture => Self::DisplayCapture {
                monitor_index: 0,
                capture_cursor: true,
                force_gdi: false,
            },
            SourceKind::WindowCapture => Self::WindowCapture {
                window_title: String::new(),
                process_name: String::new(),
                pid: 0,
                capture_cursor: true,
                client_area_only: true,
            },
            SourceKind::GameCapture => Self::GameCapture {
                mode: "AnyFullscreen".to_string(),
                window_title: String::new(),
                process_name: String::new(),
                pid: 0,
                capture_cursor: true,
                force_scaling: false,
            },
            SourceKind::VideoCaptureDevice => Self::VideoCaptureDevice {
                device_id: "default".to_string(),
                device_name: "Default Camera".to_string(),
                width: 1280,
                height: 720,
                fps: 30,
            },
            SourceKind::AudioInputCapture => Self::AudioInputCapture {
                device_id: "default".to_string(),
                device_name: "Default Microphone".to_string(),
            },
            SourceKind::AudioOutputCapture => Self::AudioOutputCapture {
                device_id: "default".to_string(),
                device_name: "Default Desktop Audio".to_string(),
            },
            SourceKind::ApplicationAudioCapture => Self::ApplicationAudioCapture {
                pid: 0,
                process_name: String::new(),
                window_title: String::new(),
            },
            SourceKind::Image => Self::Image {
                file_path: String::new(),
                unload_when_not_showing: false,
            },
            SourceKind::ImageSlideshow => Self::ImageSlideshow {
                files: Vec::new(),
                slide_time_ms: 4000,
                transition_speed_ms: 600,
                loop_slides: true,
                randomize: false,
                paused: false,
            },
            SourceKind::MediaSource => Self::MediaSource {
                local_file: String::new(),
                is_network_stream: false,
                loop_playback: false,
                restart_on_activate: true,
                speed_percent: 100,
                paused: false,
            },
            SourceKind::PlaylistSource => Self::PlaylistSource {
                files: Vec::new(),
                loop_playlist: true,
                shuffle: false,
                paused: false,
            },
            SourceKind::TextSource => Self::TextSource {
                text: "OxideStudio Live".to_string(),
                font_family: "Segoe UI".to_string(),
                font_size_px: 56.0,
                color_rgba: [255, 255, 255, 255],
                gradient_enabled: false,
                gradient_bottom_rgba: [200, 220, 255, 255],
                outline_enabled: true,
                outline_size: 2.5,
                outline_color_rgba: [0, 0, 0, 255],
                drop_shadow: true,
                align: "left".to_string(),
                read_from_file: false,
                file_path: String::new(),
            },
            SourceKind::ColorSource => Self::ColorSource {
                color_rgba: [24, 32, 48, 255],
                width: 1920,
                height: 1080,
            },
            SourceKind::BrowserSource => Self::BrowserSource {
                url: "https://obsproject.com/browser-source".to_string(),
                width: 1280,
                height: 720,
                fps: 30,
                custom_css: String::new(),
                shutdown_when_not_visible: false,
            },
            SourceKind::SceneSource => Self::SceneSource {
                scene_id: SceneId::new(),
            },
            SourceKind::GroupSource => Self::GroupSource,
            SourceKind::PluginSource => Self::PluginSource {
                plugin_id: String::new(),
                type_id: String::new(),
                settings: HashMap::new(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioMonitoringType {
    #[default]
    None,
    MonitorOnly,
    MonitorAndOutput,
}

impl AudioMonitoringType {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::None => "Monitor Off",
            Self::MonitorOnly => "Monitor Only (mute output)",
            Self::MonitorAndOutput => "Monitor and Output",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAudioSettings {
    /// Linear multiplier (1.0 = 0 dB)
    pub volume_mul: f32,
    pub muted: bool,
    pub mono_downmix: bool,
    /// -1.0 (left) .. 0.0 (center) .. +1.0 (right)
    pub balance: f32,
    pub sync_offset_ms: i32,
    /// Bitmask for audio tracks 1..=6 (bit 0..5)
    pub track_mask: u8,
    pub monitoring: AudioMonitoringType,
}

impl Default for SourceAudioSettings {
    fn default() -> Self {
        Self {
            volume_mul: 1.0,
            muted: false,
            mono_downmix: false,
            balance: 0.0,
            sync_offset_ms: 0,
            track_mask: 0b0011_1111,
            monitoring: AudioMonitoringType::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDefinition {
    pub id: SourceId,
    pub name: String,
    pub kind: SourceKind,
    pub properties: SourceProperties,
    pub video_filters: Vec<VideoFilterInstance>,
    pub audio_filters: Vec<AudioFilterInstance>,
    pub audio: SourceAudioSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Additive,
    Subtract,
    Screen,
    Multiply,
    Lighten,
    Darken,
}

impl BlendMode {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Additive => "Additive",
            Self::Subtract => "Subtract",
            Self::Screen => "Screen",
            Self::Multiply => "Multiply",
            Self::Lighten => "Lighten",
            Self::Darken => "Darken",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BoundsType {
    #[default]
    None,
    Stretch,
    ScaleInner,
    ScaleOuter,
    ScaleToWidth,
    ScaleToHeight,
    MaxOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CropRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemTransform {
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_deg: f32,
    pub crop: CropRect,
    pub bounds_type: BoundsType,
    pub bounds_size: [f32; 2],
    /// 0=top-left, 4=center, etc.
    pub alignment: u32,
    pub opacity: f32,
}

impl Default for ItemTransform {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation_deg: 0.0,
            crop: CropRect::default(),
            bounds_type: BoundsType::None,
            bounds_size: [1920.0, 1080.0],
            alignment: 0,
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneItem {
    pub id: SceneItemId,
    pub source_id: SourceId,
    pub visible: bool,
    pub locked: bool,
    pub blend_mode: BlendMode,
    pub transform: ItemTransform,
    /// When `Some`, this SceneItem acts as an expandable OBS Group containing nested items.
    pub group_children: Option<Vec<SceneItem>>,
    pub group_expanded: bool,
}

impl SceneItem {
    pub fn new(source_id: SourceId) -> Self {
        Self {
            id: SceneItemId::new(),
            source_id,
            visible: true,
            locked: false,
            blend_mode: BlendMode::Normal,
            transform: ItemTransform::default(),
            group_children: None,
            group_expanded: true,
        }
    }

    pub fn is_group(&self) -> bool {
        self.group_children.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub items: Vec<SceneItem>,
}

/// Flattened world-space render command produced after expanding groups and nested scenes.
#[derive(Debug, Clone)]
pub struct ResolvedRenderItem {
    pub item_id: SceneItemId,
    pub source_id: SourceId,
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_deg: f32,
    pub crop: CropRect,
    pub bounds_type: BoundsType,
    pub bounds_size: [f32; 2],
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub filters: Vec<VideoFilterInstance>,
}
