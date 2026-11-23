use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecordState {
    #[default]
    Stopped,
    Starting,
    Recording,
    Paused,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StreamState {
    #[default]
    Stopped,
    Connecting,
    Streaming,
    Reconnecting,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReplayBufferState {
    #[default]
    Stopped,
    Active,
    Saving,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecordingStats {
    pub state: RecordState,
    pub output_path: String,
    pub duration_ms: u64,
    pub bytes_written: u64,
    pub encoded_frames: u64,
    pub skipped_frames: u64,
    pub bitrate_kbps: f64,
    pub encoder_name: String,
    pub format_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamingStats {
    pub state: StreamState,
    pub service_name: String,
    pub server_url: String,
    pub duration_ms: u64,
    pub bytes_sent: u64,
    pub total_frames: u64,
    pub dropped_frames: u64,
    pub dropped_percent: f64,
    pub bitrate_kbps: f64,
    pub dynamic_bitrate_kbps: u32,
    pub rtt_ms: f64,
    pub reconnect_attempts: u32,
    pub active_extra_outputs: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplayStats {
    pub state: ReplayBufferState,
    pub buffered_seconds: f64,
    pub buffered_bytes: u64,
    pub cluster_count: usize,
    pub last_saved_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EngineStats {
    pub target_fps: f64,
    pub active_fps: f64,
    pub render_time_ms: f64,
    pub encode_time_ms: f64,
    pub total_rendered_frames: u64,
    pub lagged_frames: u64,
    pub skipped_frames: u64,
    pub cpu_usage_percent: f32,
    pub memory_usage_mb: f64,
    pub active_video_encoder: String,
    pub gpu_adapter_name: String,
    pub gpu_backend: String,
    pub websocket_clients: usize,
    pub recording: RecordingStats,
    pub streaming: StreamingStats,
    pub replay: ReplayStats,
}
