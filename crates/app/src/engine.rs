use obs_audio::{AudioMixer, MeterReading};
use obs_core::{
    Clock, EngineEvent, EngineStats, EventBus, RecordState, ReplayBufferState, SceneId,
    SceneItemId, SourceId, StreamState, TransitionId,
};
use obs_encoder::{probe_all_encoders, resolve_best_video_encoder, EncoderProbeReport};
use obs_filters::db_to_linear;
use obs_platform_windows::{
    enumerate_audio_devices, enumerate_monitors, enumerate_video_devices, enumerate_windows,
    AudioDeviceInfo, AudioEndpointKind, HotkeyListener, MonitorInfo, VideoDeviceInfo,
    WasapiRenderStream, WindowInfo,
};
use obs_plugins::{PluginManager, PluginMetadata};
use obs_recorder::{
    expand_recording_filename, remux_to_mp4, RecordingConfig, RecordingSession, ReplayBufferConfig,
    ReplayBufferSession,
};
use obs_renderer::{GpuCompositor, PluginFilterCallback};
use obs_scenes::{SceneCollection, SourceKind, SourceProperties};
use obs_settings::{AppRootState, AppSettings, ProfileManager, RecordingFormat};
use obs_sources::{list_system_fonts, SourceManager};
use obs_streamer::{
    load_streaming_services, StreamSession, StreamSessionConfig, StreamingServiceEntry,
};
use obs_transitions::ActiveTransition;
use obs_ui::StudioBackend;
use obs_websocket::{
    start_websocket_server, WebSocketBackend, WebSocketConfig, WebSocketServerHandle,
};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct ObsEngine {
    events: EventBus,
    profile_mgr: ProfileManager,
    root_state: RwLock<AppRootState>,
    settings: RwLock<AppSettings>,
    collection: RwLock<SceneCollection>,

    audio_mixer: Arc<AudioMixer>,
    source_mgr: Arc<SourceManager>,
    compositor: Mutex<Option<GpuCompositor>>,
    plugin_mgr: Arc<PluginManager>,
    encoder_report: RwLock<EncoderProbeReport>,
    services: Vec<StreamingServiceEntry>,

    active_transition: Mutex<Option<ActiveTransition>>,
    tbar_pos: Mutex<f32>,
    recorder: Mutex<Option<RecordingSession>>,
    replay_buf: Mutex<Option<ReplayBufferSession>>,
    streamer: Mutex<Option<StreamSession>>,

    hotkey_listener: Mutex<Option<HotkeyListener>>,
    ws_handle: Mutex<Option<WebSocketServerHandle>>,
    tokio_rt: tokio::runtime::Runtime,
    self_weak: Mutex<std::sync::Weak<Self>>,

    audio_thread_running: Arc<AtomicBool>,
    audio_thread: Mutex<Option<JoinHandle<()>>>,

    stats_snapshot: RwLock<EngineStats>,
    notices: Mutex<VecDeque<String>>,
    last_frame_instant: Mutex<Instant>,
    last_encode_instant: Mutex<Instant>,
    render_time_ema: Mutex<f64>,
    fps_ema: Mutex<f64>,
    total_frames: Mutex<u64>,
    lagged_frames: Mutex<u64>,
}

impl ObsEngine {
    pub fn initialize(headless: bool) -> Arc<Self> {
        let clock = Clock::new();
        let events = EventBus::new(512);
        let profile_mgr = ProfileManager::discover();
        let root_state = profile_mgr.load_root_state();
        let settings = profile_mgr
            .load_profile(&root_state.active_profile)
            .unwrap_or_default();

        let col_path = profile_mgr.collection_path(&root_state.active_collection);
        let collection = if col_path.exists() {
            SceneCollection::load_from_file(&col_path)
                .unwrap_or_else(|_| SceneCollection::default_starter(&root_state.active_collection))
        } else {
            let def = SceneCollection::default_starter(&root_state.active_collection);
            let _ = def.save_to_file(&col_path);
            def
        };

        let audio_mixer = Arc::new(AudioMixer::new(settings.audio.sample_rate));
        audio_mixer.set_global_mono(settings.audio.channels == "Mono");
        audio_mixer.set_meter_decay(settings.audio.meter_decay_rate_db_per_sec);

        let target_fps = settings.video.config.fps_f64().round().clamp(10.0, 120.0) as u32;
        let source_mgr = Arc::new(SourceManager::new(
            clock.clone(),
            Arc::clone(&audio_mixer),
            target_fps,
        ));
        source_mgr.sync_collection(&collection);

        let plugin_mgr = Arc::new(PluginManager::new());
        let mut extra_plugin_dirs = settings.plugins.extra_search_dirs.clone();
        extra_plugin_dirs.push(profile_mgr.user_plugins_dir().to_string_lossy().to_string());
        plugin_mgr.discover_and_load(
            &extra_plugin_dirs,
            &settings.plugins.disabled_plugins,
            &settings.plugins.plugin_configs,
        );

        let compositor = if headless {
            GpuCompositor::new_headless(settings.video.config.clone()).ok()
        } else {
            None
        };

        let encoder_report = probe_all_encoders();
        let services = load_streaming_services();
        let tokio_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .build()
            .expect("Failed building Tokio runtime");

        let engine = Arc::new(Self {
            events,
            profile_mgr,
            root_state: RwLock::new(root_state),
            settings: RwLock::new(settings),
            collection: RwLock::new(collection),
            audio_mixer,
            source_mgr,
            compositor: Mutex::new(compositor),
            plugin_mgr,
            encoder_report: RwLock::new(encoder_report),
            services,
            active_transition: Mutex::new(None),
            tbar_pos: Mutex::new(0.0),
            recorder: Mutex::new(None),
            replay_buf: Mutex::new(None),
            streamer: Mutex::new(None),
            hotkey_listener: Mutex::new(None),
            ws_handle: Mutex::new(None),
            tokio_rt,
            self_weak: Mutex::new(std::sync::Weak::new()),
            audio_thread_running: Arc::new(AtomicBool::new(true)),
            audio_thread: Mutex::new(None),
            stats_snapshot: RwLock::new(EngineStats::default()),
            notices: Mutex::new(VecDeque::with_capacity(16)),
            last_frame_instant: Mutex::new(Instant::now()),
            last_encode_instant: Mutex::new(Instant::now()),
            render_time_ema: Mutex::new(0.0),
            fps_ema: Mutex::new(60.0),
            total_frames: Mutex::new(0),
            lagged_frames: Mutex::new(0),
        });

        *engine.self_weak.lock() = Arc::downgrade(&engine);
        engine.install_plugin_hook_if_compositor();
        engine.start_audio_thread();
        engine.start_hotkeys();
        if !headless {
            engine.restart_websocket_if_enabled();
        }
        engine
    }

    fn install_plugin_hook_if_compositor(&self) {
        let pm = Arc::clone(&self.plugin_mgr);
        let hook: PluginFilterCallback = Arc::new(move |pid, fid, cfg, rgba, w, h| {
            pm.process_video_filter(pid, fid, cfg, rgba, w, h);
        });
        if let Some(comp) = self.compositor.lock().as_mut() {
            comp.set_plugin_filter_hook(hook);
        }
    }

    fn push_notice(&self, msg: impl Into<String>) {
        let m = msg.into();
        let mut q = self.notices.lock();
        if q.len() >= 16 {
            q.pop_front();
        }
        q.push_back(m);
    }

