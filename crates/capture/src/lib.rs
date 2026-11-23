use obs_audio::AudioMixer;
use obs_core::{Clock, SourceId};
use obs_platform_windows::{
    AudioEndpointKind, DisplayCapturer, GameCapturer, WasapiCaptureStream, WebcamCapturer,
    WindowCapturer,
};
use obs_scenes::{SceneCollection, SourceKind, SourceProperties};
use obs_video::VideoFrame;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureSpec {
    Display {
        monitor_index: usize,
        capture_cursor: bool,
        capture_method: String,
        force_sdr: bool,
    },
    Window {
        title: String,
        process: String,
        capture_cursor: bool,
        client_area_only: bool,
        capture_method: String,
        match_priority: String,
    },
    Game {
        mode: String,
        title: String,
        process: String,
        capture_cursor: bool,
        capture_method: String,
        force_scaling: bool,
        scale_width: u32,
        scale_height: u32,
        force_sdr: bool,
    },
    Webcam {
        device_id: String,
        width: u32,
        height: u32,
        fps: u32,
    },
    AudioInput {
        device_id: String,
    },
    AudioOutput {
        device_id: String,
    },
    AppAudio {
        pid: u32,
    },
}

impl CaptureSpec {
    fn from_source(kind: SourceKind, props: &SourceProperties) -> Option<Self> {
        match (kind, props) {
            (
                SourceKind::DisplayCapture,
                SourceProperties::DisplayCapture {
                    monitor_index,
                    capture_cursor,
                    force_gdi,
                    capture_method,
                    force_sdr,
                },
            ) => {
                let effective_method = if *force_gdi && capture_method == "Auto" {
                    "GDI".to_string()
                } else {
                    capture_method.clone()
                };
                Some(Self::Display {
                    monitor_index: *monitor_index,
                    capture_cursor: *capture_cursor,
                    capture_method: effective_method,
                    force_sdr: *force_sdr,
                })
            }
            (
                SourceKind::WindowCapture,
                SourceProperties::WindowCapture {
                    window_title,
                    process_name,
                    capture_cursor,
                    client_area_only,
                    capture_method,
                    match_priority,
                    ..
                },
            ) => Some(Self::Window {
                title: window_title.clone(),
                process: process_name.clone(),
                capture_cursor: *capture_cursor,
                client_area_only: *client_area_only,
                capture_method: capture_method.clone(),
                match_priority: match_priority.clone(),
            }),
            (
                SourceKind::GameCapture,
                SourceProperties::GameCapture {
                    mode,
                    window_title,
                    process_name,
                    capture_cursor,
                    force_scaling,
                    capture_method,
                    scale_width,
                    scale_height,
                    force_sdr,
                    ..
                },
            ) => Some(Self::Game {
                mode: mode.clone(),
                title: window_title.clone(),
                process: process_name.clone(),
                capture_cursor: *capture_cursor,
                capture_method: capture_method.clone(),
                force_scaling: *force_scaling,
                scale_width: *scale_width,
                scale_height: *scale_height,
                force_sdr: *force_sdr,
            }),
            (
                SourceKind::VideoCaptureDevice,
                SourceProperties::VideoCaptureDevice {
                    device_id,
                    width,
                    height,
                    fps,
                    ..
                },
            ) => Some(Self::Webcam {
                device_id: device_id.clone(),
                width: *width,
                height: *height,
                fps: *fps,
            }),
            (
                SourceKind::AudioInputCapture,
                SourceProperties::AudioInputCapture { device_id, .. },
            ) => Some(Self::AudioInput {
                device_id: device_id.clone(),
            }),
            (
                SourceKind::AudioOutputCapture,
                SourceProperties::AudioOutputCapture { device_id, .. },
            ) => Some(Self::AudioOutput {
                device_id: device_id.clone(),
            }),
            (
                SourceKind::ApplicationAudioCapture,
                SourceProperties::ApplicationAudioCapture { pid, .. },
            ) => Some(Self::AppAudio { pid: *pid }),
            _ => None,
        }
    }
}

struct CaptureWorker {
    spec: CaptureSpec,
    running: Arc<AtomicBool>,
    latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    last_error: Arc<Mutex<Option<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[derive(Default)]
struct CaptureWorkers {
    active: HashMap<SourceId, CaptureWorker>,
    retiring: HashMap<SourceId, JoinHandle<()>>,
}

/// Manages active OS capture threads (DXGI Display, Window, Game, Webcam, WASAPI Input/Output/Process).
pub struct CaptureManager {
    clock: Clock,
    audio_mixer: Arc<AudioMixer>,
    workers: Mutex<CaptureWorkers>,
    target_fps: Arc<Mutex<u32>>,
}

impl CaptureManager {
    pub fn new(clock: Clock, audio_mixer: Arc<AudioMixer>, target_fps: u32) -> Self {
        Self {
            clock,
            audio_mixer,
            workers: Mutex::new(CaptureWorkers::default()),
            target_fps: Arc::new(Mutex::new(target_fps.clamp(10, 120))),
        }
    }

