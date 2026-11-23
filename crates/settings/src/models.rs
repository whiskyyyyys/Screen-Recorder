use obs_video::VideoConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    HighContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StreamProtocol {
    #[default]
    Rtmp,
    Rtmps,
    Srt,
}

impl StreamProtocol {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Rtmp => "RTMP",
            Self::Rtmps => "RTMPS (TLS)",
            Self::Srt => "SRT (Caller)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OutputMode {
    #[default]
    Simple,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecordingFormat {
    #[default]
    Mkv,
    Mp4,
    FragmentedMp4,
    Mov,
    Flv,
}

impl RecordingFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mkv => "mkv",
            Self::Mp4 | Self::FragmentedMp4 => "mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Mkv => "Matroska Video (.mkv)",
            Self::Mp4 => "MPEG-4 (.mp4)",
            Self::FragmentedMp4 => "Fragmented MP4 (.mp4)",
            Self::Mov => "QuickTime (.mov)",
            Self::Flv => "Flash Video (.flv)",
        }
    }

    pub fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mkv => "matroska",
            Self::Mp4 | Self::FragmentedMp4 => "mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecordingQualityPreset {
    SameAsStream,
    #[default]
    HighQuality,
    Indistinguishable,
    Lossless,
}

impl RecordingQualityPreset {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::SameAsStream => "Same as Stream",
            Self::HighQuality => "High Quality, Medium File Size",
            Self::Indistinguishable => "Indistinguishable Quality, Large File Size",
            Self::Lossless => "Lossless Quality, Tremendously Large File Size",
        }
    }

    pub fn crf(self) -> u8 {
        match self {
            Self::SameAsStream => 21,
            Self::HighQuality => 18,
            Self::Indistinguishable => 14,
            Self::Lossless => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub theme: ThemeMode,
    pub language: String,
    pub confirm_on_stream_start: bool,
    pub confirm_on_stream_stop: bool,
    pub confirm_on_record_stop: bool,
    pub auto_record_when_streaming: bool,
    pub keep_recording_when_stream_stops: bool,
    pub snap_enabled: bool,
    pub snap_distance_px: f32,
    pub snap_to_screen_edges: bool,
    pub snap_to_sources: bool,
    pub show_active_outputs_warning_on_exit: bool,
    pub system_tray_enabled: bool,
    pub open_stats_on_startup: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            language: "en-US".to_string(),
            confirm_on_stream_start: false,
            confirm_on_stream_stop: false,
            confirm_on_record_stop: false,
            auto_record_when_streaming: false,
            keep_recording_when_stream_stops: true,
            snap_enabled: true,
            snap_distance_px: 10.0,
            snap_to_screen_edges: true,
            snap_to_sources: true,
            show_active_outputs_warning_on_exit: true,
            system_tray_enabled: true,
            open_stats_on_startup: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraOutputConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub stream_key: String,
    pub protocol: StreamProtocol,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSettings {
    pub service_name: String,
    pub server_name: String,
    pub server_url: String,
    pub stream_key: String,
    pub protocol: StreamProtocol,
    pub use_auth: bool,
    pub username: String,
    pub password: String,
    pub srt_latency_ms: u32,
    pub srt_passphrase: String,
    pub ignore_service_recommendations: bool,
    pub extra_outputs: Vec<ExtraOutputConfig>,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            service_name: "Custom".to_string(),
            server_name: "Custom RTMP".to_string(),
            server_url: "rtmp://127.0.0.1:1935/live".to_string(),
            stream_key: String::new(),
            protocol: StreamProtocol::Rtmp,
            use_auth: false,
            username: String::new(),
            password: String::new(),
            srt_latency_ms: 120,
            srt_passphrase: String::new(),
            ignore_service_recommendations: false,
            extra_outputs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSettings {
    pub mode: OutputMode,
    pub stream_video_encoder: String,
    pub stream_audio_encoder: String,
    pub stream_bitrate_kbps: u32,
    pub stream_audio_bitrate_kbps: u32,
    pub stream_rate_control: String,
    pub stream_preset: String,
    pub stream_keyint_sec: u32,
    pub stream_audio_track: u8,
    pub stream_rescale_enabled: bool,
    pub stream_rescale_width: u32,
    pub stream_rescale_height: u32,

    pub rec_path: String,
    pub rec_filename_format: String,
    pub rec_format: RecordingFormat,
    pub rec_quality_preset: RecordingQualityPreset,
    pub rec_video_encoder: String,
    pub rec_audio_encoder: String,
    pub rec_rate_control: String,
    pub rec_bitrate_kbps: u32,
    pub rec_crf: u8,
    pub rec_preset: String,
    pub rec_keyint_sec: u32,
    /// Bitmask of tracks 1..=6 (bit 0 = track 1, bit 5 = track 6).
    pub rec_audio_tracks: u8,
    pub rec_audio_bitrate_kbps: u32,
    pub auto_remux_to_mp4: bool,

    pub replay_enabled: bool,
    pub replay_duration_sec: u32,
    pub replay_max_memory_mb: u32,
    pub replay_prefix: String,
    pub replay_suffix: String,
}

impl Default for OutputSettings {
    fn default() -> Self {
        let default_videos = directories::UserDirs::new()
            .and_then(|u| u.video_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            mode: OutputMode::Simple,
            stream_video_encoder: "auto".to_string(),
            stream_audio_encoder: "aac".to_string(),
            stream_bitrate_kbps: 6000,
            stream_audio_bitrate_kbps: 160,
            stream_rate_control: "CBR".to_string(),
            stream_preset: "balanced".to_string(),
            stream_keyint_sec: 2,
            stream_audio_track: 1,
            stream_rescale_enabled: false,
            stream_rescale_width: 1280,
            stream_rescale_height: 720,

            rec_path: default_videos.to_string_lossy().to_string(),
            rec_filename_format: "%CCYY-%MM-%DD %hh-%mm-%ss".to_string(),
            rec_format: RecordingFormat::Mkv,
            rec_quality_preset: RecordingQualityPreset::HighQuality,
            rec_video_encoder: "auto".to_string(),
            rec_audio_encoder: "aac".to_string(),
            rec_rate_control: "CQP".to_string(),
            rec_bitrate_kbps: 18000,
            rec_crf: 18,
            rec_preset: "balanced".to_string(),
            rec_keyint_sec: 2,
            rec_audio_tracks: 0b0000_0001,
            rec_audio_bitrate_kbps: 192,
            auto_remux_to_mp4: false,

            replay_enabled: false,
            replay_duration_sec: 20,
            replay_max_memory_mb: 512,
            replay_prefix: "Replay".to_string(),
            replay_suffix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSettings {
    pub sample_rate: u32,
    pub channels: String,
    pub desktop_device_1: String,
    pub desktop_device_2: String,
    pub mic_device_1: String,
    pub mic_device_2: String,
    pub monitoring_device: String,
    pub meter_decay_rate_db_per_sec: f32,
    pub peak_meter_type: String,
    pub low_latency_audio_buffering: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: "Stereo".to_string(),
            desktop_device_1: "Default".to_string(),
            desktop_device_2: "Disabled".to_string(),
            mic_device_1: "Default".to_string(),
            mic_device_2: "Disabled".to_string(),
            monitoring_device: "Default".to_string(),
            meter_decay_rate_db_per_sec: 23.53,
            peak_meter_type: "SamplePeak".to_string(),
            low_latency_audio_buffering: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoSettings {
    pub config: VideoConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub action: String,
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl HotkeyBinding {
    pub fn display_combo(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if !self.key.is_empty() {
            parts.push(&self.key);
        }
        if parts.is_empty() {
            "Unbound".to_string()
        } else {
            parts.join(" + ")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeySettings {
    pub enabled: bool,
    pub bindings: Vec<HotkeyBinding>,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            bindings: vec![
                HotkeyBinding {
                    action: "record.start_stop".to_string(),
                    key: "F9".to_string(),
                    ctrl: true,
                    alt: false,
                    shift: true,
                },
                HotkeyBinding {
                    action: "record.pause_resume".to_string(),
                    key: "F10".to_string(),
                    ctrl: true,
                    alt: false,
                    shift: true,
                },
                HotkeyBinding {
                    action: "stream.start_stop".to_string(),
                    key: "F11".to_string(),
                    ctrl: true,
                    alt: false,
                    shift: true,
                },
                HotkeyBinding {
                    action: "replay.save".to_string(),
                    key: "F8".to_string(),
                    ctrl: true,
                    alt: false,
                    shift: true,
                },
                HotkeyBinding {
                    action: "studio.transition".to_string(),
                    key: "Space".to_string(),
                    ctrl: true,
                    alt: true,
                    shift: false,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibilitySettings {
    pub high_contrast: bool,
    pub ui_scale: f32,
    pub selection_color_rgba: [u8; 4],
    pub crop_color_rgba: [u8; 4],
    pub hover_color_rgba: [u8; 4],
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        Self {
            high_contrast: false,
            ui_scale: 1.0,
            selection_color_rgba: [255, 64, 64, 255],
            crop_color_rgba: [64, 224, 96, 255],
            hover_color_rgba: [96, 176, 255, 255],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedSettings {
    pub process_priority: String,
    pub auto_reconnect: bool,
    pub reconnect_delay_sec: u32,
    pub max_retries: u32,
    pub dynamic_bitrate: bool,
    pub bind_ip: String,
    pub stream_delay_enabled: bool,
    pub stream_delay_sec: u32,
    pub websocket_enabled: bool,
    pub websocket_host: String,
    pub websocket_port: u16,
    pub websocket_auth_enabled: bool,
    pub websocket_password: String,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            process_priority: "Normal".to_string(),
            auto_reconnect: true,
            reconnect_delay_sec: 5,
            max_retries: 20,
            dynamic_bitrate: true,
            bind_ip: "0.0.0.0".to_string(),
            stream_delay_enabled: false,
            stream_delay_sec: 10,
            websocket_enabled: true,
            websocket_host: "127.0.0.1".to_string(),
            websocket_port: 4455,
            websocket_auth_enabled: false,
            websocket_password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginSettings {
    pub disabled_plugins: Vec<String>,
    pub extra_search_dirs: Vec<String>,
    pub plugin_configs: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    pub general: GeneralSettings,
    pub stream: StreamSettings,
    pub output: OutputSettings,
    pub audio: AudioSettings,
    pub video: VideoSettings,
    pub hotkeys: HotkeySettings,
    pub accessibility: AccessibilitySettings,
    pub advanced: AdvancedSettings,
    pub plugins: PluginSettings,
}