    fn start_audio_thread(self: &Arc<Self>) {
        let eng = Arc::clone(self);
        let running = Arc::clone(&self.audio_thread_running);
        let handle = thread::Builder::new()
            .name("obs-audio-mixer".to_string())
            .spawn(move || {
                let mut monitor_dev_id = String::new();
                let mut monitor_out: Option<WasapiRenderStream> = None;

                while running.load(Ordering::Relaxed) {
                    let sr = eng.audio_mixer.sample_rate();
                    let block_frames = ((sr as u64 * 20) / 1000) as usize; // 20ms block
                    let t0 = Instant::now();
                    let block = eng.audio_mixer.mix_block(block_frames);

                    // Monitoring audio output via WASAPI if any audio is active
                    let has_mon_audio = block.monitor.iter().any(|&s| s.abs() > 1e-6);
                    if has_mon_audio {
                        let desired_id = eng.settings.read().audio.monitoring_device.clone();
                        if desired_id != "Disabled" {
                            if monitor_out.is_none() || monitor_dev_id != desired_id {
                                monitor_out = WasapiRenderStream::open_output(&desired_id).ok();
                                monitor_dev_id = desired_id;
                            }
                            if let Some(m) = &monitor_out {
                                let out_sr = m.sample_rate();
                                if out_sr != block.sample_rate {
                                    let rs = obs_audio::resample_stereo_linear(
                                        &block.monitor,
                                        block.sample_rate,
                                        out_sr,
                                    );
                                    m.write_stereo_f32(&rs);
                                } else {
                                    m.write_stereo_f32(&block.monitor);
                                }
                            }
                        }
                    }

                    if let Some(rec) = eng.recorder.lock().as_ref() {
                        rec.push_audio_tracks(&block.tracks);
                    }
                    let stream_track_idx = eng
                        .settings
                        .read()
                        .output
                        .stream_audio_track
                        .saturating_sub(1)
                        .min(5) as usize;
                    if let Some(st) = eng.streamer.lock().as_ref() {
                        st.push_audio_f32(&block.tracks[stream_track_idx]);
                    }
                    if let Some(rb) = eng.replay_buf.lock().as_ref() {
                        rb.push_audio_f32(&block.tracks[0]);
                    }

                    let elapsed = t0.elapsed();
                    let target = Duration::from_millis(20);
                    if elapsed < target {
                        thread::sleep(target - elapsed);
                    }
                }
            })
            .ok();
        *self.audio_thread.lock() = handle;
    }

    fn start_hotkeys(self: &Arc<Self>) {
        let cfg = self.settings.read().hotkeys.clone();
        let eng = Arc::downgrade(self);
        let listener = HotkeyListener::start(cfg.bindings, cfg.enabled, move |action| {
            if let Some(e) = eng.upgrade() {
                let _ = e.handle_hotkey_action(&action);
            }
        });
        *self.hotkey_listener.lock() = Some(listener);
    }

    fn handle_hotkey_action(&self, action: &str) -> Result<(), String> {
        match action {
            "record.start_stop" => {
                let active = self.recorder.lock().is_some();
                if active {
                    let p = self.stop_recording()?;
                    self.push_notice(format!("Recording saved: {}", p.display()));
                } else {
                    self.start_recording()?;
                }
            }
            "record.pause_resume" => {
                self.pause_or_resume_recording()?;
            }
            "stream.start_stop" => {
                let active = self.streamer.lock().is_some();
                if active {
                    self.stop_streaming()?;
                } else {
                    self.start_streaming()?;
                }
            }
            "replay.save" => {
                let p = StudioBackend::save_replay_buffer(self)?;
                self.push_notice(format!("Replay saved: {}", p.display()));
            }
            "studio.transition" => {
                StudioBackend::trigger_transition(self);
            }
            other if other.starts_with("scene:") => {
                let name = &other["scene:".len()..];
                let id = self.collection.read().scene_by_name(name).map(|s| s.id);
                if let Some(sid) = id {
                    self.switch_scene(sid);
                }
            }
            _ => return Err(format!("Unknown hotkey action: {action}")),
        }
        Ok(())
    }

    pub fn restart_websocket_if_enabled(self: &Arc<Self>) {
        if let Some(mut old) = self.ws_handle.lock().take() {
            old.stop();
        }
        let adv = self.settings.read().advanced.clone();
        if !adv.websocket_enabled {
            return;
        }
        let cfg = WebSocketConfig {
            host: adv.websocket_host,
            port: adv.websocket_port,
            auth_enabled: adv.websocket_auth_enabled,
            password: adv.websocket_password,
        };
        let backend: Arc<dyn WebSocketBackend> = self.clone();
        let ev = self.events.clone();
        if let Ok(handle) = self
            .tokio_rt
            .block_on(start_websocket_server(cfg, backend, ev))
        {
            *self.ws_handle.lock() = Some(handle);
        }
    }

    pub fn ws_bound_port(&self) -> Option<u16> {
        self.ws_handle.lock().as_ref().map(|h| h.bound_addr.port())
    }

    fn save_active_collection(&self) {
        let name = self.root_state.read().active_collection.clone();
        let path = self.profile_mgr.collection_path(&name);
        let _ = self.collection.read().save_to_file(&path);
    }

    pub fn run_self_test(self: &Arc<Self>) -> Result<(), String> {
        println!("[1/6] Checking GPU Compositor...");
        {
            let guard = self.compositor.lock();
            let comp = guard
                .as_ref()
                .ok_or_else(|| "Headless GPU compositor failed to initialize".to_string())?;
            println!(
                "      Adapter: {} ({})",
                comp.adapter_name(),
                comp.backend_name()
            );
        }

        println!("[2/6] Checking Windows Display & Audio Devices...");
        let mons = self.enumerate_monitors();
        let inputs = self.enumerate_audio_inputs();
        let outputs = self.enumerate_audio_outputs();
        println!(
            "      Monitors: {}, Audio Inputs: {}, Audio Outputs: {}",
            mons.len(),
            inputs.len(),
            outputs.len()
        );

        println!("[3/6] Probing Hardware & Software Encoders...");
        let rep = self.encoder_report();
        if !rep.ffmpeg_found {
            return Err("ffmpeg.exe not found".to_string());
        }
        let ready: Vec<String> = rep
            .video_encoders
            .iter()
            .filter(|e| e.available)
            .map(|e| e.id.clone())
            .collect();
        println!("      Available Encoders: {}", ready.join(", "));

        println!("[4/6] Checking Dynamic Plugin Loader...");
        let plugs = self.plugins();
        println!("      Discovered Plugins: {}", plugs.len());

        println!("[5/6] Verifying Live Frame Render + 1-second Real MKV Recording...");
        let tmp_mkv = std::env::temp_dir().join("oxidestudio-selftest.mkv");
        let _ = std::fs::remove_file(&tmp_mkv);
        let rec_enc = resolve_best_video_encoder("auto", &rep);
        let (ow, oh) = self.settings.read().video.config.sanitized_output_size();
        let rec = RecordingSession::start(RecordingConfig {
            output_path: tmp_mkv.clone(),
            format: RecordingFormat::Mkv,
            width: ow,
            height: oh,
            fps: 30,
            sample_rate: 48000,
            video_encoder: rec_enc.clone(),
            audio_encoder: "aac".to_string(),
            rate_control: "CBR".to_string(),
            bitrate_kbps: 4000,
            crf: 20,
            preset: "speed".to_string(),
            keyint_sec: 1,
            audio_track_mask: 1,
            audio_bitrate_kbps: 128,
            color_space: obs_video::ColorSpace::Rec709,
            color_range: obs_video::ColorRange::Partial,
            auto_remux_to_mp4: false,
        })
        .map_err(|e| format!("Self-test start recording failed: {e}"))?;

        let silent_track = vec![0.0f32; 960 * 2];
        let tracks = [
            silent_track.clone(),
            silent_track.clone(),
            silent_track.clone(),
            silent_track.clone(),
            silent_track.clone(),
            silent_track,
        ];
        for _ in 0..30 {
            let frames = self
                .source_mgr
                .collect_video_frames(&self.collection.read());
            if let Some(comp) = self.compositor.lock().as_mut() {
                if let Some(nv12) =
                    comp.render_frame(&self.collection.read(), &frames, None, false, false, true)
                {
                    rec.push_video_nv12(nv12);
                }
            }
            rec.push_audio_tracks(&tracks);
            thread::sleep(Duration::from_millis(20));
        }
        let saved = rec
            .stop()
            .map_err(|e| format!("Self-test stop failed: {e}"))?;
        let meta = std::fs::metadata(&saved).map_err(|e| format!("Output missing: {e}"))?;
        if meta.len() < 1024 {
            return Err("Self-test MKV output unexpectedly small".to_string());
        }
        println!(
            "      Recorded {} bytes using {} -> {}",
            meta.len(),
            rec_enc,
            saved.display()
        );
        let _ = std::fs::remove_file(&saved);

        println!("[6/6] All OxideStudio self-test checks PASSED.");
        Ok(())
    }
}

