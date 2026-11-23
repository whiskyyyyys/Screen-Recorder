use obs_audio::MeterReading;
use obs_core::{EngineStats, SceneId, SceneItemId, SourceId, TransitionId};
use obs_encoder::EncoderProbeReport;
use obs_platform_windows::{AudioDeviceInfo, MonitorInfo, VideoDeviceInfo, WindowInfo};
use obs_plugins::PluginMetadata;
use obs_scenes::{SceneCollection, SourceKind, SourceProperties};
use obs_settings::{AppRootState, AppSettings};
use obs_streamer::StreamingServiceEntry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiNotice {
    pub id: u64,
    pub message: String,
    pub is_error: bool,
}

/// Interface bridging `obs-ui` to `ObsEngine` (`crates/app`) cleanly without circular dependencies.
pub trait StudioBackend: Send + Sync {
    fn attach_wgpu(
        &self,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter_name: String,
        backend_name: String,
    );

    fn tick_frame(&self, render_multiview: bool);

    fn program_texture_view(&self) -> Option<wgpu::TextureView>;
    fn preview_texture_view(&self) -> Option<wgpu::TextureView>;
    fn multiview_texture_view(&self) -> Option<wgpu::TextureView>;
    fn canvas_size(&self) -> (u32, u32);
    fn source_dimensions(&self, source_id: SourceId) -> Option<(u32, u32)>;

    fn stats(&self) -> EngineStats;
    fn audio_meters(&self) -> HashMap<SourceId, MeterReading>;
    fn source_errors(&self) -> HashMap<SourceId, String>;
    fn recent_notices(&self) -> Vec<UiNotice>;
    fn has_active_output(&self) -> bool;

    fn settings(&self) -> AppSettings;
    fn apply_settings(&self, settings: AppSettings) -> Result<(), String>;

    fn root_state(&self) -> AppRootState;
    fn update_dock_layout(&self, layout_json: Option<String>);

    fn collection(&self) -> SceneCollection;
    fn with_collection_mut(&self, f: &mut dyn FnMut(&mut SceneCollection));

    fn studio_mode(&self) -> bool;
    fn set_studio_mode(&self, enabled: bool);
    fn switch_scene(&self, scene_id: SceneId);
    fn set_preview_scene(&self, scene_id: SceneId);
    fn trigger_transition(&self);
    fn set_tbar_position(&self, pos: f32);
    fn tbar_position(&self) -> f32;
    fn set_active_transition(&self, id: TransitionId);
    fn set_transition_duration(&self, ms: u32);

    fn create_scene(&self, name: &str) -> Result<SceneId, String>;
    fn rename_scene(&self, id: SceneId, new_name: &str) -> Result<(), String>;
    fn duplicate_scene(&self, id: SceneId) -> Result<SceneId, String>;
    fn delete_scene(&self, id: SceneId) -> Result<(), String>;
    fn move_scene(&self, from_idx: usize, to_idx: usize);

    fn add_new_source(
        &self,
        scene_id: SceneId,
        name: &str,
        kind: SourceKind,
        props: SourceProperties,
    ) -> Result<(SourceId, SceneItemId), String>;
    fn add_existing_source(
        &self,
        scene_id: SceneId,
        source_id: SourceId,
    ) -> Result<SceneItemId, String>;
    fn remove_scene_item(&self, scene_id: SceneId, item_id: SceneItemId) -> Result<(), String>;
    fn group_items(
        &self,
        scene_id: SceneId,
        items: &[SceneItemId],
        name: &str,
    ) -> Result<SceneItemId, String>;
    fn ungroup_item(&self, scene_id: SceneId, group_id: SceneItemId) -> Result<(), String>;

    fn start_recording(&self) -> Result<(), String>;
    fn stop_recording(&self) -> Result<PathBuf, String>;
    fn pause_or_resume_recording(&self) -> Result<(), String>;

    fn start_streaming(&self) -> Result<(), String>;
    fn stop_streaming(&self) -> Result<(), String>;

    fn start_replay_buffer(&self) -> Result<(), String>;
    fn stop_replay_buffer(&self) -> Result<(), String>;
    fn save_replay_buffer(&self) -> Result<PathBuf, String>;

    fn remux_recording(
        &self,
        src: &std::path::Path,
        dst: Option<&std::path::Path>,
    ) -> Result<PathBuf, String>;

    fn media_restart(&self, id: SourceId);
    fn media_next(&self, id: SourceId);
    fn slideshow_prev(&self, id: SourceId);
    fn browser_refresh(&self, id: SourceId);

    fn list_profiles(&self) -> Vec<String>;
    fn active_profile(&self) -> String;
    fn switch_profile(&self, name: &str) -> Result<(), String>;
    fn create_profile(&self, name: &str) -> Result<(), String>;
    fn duplicate_profile(&self, new_name: &str) -> Result<(), String>;
    fn rename_profile(&self, new_name: &str) -> Result<(), String>;
    fn delete_profile(&self, name: &str) -> Result<(), String>;

    fn list_collections(&self) -> Vec<String>;
    fn active_collection_name(&self) -> String;
    fn switch_collection(&self, name: &str) -> Result<(), String>;
    fn create_collection(&self, name: &str) -> Result<(), String>;
    fn import_collection(&self, path: &std::path::Path) -> Result<(), String>;
    fn export_collection(&self, path: &std::path::Path) -> Result<(), String>;

    fn encoder_report(&self) -> EncoderProbeReport;
    fn reprobe_encoders(&self) -> EncoderProbeReport;
    fn services(&self) -> Vec<StreamingServiceEntry>;
    fn plugins(&self) -> Vec<PluginMetadata>;
    fn available_plugin_filters(&self) -> Vec<(String, String, String)>;
    fn reload_plugins(&self);

    fn enumerate_monitors(&self) -> Vec<MonitorInfo>;
    fn enumerate_windows(&self) -> Vec<WindowInfo>;
    fn enumerate_video_devices(&self) -> Vec<VideoDeviceInfo>;
    fn enumerate_audio_inputs(&self) -> Vec<AudioDeviceInfo>;
    fn enumerate_audio_outputs(&self) -> Vec<AudioDeviceInfo>;
    fn system_fonts(&self) -> Vec<String>;

    fn shutdown(&self);
}