    pub fn set_target_fps(&self, fps: u32) {
        *self.target_fps.lock() = fps.clamp(10, 120);
    }

    pub fn sync_with_collection(&self, collection: &SceneCollection) {
        let active_audio = collection.active_audio_source_ids(None);
        self.sync_with_collection_active(collection, &active_audio);
    }

    pub fn sync_with_collection_active(
        &self,
        collection: &SceneCollection,
        active_audio: &HashSet<SourceId>,
    ) {
        let mut desired: HashMap<SourceId, CaptureSpec> = HashMap::new();
        for (id, src) in &collection.sources {
            if let Some(spec) = CaptureSpec::from_source(src.kind, &src.properties) {
                let is_audio_only = matches!(
                    spec,
                    CaptureSpec::AudioInput { .. }
                        | CaptureSpec::AudioOutput { .. }
                        | CaptureSpec::AppAudio { .. }
                );
                if is_audio_only && !active_audio.contains(id) {
                    continue;
                }
                desired.insert(*id, spec);
            }
        }

        let mut workers = self.workers.lock();
        let CaptureWorkers { active, retiring } = &mut *workers;
        retiring.retain(|_, handle| !handle.is_finished());
        active.retain(|id, worker| {
            if desired.get(id) == Some(&worker.spec) {
                return true;
            }
            worker.running.store(false, Ordering::Relaxed);
            if let Some(handle) = worker.handle.take() {
                retiring.insert(*id, handle);
            }
            false
        });

        for (id, spec) in desired {
            active
                .entry(id)
                .or_insert_with(|| self.spawn_worker(id, spec, retiring.remove(&id)));
        }
    }

    pub fn latest_video_frames(&self) -> HashMap<SourceId, VideoFrame> {
        let workers = self.workers.lock();
        let mut out = HashMap::new();
        for (id, w) in &workers.active {
            if let Some(frame) = w.latest_frame.read().as_ref() {
                out.insert(*id, frame.clone());
            }
        }
        out
    }

    pub fn worker_errors(&self) -> HashMap<SourceId, String> {
        let workers = self.workers.lock();
        let mut out = HashMap::new();
        for (id, w) in &workers.active {
            if let Some(err) = w.last_error.lock().as_ref() {
                out.insert(*id, err.clone());
            }
        }
        out
    }

    pub fn stop_all(&self) {
        let mut workers = self.workers.lock();
        workers.active.clear();
        for (_, handle) in workers.retiring.drain() {
            let _ = handle.join();
        }
    }