impl StudioBackend for ObsEngine {
    fn attach_wgpu(
        &self,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter_name: String,
        backend_name: String,
    ) {
        let cfg = self.settings.read().video.config.clone();
        let mut comp = GpuCompositor::from_wgpu(device, queue, adapter_name, backend_name, cfg);
        let pm = Arc::clone(&self.plugin_mgr);
        comp.set_plugin_filter_hook(Arc::new(move |pid, fid, cfg, rgba, w, h| {
            pm.process_video_filter(pid, fid, cfg, rgba, w, h);
        }));
        *self.compositor.lock() = Some(comp);
    }

    fn tick_frame(&self, render_multiview: bool) {
        let t_start = Instant::now();
        let dt_ms = {
            let mut last = self.last_frame_instant.lock();
            let d = last.elapsed().as_secs_f64() * 1000.0;
            *last = t_start;
            d.max(0.1)
        };

        // Advance active transition if running
        let mut finished_to_scene: Option<(SceneId, String)> = None;
        {
            let mut tr_guard = self.active_transition.lock();
            if let Some(tr) = tr_guard.as_mut() {
                if !tr.manual_t_bar && tr.advance(dt_ms as f32) {
                    finished_to_scene = Some((tr.to_scene, tr.config.name.clone()));
                    *tr_guard = None;
                }
            }
        }
        if let Some((new_prog, tr_name)) = finished_to_scene {
            let sname = {
                let mut col = self.collection.write();
                let old_prog = col.active_scene_id;
                col.active_scene_id = new_prog;
                if self.root_state.read().studio_mode {
                    col.preview_scene_id = Some(old_prog);
                }
                col.scene_by_id(new_prog)
                    .map(|s| s.name.clone())
                    .unwrap_or_default()
            };
            self.events
                .publish(EngineEvent::TransitionEnded { name: tr_name });
            self.events.publish(EngineEvent::SceneSwitched {
                scene_id: new_prog,
                scene_name: sname,
            });
        }

        let col = self.collection.read().clone();
        let frames = self.source_mgr.collect_video_frames(&col);

        let rec_active = self
            .recorder
            .lock()
            .as_ref()
            .map(|r| !r.is_paused())
            .unwrap_or(false);
        let stream_active = self.streamer.lock().is_some();
        let replay_active = self.replay_buf.lock().is_some();
        let any_output = rec_active || stream_active || replay_active;

        let target_interval_us = self.settings.read().video.config.frame_interval_us();
        let should_push_encode = if any_output {
            let mut last_enc = self.last_encode_instant.lock();
            if last_enc.elapsed().as_micros() as u64 >= target_interval_us.saturating_sub(1500) {
                *last_enc = t_start;
                true
            } else {
                false
            }
        } else {
            false
        };

        let studio = self.root_state.read().studio_mode;
        let tr_opt = self.active_transition.lock().clone();

        let (adapter_name, backend_name) = {
            let mut comp_guard = self.compositor.lock();
            if let Some(comp) = comp_guard.as_mut() {
                let nv12_opt = comp.render_frame(
                    &col,
                    &frames,
                    tr_opt.as_ref(),
                    studio,
                    render_multiview,
                    should_push_encode,
                );
                if let Some(nv12) = nv12_opt {
                    if let Some(rec) = self.recorder.lock().as_ref() {
                        rec.push_video_nv12(nv12);
                    }
                    if let Some(st) = self.streamer.lock().as_ref() {
                        st.push_video_nv12(nv12);
                    }
                    if let Some(rb) = self.replay_buf.lock().as_ref() {
                        rb.push_video_nv12(nv12);
                    }
                }
                (
                    comp.adapter_name().to_string(),
                    comp.backend_name().to_string(),
                )
            } else {
                (String::new(), String::new())
            }
        };

        let render_ms = t_start.elapsed().as_secs_f64() * 1000.0;
        let inst_fps = (1000.0 / dt_ms).clamp(1.0, 240.0);
        {
            let mut r_ema = self.render_time_ema.lock();
            *r_ema = if *r_ema == 0.0 {
                render_ms
            } else {
                *r_ema * 0.9 + render_ms * 0.1
            };
            let mut f_ema = self.fps_ema.lock();
            *f_ema = *f_ema * 0.9 + inst_fps * 0.1;
        }
        let tot = {
            let mut tf = self.total_frames.lock();
            *tf += 1;
            *tf
        };
        if render_ms > (target_interval_us as f64 / 1000.0) {
            *self.lagged_frames.lock() += 1;
        }

        let rec_stats = self
            .recorder
            .lock()
            .as_ref()
            .map(|r| r.stats())
            .unwrap_or_default();
        let st_stats = self
            .streamer
            .lock()
            .as_ref()
            .map(|s| s.stats())
            .unwrap_or_default();
        let rb_stats = self
            .replay_buf
            .lock()
            .as_ref()
            .map(|rb| rb.stats())
            .unwrap_or_default();
        let ws_clients = self
            .ws_handle
            .lock()
            .as_ref()
            .map(|h| h.client_count.load(Ordering::Relaxed))
            .unwrap_or(0);

        let report = self.encoder_report.read();
        let active_enc =
            resolve_best_video_encoder(&self.settings.read().output.rec_video_encoder, &report);

        let mut stats = self.stats_snapshot.write();
        stats.target_fps = self.settings.read().video.config.fps_f64();
        stats.active_fps = *self.fps_ema.lock();
        stats.render_time_ms = *self.render_time_ema.lock();
        stats.total_rendered_frames = tot;
        stats.lagged_frames = *self.lagged_frames.lock();
        stats.skipped_frames = rec_stats.skipped_frames;
        stats.memory_usage_mb = query_process_memory_mb();
        stats.active_video_encoder = active_enc;
        stats.gpu_adapter_name = adapter_name;
        stats.gpu_backend = backend_name;
        stats.websocket_clients = ws_clients;
        stats.recording = rec_stats;
        stats.streaming = st_stats;
        stats.replay = rb_stats;
    }

    fn program_texture_view(&self) -> Option<wgpu::TextureView> {
        self.compositor
            .lock()
            .as_ref()
            .map(|c| c.program_view().clone())
    }

    fn preview_texture_view(&self) -> Option<wgpu::TextureView> {
        self.compositor
            .lock()
            .as_ref()
            .map(|c| c.preview_view().clone())
    }

    fn multiview_texture_view(&self) -> Option<wgpu::TextureView> {
        self.compositor
            .lock()
            .as_ref()
            .map(|c| c.multiview_view().clone())
    }

    fn canvas_size(&self) -> (u32, u32) {
        self.compositor
            .lock()
            .as_ref()
            .map(|c| c.canvas_size())
            .unwrap_or_else(|| self.settings.read().video.config.sanitized_canvas_size())
    }

    fn source_dimensions(&self, source_id: SourceId) -> Option<(u32, u32)> {
        self.compositor
            .lock()
            .as_ref()
            .and_then(|c| c.source_dimensions(source_id))
    }

    fn stats(&self) -> EngineStats {
        self.stats_snapshot.read().clone()
    }

    fn audio_meters(&self) -> HashMap<SourceId, MeterReading> {
        self.audio_mixer.meters_snapshot()
    }

    fn source_errors(&self) -> HashMap<SourceId, String> {
        self.source_mgr.source_errors()
    }

    fn recent_notices(&self) -> Vec<String> {
        self.notices.lock().drain(..).collect()
    }

    fn settings(&self) -> AppSettings {
        self.settings.read().clone()
    }

    fn apply_settings(&self, new_settings: AppSettings) -> Result<(), String> {
        let profile = self.root_state.read().active_profile.clone();
        self.profile_mgr
            .save_profile(&profile, &new_settings)
            .map_err(|e| e.to_string())?;

        self.audio_mixer
            .set_sample_rate(new_settings.audio.sample_rate);
        self.audio_mixer
            .set_global_mono(new_settings.audio.channels == "Mono");
        self.audio_mixer
            .set_meter_decay(new_settings.audio.meter_decay_rate_db_per_sec);

        self.source_mgr.set_target_fps(
            new_settings
                .video
                .config
                .fps_f64()
                .round()
                .clamp(10.0, 120.0) as u32,
        );
        if let Some(comp) = self.compositor.lock().as_mut() {
            comp.update_video_config(new_settings.video.config.clone());
        }
        if let Some(hk) = self.hotkey_listener.lock().as_ref() {
            hk.update(
                new_settings.hotkeys.bindings.clone(),
                new_settings.hotkeys.enabled,
            );
        }

        let ws_changed = {
            let old = self.settings.read();
            old.advanced.websocket_enabled != new_settings.advanced.websocket_enabled
                || old.advanced.websocket_port != new_settings.advanced.websocket_port
                || old.advanced.websocket_host != new_settings.advanced.websocket_host
                || old.advanced.websocket_auth_enabled
                    != new_settings.advanced.websocket_auth_enabled
                || old.advanced.websocket_password != new_settings.advanced.websocket_password
        };

        *self.settings.write() = new_settings;
        if ws_changed {
            if let Some(eng) = self.self_weak.lock().upgrade() {
                eng.restart_websocket_if_enabled();
            }
        }
        Ok(())
    }

    fn root_state(&self) -> AppRootState {
        self.root_state.read().clone()
    }

    fn update_dock_layout(&self, layout_json: Option<String>) {
        let mut rs = self.root_state.write();
        rs.dock_layout_json = layout_json;
        let _ = self.profile_mgr.save_root_state(&rs);
    }

    fn collection(&self) -> SceneCollection {
        self.collection.read().clone()
    }

    fn with_collection_mut(&self, f: &mut dyn FnMut(&mut SceneCollection)) {
        let col_snapshot = {
            let mut col = self.collection.write();
            f(&mut col);
            col.clone()
        };
        self.source_mgr.sync_collection(&col_snapshot);
        self.save_active_collection();
    }

    fn studio_mode(&self) -> bool {
        self.root_state.read().studio_mode
    }

    fn set_studio_mode(&self, enabled: bool) {
        {
            let mut rs = self.root_state.write();
            rs.studio_mode = enabled;
            let _ = self.profile_mgr.save_root_state(&rs);
        }
        if enabled {
            let mut col = self.collection.write();
            if col.preview_scene_id.is_none() {
                col.preview_scene_id = Some(col.active_scene_id);
            }
        }
        self.events
            .publish(EngineEvent::StudioModeChanged { enabled });
    }

    fn switch_scene(&self, scene_id: SceneId) {
        let (from_id, tr_cfg, target_name) = {
            let mut col = self.collection.write();
            if col.active_scene_id == scene_id {
                return;
            }
            let mut tr = col
                .active_transition()
                .cloned()
                .unwrap_or_else(|| obs_transitions::TransitionConfig::default_presets()[1].clone());
            tr.duration_ms = col.transition_duration_ms;
            let from = col.active_scene_id;
            let name = col
                .scene_by_id(scene_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if matches!(tr.kind, obs_transitions::TransitionKind::Cut) || tr.duration_ms == 0 {
                col.active_scene_id = scene_id;
                (from, None, name)
            } else {
                (from, Some(tr), name)
            }
        };

        if let Some(tr) = tr_cfg {
            self.events.publish(EngineEvent::TransitionStarted {
                name: tr.name.clone(),
                duration_ms: tr.duration_ms,
            });
            *self.active_transition.lock() =
                Some(ActiveTransition::start_auto(tr, from_id, scene_id));
        } else {
            self.events.publish(EngineEvent::SceneSwitched {
                scene_id,
                scene_name: target_name,
            });
        }
        self.save_active_collection();
    }

    fn set_preview_scene(&self, scene_id: SceneId) {
        let sname = {
            let mut col = self.collection.write();
            col.preview_scene_id = Some(scene_id);
            col.scene_by_id(scene_id)
                .map(|s| s.name.clone())
                .unwrap_or_default()
        };
        self.events.publish(EngineEvent::PreviewSceneChanged {
            scene_id,
            scene_name: sname,
        });
    }

    fn trigger_transition(&self) {
        let target = self
            .collection
            .read()
            .preview_scene_id
            .unwrap_or(self.collection.read().active_scene_id);
        *self.tbar_pos.lock() = 0.0;
        self.switch_scene(target);
    }

    fn set_tbar_position(&self, pos: f32) {
        let p = pos.clamp(0.0, 1.0);
        *self.tbar_pos.lock() = p;
        let col = self.collection.read().clone();
        let preview_id = col.preview_scene_id.unwrap_or(col.active_scene_id);
        if preview_id == col.active_scene_id {
            return;
        }
        if p >= 0.99 {
            *self.active_transition.lock() = None;
            *self.tbar_pos.lock() = 0.0;
            let mut w = self.collection.write();
            let old = w.active_scene_id;
            w.active_scene_id = preview_id;
            w.preview_scene_id = Some(old);
            let sname = w
                .scene_by_id(preview_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            self.events.publish(EngineEvent::SceneSwitched {
                scene_id: preview_id,
                scene_name: sname,
            });
        } else if p <= 0.01 {
            *self.active_transition.lock() = None;
        } else {
            let tr = col
                .active_transition()
                .cloned()
                .unwrap_or_else(|| obs_transitions::TransitionConfig::default_presets()[1].clone());
            *self.active_transition.lock() = Some(ActiveTransition::set_t_bar(
                tr,
                col.active_scene_id,
                preview_id,
                p,
            ));
        }
    }

    fn tbar_position(&self) -> f32 {
        *self.tbar_pos.lock()
    }

    fn set_active_transition(&self, id: TransitionId) {
        self.collection.write().active_transition_id = id;
        self.save_active_collection();
    }

    fn set_transition_duration(&self, ms: u32) {
        self.collection.write().transition_duration_ms = ms;
        self.save_active_collection();
    }

    fn create_scene(&self, name: &str) -> Result<SceneId, String> {
        let id = self
            .collection
            .write()
            .create_scene(name)
            .map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::SceneListChanged);
        self.save_active_collection();
        Ok(id)
    }

    fn rename_scene(&self, id: SceneId, new_name: &str) -> Result<(), String> {
        self.collection
            .write()
            .rename_scene(id, new_name)
            .map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::SceneListChanged);
        self.save_active_collection();
        Ok(())
    }