    fn spawn_worker(
        &self,
        source_id: SourceId,
        spec: CaptureSpec,
        previous: Option<JoinHandle<()>>,
    ) -> CaptureWorker {
        let running = Arc::new(AtomicBool::new(true));
        let latest_frame = Arc::new(RwLock::new(None));
        let last_error = Arc::new(Mutex::new(None));

        let r = Arc::clone(&running);
        let lf = Arc::clone(&latest_frame);
        let le = Arc::clone(&last_error);
        let clock = self.clock.clone();
        let mixer = Arc::clone(&self.audio_mixer);
        let fps_arc = Arc::clone(&self.target_fps);
        let spec_clone = spec.clone();

        let handle = thread::Builder::new()
            .name(format!("obs-cap-{}", &source_id.to_string()[..8]))
            .spawn(move || {
                // Release the old device before reopening it, off the UI thread.
                if let Some(handle) = previous {
                    let _ = handle.join();
                }
                if r.load(Ordering::Relaxed) {
                    run_worker_loop(source_id, spec_clone, r, lf, le, clock, mixer, fps_arc);
                }
            })
            .map_err(|error| {
                *last_error.lock() = Some(format!("Unable to start capture worker: {error}"));
            })
            .ok();

        CaptureWorker {
            spec,
            running,
            latest_frame,
            last_error,
            handle,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker_loop(
    source_id: SourceId,
    spec: CaptureSpec,
    running: Arc<AtomicBool>,
    latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    last_error: Arc<Mutex<Option<String>>>,
    clock: Clock,
    mixer: Arc<AudioMixer>,
    fps_arc: Arc<Mutex<u32>>,
) {
    match spec {
        CaptureSpec::Display {
            monitor_index,
            capture_cursor,
            capture_method,
            force_sdr,
        } => {
            let mut cap = match DisplayCapturer::new_with_options(
                monitor_index,
                capture_cursor,
                &capture_method,
                force_sdr,
            ) {
                Ok(c) => c,
                Err(e) => {
                    *last_error.lock() = Some(e.to_string());
                    return;
                }
            };
            while running.load(Ordering::Relaxed) {
                let fps = (*fps_arc.lock()).clamp(10, 120);
                let interval = Duration::from_millis((1000 / fps).max(4) as u64);
                let t0 = std::time::Instant::now();
                match cap.capture_frame(16, clock.now_us()) {
                    Ok(Some(frame)) => {
                        *latest_frame.write() = Some(frame);
                        *last_error.lock() = None;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *last_error.lock() = Some(e.to_string());
                    }
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        }
        CaptureSpec::Window {
            title,
            process,
            capture_cursor,
            client_area_only,
            capture_method,
            match_priority,
        } => {
            let mut cap = WindowCapturer::new_with_options(
                title,
                process,
                capture_cursor,
                client_area_only,
                capture_method,
                match_priority,
            );
            while running.load(Ordering::Relaxed) {
                let fps = (*fps_arc.lock()).clamp(10, 60);
                let interval = Duration::from_millis((1000 / fps).max(8) as u64);
                let t0 = std::time::Instant::now();
                match cap.capture_frame(clock.now_us()) {
                    Ok(Some(frame)) => {
                        *latest_frame.write() = Some(frame);
                        *last_error.lock() = None;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *last_error.lock() = Some(e.to_string());
                    }
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        }
        CaptureSpec::Game {
            mode,
            title,
            process,
            capture_cursor,
            capture_method,
            force_scaling,
            scale_width,
            scale_height,
            force_sdr,
        } => {
            let mut cap = GameCapturer::new_with_options(
                mode,
                title,
                process,
                capture_cursor,
                capture_method,
                force_scaling,
                scale_width,
                scale_height,
                force_sdr,
            );
            while running.load(Ordering::Relaxed) {
                let fps = (*fps_arc.lock()).clamp(10, 120);
                let interval = Duration::from_millis((1000 / fps).max(4) as u64);
                let t0 = std::time::Instant::now();
                match cap.capture_frame(clock.now_us()) {
                    Ok(Some(frame)) => {
                        *latest_frame.write() = Some(frame);
                        *last_error.lock() = None;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *last_error.lock() = Some(e.to_string());
                    }
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        }
        CaptureSpec::Webcam {
            device_id,
            width,
            height,
            fps,
        } => {
            let mut cap = match WebcamCapturer::open(&device_id, width, height, fps) {
                Ok(c) => c,
                Err(e) => {
                    *last_error.lock() = Some(e.to_string());
                    return;
                }
            };
            let interval = Duration::from_millis((1000 / fps.clamp(5, 60)) as u64);
            while running.load(Ordering::Relaxed) {
                let t0 = std::time::Instant::now();
                match cap.read_frame(clock.now_us()) {
                    Ok(Some(frame)) => {
                        *latest_frame.write() = Some(frame);
                        *last_error.lock() = None;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *last_error.lock() = Some(e.to_string());
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        }
        CaptureSpec::AudioInput { device_id } => {
            run_wasapi_loop(
                source_id,
                || WasapiCaptureStream::open_endpoint(&device_id, AudioEndpointKind::Input),
                running,
                last_error,
                mixer,
            );
        }
        CaptureSpec::AudioOutput { device_id } => {
            run_wasapi_loop(
                source_id,
                || WasapiCaptureStream::open_endpoint(&device_id, AudioEndpointKind::Output),
                running,
                last_error,
                mixer,
            );
        }
        CaptureSpec::AppAudio { pid } => {
            let sr = mixer.sample_rate();
            run_wasapi_loop(
                source_id,
                || WasapiCaptureStream::open_process_loopback(pid, sr),
                running,
                last_error,
                mixer,
            );
        }
    }
}

fn run_wasapi_loop<F>(
    source_id: SourceId,
    opener: F,
    running: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    mixer: Arc<AudioMixer>,
) where
    F: Fn() -> obs_core::Result<WasapiCaptureStream>,
{
    let stream = match opener() {
        Ok(s) => s,
        Err(e) => {
            *last_error.lock() = Some(e.to_string());
            return;
        }
    };
    *last_error.lock() = None;
    let sr = stream.sample_rate();
    while running.load(Ordering::Relaxed) {
        match stream.read_stereo_f32() {
            Ok(samples) => {
                if !samples.is_empty() {
                    mixer.push_interleaved_stereo(source_id, &samples, sr);
                }
            }
            Err(e) => {
                *last_error.lock() = Some(e.to_string());
                thread::sleep(Duration::from_millis(50));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}