    fn duplicate_scene(&self, id: SceneId) -> Result<SceneId, String> {
        let nid = self
            .collection
            .write()
            .duplicate_scene(id, None)
            .map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::SceneListChanged);
        self.save_active_collection();
        Ok(nid)
    }

    fn delete_scene(&self, id: SceneId) -> Result<(), String> {
        self.collection
            .write()
            .delete_scene(id)
            .map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::SceneListChanged);
        self.save_active_collection();
        Ok(())
    }

    fn move_scene(&self, from_idx: usize, to_idx: usize) {
        let _ = self.collection.write().reorder_scene(from_idx, to_idx);
        self.events.publish(EngineEvent::SceneListChanged);
        self.save_active_collection();
    }

    fn add_new_source(
        &self,
        scene_id: SceneId,
        name: &str,
        kind: SourceKind,
        props: SourceProperties,
    ) -> Result<(SourceId, SceneItemId), String> {
        let res = self
            .collection
            .write()
            .create_source_and_add(scene_id, name, kind, props)
            .map_err(|e| e.to_string())?;
        self.source_mgr.sync_collection(&self.collection.read());
        self.save_active_collection();
        Ok(res)
    }

    fn add_existing_source(
        &self,
        scene_id: SceneId,
        source_id: SourceId,
    ) -> Result<SceneItemId, String> {
        let res = self
            .collection
            .write()
            .add_existing_source_to_scene(scene_id, source_id)
            .map_err(|e| e.to_string())?;
        self.source_mgr.sync_collection(&self.collection.read());
        self.save_active_collection();
        Ok(res)
    }

    fn remove_scene_item(&self, scene_id: SceneId, item_id: SceneItemId) -> Result<(), String> {
        self.collection
            .write()
            .remove_scene_item(scene_id, item_id)
            .map_err(|e| e.to_string())?;
        self.source_mgr.sync_collection(&self.collection.read());
        self.save_active_collection();
        Ok(())
    }

    fn group_items(
        &self,
        scene_id: SceneId,
        items: &[SceneItemId],
        name: &str,
    ) -> Result<SceneItemId, String> {
        let id = self
            .collection
            .write()
            .group_items(scene_id, items, name)
            .map_err(|e| e.to_string())?;
        self.save_active_collection();
        Ok(id)
    }

    fn ungroup_item(&self, scene_id: SceneId, group_id: SceneItemId) -> Result<(), String> {
        self.collection
            .write()
            .ungroup_item(scene_id, group_id)
            .map_err(|e| e.to_string())?;
        self.save_active_collection();
        Ok(())
    }

    fn start_recording(&self) -> Result<(), String> {
        if self.recorder.lock().is_some() {
            return Err("Recording already in progress".to_string());
        }
        let s = self.settings.read().clone();
        let report = self.encoder_report.read();
        let encoder_id = resolve_best_video_encoder(&s.output.rec_video_encoder, &report);
        let (ow, oh) = s.video.config.sanitized_output_size();
        let fps = s.video.config.fps_f64().round().clamp(1.0, 240.0) as u32;
        let out_path = expand_recording_filename(
            &s.output.rec_path,
            &s.output.rec_filename_format,
            s.output.rec_format,
        );

        let crf = if s.output.mode == obs_settings::OutputMode::Simple {
            s.output.rec_quality_preset.crf()
        } else {
            s.output.rec_crf
        };

        let session = RecordingSession::start(RecordingConfig {
            output_path: out_path.clone(),
            format: s.output.rec_format,
            width: ow,
            height: oh,
            fps,
            sample_rate: s.audio.sample_rate,
            video_encoder: encoder_id,
            audio_encoder: s.output.rec_audio_encoder,
            rate_control: s.output.rec_rate_control,
            bitrate_kbps: s.output.rec_bitrate_kbps,
            crf,
            preset: s.output.rec_preset,
            keyint_sec: s.output.rec_keyint_sec,
            audio_track_mask: s.output.rec_audio_tracks,
            audio_bitrate_kbps: s.output.rec_audio_bitrate_kbps,
            color_space: s.video.config.color_space,
            color_range: s.video.config.color_range,
            auto_remux_to_mp4: s.output.auto_remux_to_mp4,
        })
        .map_err(|e| e.to_string())?;

        *self.recorder.lock() = Some(session);
        self.events.publish(EngineEvent::RecordStateChanged {
            state: RecordState::Recording,
            output_path: Some(out_path.to_string_lossy().to_string()),
        });
        Ok(())
    }

    fn stop_recording(&self) -> Result<PathBuf, String> {
        let session = self
            .recorder
            .lock()
            .take()
            .ok_or_else(|| "Recording is not active".to_string())?;
        let path = session.stop().map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::RecordStateChanged {
            state: RecordState::Stopped,
            output_path: Some(path.to_string_lossy().to_string()),
        });
        Ok(path)
    }

    fn pause_or_resume_recording(&self) -> Result<(), String> {
        let guard = self.recorder.lock();
        let rec = guard
            .as_ref()
            .ok_or_else(|| "Recording is not active".to_string())?;
        let state = if rec.is_paused() {
            rec.resume();
            RecordState::Recording
        } else {
            rec.pause();
            RecordState::Paused
        };
        self.events.publish(EngineEvent::RecordStateChanged {
            state,
            output_path: None,
        });
        Ok(())
    }

    fn start_streaming(&self) -> Result<(), String> {
        if self.streamer.lock().is_some() {
            return Err("Streaming is already active".to_string());
        }
        let s = self.settings.read().clone();
        if s.stream.stream_key.trim().is_empty()
            && s.stream.protocol != obs_settings::StreamProtocol::Srt
        {
            return Err("Stream key is empty. Configure it in Settings -> Stream.".to_string());
        }
        let report = self.encoder_report.read();
        let encoder_id = resolve_best_video_encoder(&s.output.stream_video_encoder, &report);
        let (ow, oh) = s.video.config.sanitized_output_size();
        let fps = s.video.config.fps_f64().round().clamp(1.0, 240.0) as u32;

        let sess = StreamSession::start(
            StreamSessionConfig {
                service_name: s.stream.service_name,
                server_url: s.stream.server_url,
                stream_key: s.stream.stream_key,
                protocol: s.stream.protocol,
                srt_latency_ms: s.stream.srt_latency_ms,
                srt_passphrase: s.stream.srt_passphrase,
                extra_outputs: s.stream.extra_outputs,
                width: ow,
                height: oh,
                fps,
                sample_rate: s.audio.sample_rate,
                video_encoder: encoder_id,
                audio_encoder: s.output.stream_audio_encoder,
                bitrate_kbps: s.output.stream_bitrate_kbps,
                audio_bitrate_kbps: s.output.stream_audio_bitrate_kbps,
                keyint_sec: s.output.stream_keyint_sec,
                preset: s.output.stream_preset,
                color_space: s.video.config.color_space,
                color_range: s.video.config.color_range,
                auto_reconnect: s.advanced.auto_reconnect,
                reconnect_delay_sec: s.advanced.reconnect_delay_sec,
                max_retries: s.advanced.max_retries,
                dynamic_bitrate: s.advanced.dynamic_bitrate,
            },
            self.tokio_rt.handle().clone(),
        )
        .map_err(|e| e.to_string())?;

        *self.streamer.lock() = Some(sess);
        self.events.publish(EngineEvent::StreamStateChanged {
            state: StreamState::Connecting,
            error: None,
        });

        if s.general.auto_record_when_streaming && self.recorder.lock().is_none() {
            let _ = self.start_recording();
        }
        Ok(())
    }

    fn stop_streaming(&self) -> Result<(), String> {
        let mut sess = self
            .streamer
            .lock()
            .take()
            .ok_or_else(|| "Stream is not active".to_string())?;
        sess.stop();
        self.events.publish(EngineEvent::StreamStateChanged {
            state: StreamState::Stopped,
            error: None,
        });
        let s = self.settings.read();
        if s.general.auto_record_when_streaming && !s.general.keep_recording_when_stream_stops {
            drop(s);
            let _ = self.stop_recording();
        }
        Ok(())
    }

    fn start_replay_buffer(&self) -> Result<(), String> {
        if self.replay_buf.lock().is_some() {
            return Err("Replay buffer already running".to_string());
        }
        let s = self.settings.read().clone();
        let report = self.encoder_report.read();
        let encoder_id = resolve_best_video_encoder(&s.output.rec_video_encoder, &report);
        let (ow, oh) = s.video.config.sanitized_output_size();
        let fps = s.video.config.fps_f64().round().clamp(1.0, 240.0) as u32;

        let rb = ReplayBufferSession::start(ReplayBufferConfig {
            width: ow,
            height: oh,
            fps,
            sample_rate: s.audio.sample_rate,
            encoder_id,
            bitrate_kbps: s.output.rec_bitrate_kbps,
            audio_bitrate_kbps: s.output.rec_audio_bitrate_kbps,
            color_space: s.video.config.color_space,
            color_range: s.video.config.color_range,
            max_duration_sec: s.output.replay_duration_sec,
            max_memory_mb: s.output.replay_max_memory_mb,
        })
        .map_err(|e| e.to_string())?;

        *self.replay_buf.lock() = Some(rb);
        self.events.publish(EngineEvent::ReplayBufferStateChanged {
            state: ReplayBufferState::Active,
        });
        Ok(())
    }

    fn stop_replay_buffer(&self) -> Result<(), String> {
        let mut rb = self
            .replay_buf
            .lock()
            .take()
            .ok_or_else(|| "Replay buffer is not running".to_string())?;
        rb.stop();
        self.events.publish(EngineEvent::ReplayBufferStateChanged {
            state: ReplayBufferState::Stopped,
        });
        Ok(())
    }

    fn save_replay_buffer(&self) -> Result<PathBuf, String> {
        let s = self.settings.read().clone();
        let ts = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
        let fname = format!(
            "{} {}{}",
            s.output.replay_prefix, ts, s.output.replay_suffix
        );
        let out_path = PathBuf::from(&s.output.rec_path)
            .join(format!("{fname}.{}", s.output.rec_format.extension()));
        let guard = self.replay_buf.lock();
        let rb = guard
            .as_ref()
            .ok_or_else(|| "Replay buffer is not running".to_string())?;
        let saved = rb.save_replay(&out_path).map_err(|e| e.to_string())?;
        self.events.publish(EngineEvent::ReplaySaved {
            saved_path: saved.to_string_lossy().to_string(),
        });
        Ok(saved)
    }

    fn remux_recording(&self, src: &Path, dst: Option<&Path>) -> Result<PathBuf, String> {
        remux_to_mp4(src, dst).map_err(|e| e.to_string())
    }

    fn media_restart(&self, id: SourceId) {
        self.source_mgr.media_restart(id);
    }

    fn media_next(&self, id: SourceId) {
        self.source_mgr.media_next(id);
    }

    fn slideshow_prev(&self, id: SourceId) {
        self.source_mgr.slideshow_prev(id);
    }

    fn browser_refresh(&self, id: SourceId) {
        self.source_mgr.browser_refresh(id);
    }

    fn list_profiles(&self) -> Vec<String> {
        self.profile_mgr.list_profiles()
    }

    fn active_profile(&self) -> String {
        self.root_state.read().active_profile.clone()
    }

    fn switch_profile(&self, name: &str) -> Result<(), String> {
        let new_settings = self
            .profile_mgr
            .load_profile(name)
            .map_err(|e| e.to_string())?;
        {
            let mut rs = self.root_state.write();
            rs.active_profile = name.to_string();
            let _ = self.profile_mgr.save_root_state(&rs);
        }
        self.apply_settings(new_settings)?;
        self.events.publish(EngineEvent::ProfileChanged {
            profile: name.to_string(),
        });
        Ok(())
    }

    fn create_profile(&self, name: &str) -> Result<(), String> {
        self.profile_mgr
            .create_profile(name, None)
            .map_err(|e| e.to_string())?;
        self.switch_profile(name)
    }

    fn duplicate_profile(&self, new_name: &str) -> Result<(), String> {
        let cur = self.settings.read().clone();
        self.profile_mgr
            .create_profile(new_name, Some(&cur))
            .map_err(|e| e.to_string())?;
        self.switch_profile(new_name)
    }

    fn rename_profile(&self, new_name: &str) -> Result<(), String> {
        let old = self.active_profile();
        self.profile_mgr
            .rename_profile(&old, new_name)
            .map_err(|e| e.to_string())?;
        let mut rs = self.root_state.write();
        rs.active_profile = new_name.to_string();
        let _ = self.profile_mgr.save_root_state(&rs);
        Ok(())
    }

    fn delete_profile(&self, name: &str) -> Result<(), String> {
        self.profile_mgr
            .delete_profile(name)
            .map_err(|e| e.to_string())?;
        let next = self
            .profile_mgr
            .list_profiles()
            .into_iter()
            .next()
            .unwrap_or_else(|| "Default".to_string());
        self.switch_profile(&next)
    }

    fn list_collections(&self) -> Vec<String> {
        self.profile_mgr.list_collections()
    }

    fn active_collection_name(&self) -> String {
        self.root_state.read().active_collection.clone()
    }

    fn switch_collection(&self, name: &str) -> Result<(), String> {
        self.save_active_collection();
        let path = self.profile_mgr.collection_path(name);
        let col = if path.exists() {
            SceneCollection::load_from_file(&path).map_err(|e| e.to_string())?
        } else {
            let def = SceneCollection::default_starter(name);
            def.save_to_file(&path).map_err(|e| e.to_string())?;
            def
        };
        {
            let mut rs = self.root_state.write();
            rs.active_collection = name.to_string();
            let _ = self.profile_mgr.save_root_state(&rs);
        }
        self.source_mgr.sync_collection(&col);
        *self.collection.write() = col;
        self.events.publish(EngineEvent::SceneCollectionChanged {
            collection: name.to_string(),
        });
        Ok(())
    }

    fn create_collection(&self, name: &str) -> Result<(), String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("Collection name cannot be empty".to_string());
        }
        let path = self.profile_mgr.collection_path(trimmed);
        let col = SceneCollection::default_starter(trimmed);
        col.save_to_file(&path).map_err(|e| e.to_string())?;
        self.switch_collection(trimmed)
    }

    fn import_collection(&self, path: &Path) -> Result<(), String> {
        let col = SceneCollection::load_from_file(path).map_err(|e| e.to_string())?;
        let name = col.name.clone();
        let dst = self.profile_mgr.collection_path(&name);
        col.save_to_file(&dst).map_err(|e| e.to_string())?;
        self.switch_collection(&name)
    }

    fn export_collection(&self, path: &Path) -> Result<(), String> {
        self.collection
            .read()
            .save_to_file(path)
            .map_err(|e| e.to_string())
    }

    fn encoder_report(&self) -> EncoderProbeReport {
        self.encoder_report.read().clone()
    }

    fn reprobe_encoders(&self) -> EncoderProbeReport {
        let rep = probe_all_encoders();
        *self.encoder_report.write() = rep.clone();
        rep
    }

    fn services(&self) -> Vec<StreamingServiceEntry> {
        self.services.clone()
    }

    fn plugins(&self) -> Vec<PluginMetadata> {
        self.plugin_mgr.list_plugins()
    }

    fn available_plugin_filters(&self) -> Vec<(String, String, String)> {
        self.plugin_mgr.available_video_filters()
    }

    fn reload_plugins(&self) {
        let s = self.settings.read().clone();
        let mut dirs = s.plugins.extra_search_dirs;
        dirs.push(
            self.profile_mgr
                .user_plugins_dir()
                .to_string_lossy()
                .to_string(),
        );
        self.plugin_mgr.discover_and_load(
            &dirs,
            &s.plugins.disabled_plugins,
            &s.plugins.plugin_configs,
        );
    }

    fn enumerate_monitors(&self) -> Vec<MonitorInfo> {
        enumerate_monitors()
    }

    fn enumerate_windows(&self) -> Vec<WindowInfo> {
        enumerate_windows()
    }

    fn enumerate_video_devices(&self) -> Vec<VideoDeviceInfo> {
        enumerate_video_devices()
    }

    fn enumerate_audio_inputs(&self) -> Vec<AudioDeviceInfo> {
        enumerate_audio_devices(AudioEndpointKind::Input)
    }

    fn enumerate_audio_outputs(&self) -> Vec<AudioDeviceInfo> {
        enumerate_audio_devices(AudioEndpointKind::Output)
    }

    fn system_fonts(&self) -> Vec<String> {
        list_system_fonts()
    }

    fn shutdown(&self) {
        self.save_active_collection();
        if let Some(rec) = self.recorder.lock().take() {
            let _ = rec.stop();
        }
        if let Some(mut st) = self.streamer.lock().take() {
            st.stop();
        }
        if let Some(mut rb) = self.replay_buf.lock().take() {
            rb.stop();
        }
        if let Some(mut ws) = self.ws_handle.lock().take() {
            ws.stop();
        }
        self.hotkey_listener.lock().take();
        self.source_mgr.stop_all();
        self.audio_thread_running.store(false, Ordering::Relaxed);
        if let Some(h) = self.audio_thread.lock().take() {
            let _ = h.join();
        }
        self.plugin_mgr.unload_all();
    }
}

impl WebSocketBackend for ObsEngine {
    fn stats(&self) -> EngineStats {
        StudioBackend::stats(self)
    }

    fn list_scenes(&self) -> (String, Option<String>, Vec<String>) {
        let col = self.collection.read();
        let prog = col
            .active_scene()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let prev = col
            .preview_scene_id
            .and_then(|id| col.scene_by_id(id))
            .map(|s| s.name.clone());
        let names = col.scenes.iter().map(|s| s.name.clone()).collect();
        (prog, prev, names)
    }

    fn set_current_program_scene(&self, scene_name: &str) -> Result<(), String> {
        let id = self
            .collection
            .read()
            .scene_by_name(scene_name)
            .map(|s| s.id)
            .ok_or_else(|| format!("Scene '{scene_name}' not found"))?;
        self.switch_scene(id);
        Ok(())
    }

    fn set_current_preview_scene(&self, scene_name: &str) -> Result<(), String> {
        let id = self
            .collection
            .read()
            .scene_by_name(scene_name)
            .map(|s| s.id)
            .ok_or_else(|| format!("Scene '{scene_name}' not found"))?;
        self.set_preview_scene(id);
        Ok(())
    }

    fn create_scene(&self, scene_name: &str) -> Result<(), String> {
        StudioBackend::create_scene(self, scene_name).map(|_| ())
    }

    fn remove_scene(&self, scene_name: &str) -> Result<(), String> {
        let id = self
            .collection
            .read()
            .scene_by_name(scene_name)
            .map(|s| s.id)
            .ok_or_else(|| format!("Scene '{scene_name}' not found"))?;
        StudioBackend::delete_scene(self, id)
    }

    fn set_scene_name(&self, scene_name: &str, new_name: &str) -> Result<(), String> {
        let id = self
            .collection
            .read()
            .scene_by_name(scene_name)
            .map(|s| s.id)
            .ok_or_else(|| format!("Scene '{scene_name}' not found"))?;
        StudioBackend::rename_scene(self, id, new_name)
    }

    fn get_studio_mode(&self) -> bool {
        StudioBackend::studio_mode(self)
    }

    fn set_studio_mode(&self, enabled: bool) {
        StudioBackend::set_studio_mode(self, enabled)
    }

    fn trigger_transition(&self) {
        StudioBackend::trigger_transition(self)
    }

    fn get_transition_info(&self) -> (String, u32, Vec<String>) {
        let col = self.collection.read();
        let cur = col
            .active_transition()
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let list = col.transitions.iter().map(|t| t.name.clone()).collect();
        (cur, col.transition_duration_ms, list)
    }

    fn set_current_transition(&self, name: &str) -> Result<(), String> {
        let id = self
            .collection
            .read()
            .transitions
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.id)
            .ok_or_else(|| format!("Transition '{name}' not found"))?;
        self.set_active_transition(id);
        Ok(())
    }

    fn set_transition_duration(&self, duration_ms: u32) {
        StudioBackend::set_transition_duration(self, duration_ms)
    }

    fn set_tbar_position(&self, pos: f32) {
        StudioBackend::set_tbar_position(self, pos)
    }

    fn list_scene_items(&self, scene_name: &str) -> Result<Vec<Value>, String> {
        let col = self.collection.read();
        let sc = if scene_name.is_empty() {
            col.active_scene()
        } else {
            col.scene_by_name(scene_name)
        }
        .ok_or_else(|| format!("Scene '{scene_name}' not found"))?;

        Ok(sc
            .items
            .iter()
            .enumerate()
            .map(|(idx, it)| {
                let name = col
                    .sources
                    .get(&it.source_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or("Unknown");
                json!({
                    "sceneItemId": idx as i64 + 1,
                    "sourceUuid": it.source_id.to_string(),
                    "sourceName": name,
                    "sceneItemEnabled": it.visible,
                    "sceneItemLocked": it.locked,
                    "sceneItemIndex": idx,
                    "sceneItemTransform": {
                        "positionX": it.transform.pos[0],
                        "positionY": it.transform.pos[1],
                        "scaleX": it.transform.scale[0],
                        "scaleY": it.transform.scale[1],
                        "rotation": it.transform.rotation_deg,
                        "cropLeft": it.transform.crop.left,
                        "cropTop": it.transform.crop.top,
                        "cropRight": it.transform.crop.right,
                        "cropBottom": it.transform.crop.bottom,
                    }
                })
            })
            .collect())
    }

    fn set_scene_item_enabled(
        &self,
        scene_name: &str,
        item_id: i64,
        enabled: bool,
    ) -> Result<(), String> {
        let mut col = self.collection.write();
        let sc = if scene_name.is_empty() {
            col.active_scene_mut()
        } else {
            col.scenes.iter_mut().find(|s| s.name == scene_name)
        }
        .ok_or_else(|| "Scene not found".to_string())?;
        let idx = (item_id - 1).max(0) as usize;
        let item = sc
            .items
            .get_mut(idx)
            .ok_or_else(|| "Scene item not found".to_string())?;
        item.visible = enabled;
        Ok(())
    }

    fn set_scene_item_locked(
        &self,
        scene_name: &str,
        item_id: i64,
        locked: bool,
    ) -> Result<(), String> {
        let mut col = self.collection.write();
        let sc = if scene_name.is_empty() {
            col.active_scene_mut()
        } else {
            col.scenes.iter_mut().find(|s| s.name == scene_name)
        }
        .ok_or_else(|| "Scene not found".to_string())?;
        let idx = (item_id - 1).max(0) as usize;
        let item = sc
            .items
            .get_mut(idx)
            .ok_or_else(|| "Scene item not found".to_string())?;
        item.locked = locked;
        Ok(())
    }

    fn set_scene_item_transform(
        &self,
        scene_name: &str,
        item_id: i64,
        tr: &Value,
    ) -> Result<(), String> {
        let mut col = self.collection.write();
        let sc = if scene_name.is_empty() {
            col.active_scene_mut()
        } else {
            col.scenes.iter_mut().find(|s| s.name == scene_name)
        }
        .ok_or_else(|| "Scene not found".to_string())?;
        let idx = (item_id - 1).max(0) as usize;
        let item = sc
            .items
            .get_mut(idx)
            .ok_or_else(|| "Scene item not found".to_string())?;
        if let Some(x) = tr.get("positionX").and_then(|v| v.as_f64()) {
            item.transform.pos[0] = x as f32;
        }
        if let Some(y) = tr.get("positionY").and_then(|v| v.as_f64()) {
            item.transform.pos[1] = y as f32;
        }
        if let Some(sx) = tr.get("scaleX").and_then(|v| v.as_f64()) {
            item.transform.scale[0] = sx as f32;
        }
        if let Some(sy) = tr.get("scaleY").and_then(|v| v.as_f64()) {
            item.transform.scale[1] = sy as f32;
        }
        if let Some(rot) = tr.get("rotation").and_then(|v| v.as_f64()) {
            item.transform.rotation_deg = rot as f32;
        }
        Ok(())
    }

    fn remove_scene_item(&self, scene_name: &str, item_id: i64) -> Result<(), String> {
        let (sid, iid) = {
            let col = self.collection.read();
            let sc = if scene_name.is_empty() {
                col.active_scene()
            } else {
                col.scene_by_name(scene_name)
            }
            .ok_or_else(|| "Scene not found".to_string())?;
            let idx = (item_id - 1).max(0) as usize;
            let it = sc
                .items
                .get(idx)
                .ok_or_else(|| "Scene item not found".to_string())?;
            (sc.id, it.id)
        };
        StudioBackend::remove_scene_item(self, sid, iid)
    }

    fn list_inputs(&self) -> Vec<Value> {
        self.collection
            .read()
            .sources
            .values()
            .map(|s| {
                json!({
                    "inputUuid": s.id.to_string(),
                    "inputName": s.name,
                    "inputKind": s.kind.obs_kind_id(),
                })
            })
            .collect()
    }

    fn get_input_volume(&self, input_name: &str) -> Result<(f32, f32), String> {
        let col = self.collection.read();
        let src = col
            .sources
            .values()
            .find(|s| s.name == input_name)
            .ok_or_else(|| format!("Input '{input_name}' not found"))?;
        Ok((
            src.audio.volume_mul,
            obs_filters::linear_to_db(src.audio.volume_mul),
        ))
    }

    fn set_input_volume(
        &self,
        input_name: &str,
        volume_mul: Option<f32>,
        volume_db: Option<f32>,
    ) -> Result<(), String> {
        let mul = volume_mul
            .or_else(|| volume_db.map(db_to_linear))
            .ok_or_else(|| "Missing inputVolumeMul or inputVolumeDb".to_string())?
            .clamp(0.0, 4.0);
        let (sid, sname) = {
            let mut col = self.collection.write();
            let src = col
                .sources
                .values_mut()
                .find(|s| s.name == input_name)
                .ok_or_else(|| format!("Input '{input_name}' not found"))?;
            src.audio.volume_mul = mul;
            (src.id, src.name.clone())
        };
        self.source_mgr.sync_collection(&self.collection.read());
        self.events.publish(EngineEvent::SourceVolumeChanged {
            source_id: sid,
            name: sname,
            volume_db: obs_filters::linear_to_db(mul),
            volume_mul: mul,
        });
        Ok(())
    }

    fn get_input_mute(&self, input_name: &str) -> Result<bool, String> {
        let col = self.collection.read();
        let src = col
            .sources
            .values()
            .find(|s| s.name == input_name)
            .ok_or_else(|| format!("Input '{input_name}' not found"))?;
        Ok(src.audio.muted)
    }

    fn set_input_mute(&self, input_name: &str, muted: bool) -> Result<(), String> {
        let (sid, sname) = {
            let mut col = self.collection.write();
            let src = col
                .sources
                .values_mut()
                .find(|s| s.name == input_name)
                .ok_or_else(|| format!("Input '{input_name}' not found"))?;
            src.audio.muted = muted;
            (src.id, src.name.clone())
        };
        self.source_mgr.sync_collection(&self.collection.read());
        self.events.publish(EngineEvent::SourceMuteChanged {
            source_id: sid,
            name: sname,
            muted,
        });
        Ok(())
    }

    fn toggle_input_mute(&self, input_name: &str) -> Result<bool, String> {
        let now = !self.get_input_mute(input_name)?;
        self.set_input_mute(input_name, now)?;
        Ok(now)
    }

    fn start_record(&self) -> Result<(), String> {
        StudioBackend::start_recording(self)
    }

    fn stop_record(&self) -> Result<String, String> {
        StudioBackend::stop_recording(self).map(|p| p.to_string_lossy().to_string())
    }

    fn pause_record(&self) -> Result<(), String> {
        let g = self.recorder.lock();
        let r = g.as_ref().ok_or_else(|| "Not recording".to_string())?;
        r.pause();
        Ok(())
    }

    fn resume_record(&self) -> Result<(), String> {
        let g = self.recorder.lock();
        let r = g.as_ref().ok_or_else(|| "Not recording".to_string())?;
        r.resume();
        Ok(())
    }

    fn toggle_record_pause(&self) -> Result<bool, String> {
        self.pause_or_resume_recording()?;
        Ok(self
            .recorder
            .lock()
            .as_ref()
            .map(|r| r.is_paused())
            .unwrap_or(false))
    }

    fn start_stream(&self) -> Result<(), String> {
        StudioBackend::start_streaming(self)
    }

    fn stop_stream(&self) -> Result<(), String> {
        StudioBackend::stop_streaming(self)
    }

    fn start_replay_buffer(&self) -> Result<(), String> {
        StudioBackend::start_replay_buffer(self)
    }

    fn stop_replay_buffer(&self) -> Result<(), String> {
        StudioBackend::stop_replay_buffer(self)
    }

    fn save_replay_buffer(&self) -> Result<String, String> {
        StudioBackend::save_replay_buffer(self).map(|p| p.to_string_lossy().to_string())
    }

    fn list_profiles(&self) -> (String, Vec<String>) {
        (
            StudioBackend::active_profile(self),
            StudioBackend::list_profiles(self),
        )
    }

    fn set_current_profile(&self, name: &str) -> Result<(), String> {
        StudioBackend::switch_profile(self, name)
    }

    fn list_scene_collections(&self) -> (String, Vec<String>) {
        (
            StudioBackend::active_collection_name(self),
            StudioBackend::list_collections(self),
        )
    }

    fn set_current_scene_collection(&self, name: &str) -> Result<(), String> {
        StudioBackend::switch_collection(self, name)
    }

    fn list_hotkey_names(&self) -> Vec<String> {
        self.settings
            .read()
            .hotkeys
            .bindings
            .iter()
            .map(|b| b.action.clone())
            .collect()
    }

    fn trigger_hotkey(&self, action: &str) -> Result<(), String> {
        self.handle_hotkey_action(action)
    }
}

fn query_process_memory_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .is_ok()
        {
            return (pmc.WorkingSetSize as f64) / (1024.0 * 1024.0);
        }
    }
    0.0
}
