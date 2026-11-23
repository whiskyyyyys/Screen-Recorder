pub mod browser;
pub mod media_player;
pub mod text_render;

use browser::{BrowserSpec, BrowserWorker};
use image::{codecs::gif::GifDecoder, AnimationDecoder};
use media_player::{MediaSpec, MediaWorker};
use obs_audio::AudioMixer;
use obs_capture::CaptureManager;
use obs_core::{Clock, SceneId, SourceId};
use obs_scenes::{SceneCollection, SourceKind, SourceProperties};
use obs_video::VideoFrame;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use text_render::render_text_frame;

pub use text_render::list_system_fonts;

struct StaticImageCache {
    path: String,
    frames: Vec<(VideoFrame, u32)>,
    current_idx: usize,
    last_switch: Instant,
    seq: u64,
}

struct SlideshowState {
    files_key: Vec<String>,
    slide_time_ms: u32,
    randomize: bool,
    loop_slides: bool,
    paused: bool,
    idx: usize,
    last_advance: Instant,
    cached_frames: HashMap<String, VideoFrame>,
}

struct TextCache {
    signature: String,
    last_file_mtime: Option<SystemTime>,
    frame: VideoFrame,
}

struct ColorCache {
    color: [u8; 4],
    width: u32,
    height: u32,
    frame: VideoFrame,
}

/// Manages live frame generation and capture coordination for all OBS source types.
pub struct SourceManager {
    clock: Clock,
    capture: CaptureManager,
    audio_mixer: Arc<AudioMixer>,
    colors: Mutex<HashMap<SourceId, ColorCache>>,
    texts: Mutex<HashMap<SourceId, TextCache>>,
    images: Mutex<HashMap<SourceId, StaticImageCache>>,
    slideshows: Mutex<HashMap<SourceId, SlideshowState>>,
    media_workers: Mutex<HashMap<SourceId, MediaWorker>>,
    browser_workers: Mutex<HashMap<SourceId, BrowserWorker>>,
    plugin_frames: Mutex<HashMap<SourceId, VideoFrame>>,
    non_capture_errors: Mutex<HashMap<SourceId, String>>,
    seq_counter: Mutex<u64>,
}

impl SourceManager {
    pub fn new(clock: Clock, audio_mixer: Arc<AudioMixer>, target_fps: u32) -> Self {
        Self {
            capture: CaptureManager::new(clock.clone(), Arc::clone(&audio_mixer), target_fps),
            clock,
            audio_mixer,
            colors: Mutex::new(HashMap::new()),
            texts: Mutex::new(HashMap::new()),
            images: Mutex::new(HashMap::new()),
            slideshows: Mutex::new(HashMap::new()),
            media_workers: Mutex::new(HashMap::new()),
            browser_workers: Mutex::new(HashMap::new()),
            plugin_frames: Mutex::new(HashMap::new()),
            non_capture_errors: Mutex::new(HashMap::new()),
            seq_counter: Mutex::new(100),
        }
    }

    fn next_seq(&self) -> u64 {
        let mut s = self.seq_counter.lock();
        *s += 1;
        *s
    }

    pub fn set_target_fps(&self, fps: u32) {
        self.capture.set_target_fps(fps);
    }

    pub fn push_plugin_frame(&self, source_id: SourceId, frame: VideoFrame) {
        self.plugin_frames.lock().insert(source_id, frame);
    }

    /// Synchronizes background capture, media, browser, and audio mixer state with `collection`,
    /// gating audio capture and mixing to active program/transition scenes + enabled global devices.
    pub fn sync_collection(&self, collection: &SceneCollection) {
        self.sync_collection_with_transition(collection, None);
    }

    pub fn sync_collection_with_transition(
        &self,
        collection: &SceneCollection,
        transition_scenes: Option<(SceneId, SceneId)>,
    ) {
        let active_audio = collection.active_audio_source_ids(transition_scenes);
        self.capture
            .sync_with_collection_active(collection, &active_audio);

        // Sync audio mixer channels ONLY for active audio sources + enabled global audio devices
        let mut audio_desired = Vec::new();
        for (id, src) in &collection.sources {
            if src.kind.has_audio() && active_audio.contains(id) {
                audio_desired.push((*id, src.audio.clone(), src.audio_filters.clone()));
            }
        }
        self.audio_mixer.sync_sources(&audio_desired);

        // Sync MediaSource & PlaylistSource workers
        let mut desired_media = HashMap::new();
        let mut desired_browser = HashMap::new();
        for (id, src) in &collection.sources {
            match (&src.kind, &src.properties) {
                (
                    SourceKind::MediaSource,
                    SourceProperties::MediaSource {
                        local_file,
                        loop_playback,
                        speed_percent,
                        paused,
                        ..
                    },
                ) => {
                    let files = if local_file.trim().is_empty() {
                        Vec::new()
                    } else {
                        vec![local_file.clone()]
                    };
                    desired_media.insert(
                        *id,
                        MediaSpec {
                            files,
                            loop_playback: *loop_playback,
                            shuffle: false,
                            speed_percent: *speed_percent,
                            paused: *paused,
                        },
                    );
                }
                (
                    SourceKind::PlaylistSource,
                    SourceProperties::PlaylistSource {
                        files,
                        loop_playlist,
                        shuffle,
                        paused,
                    },
                ) => {
                    desired_media.insert(
                        *id,
                        MediaSpec {
                            files: files.clone(),
                            loop_playback: *loop_playlist,
                            shuffle: *shuffle,
                            speed_percent: 100,
                            paused: *paused,
                        },
                    );
                }
                (
                    SourceKind::BrowserSource,
                    SourceProperties::BrowserSource {
                        url,
                        width,
                        height,
                        fps,
                        custom_css,
                        ..
                    },
                ) => {
                    desired_browser.insert(
                        *id,
                        BrowserSpec {
                            url: url.clone(),
                            width: *width,
                            height: *height,
                            fps: *fps,
                            custom_css: custom_css.clone(),
                        },
                    );
                }
                _ => {}
            }
        }

        {
            let mut mw = self.media_workers.lock();
            mw.retain(|id, w| {
                desired_media
                    .get(id)
                    .map(|s| {
                        // Pause state can change without respawning worker
                        w.set_paused(s.paused);
                        w.spec.files == s.files
                            && w.spec.loop_playback == s.loop_playback
                            && w.spec.speed_percent == s.speed_percent
                    })
                    .unwrap_or(false)
            });
            for (id, spec) in desired_media {
                mw.entry(id).or_insert_with(|| {
                    MediaWorker::start(id, spec, self.clock.clone(), Arc::clone(&self.audio_mixer))
                });
            }
        }

        {
            let mut bw = self.browser_workers.lock();
            bw.retain(|id, w| {
                desired_browser
                    .get(id)
                    .map(|s| *s == w.spec)
                    .unwrap_or(false)
            });
            for (id, spec) in desired_browser {
                bw.entry(id)
                    .or_insert_with(|| BrowserWorker::start(id, spec, self.clock.clone()));
            }
        }
    }

    pub fn media_restart(&self, source_id: SourceId) {
        if let Some(w) = self.media_workers.lock().get(&source_id) {
            w.trigger_restart();
        }
    }

    pub fn media_next(&self, source_id: SourceId) {
        if let Some(w) = self.media_workers.lock().get(&source_id) {
            w.trigger_next();
        }
        if let Some(s) = self.slideshows.lock().get_mut(&source_id) {
            if !s.files_key.is_empty() {
                s.idx = (s.idx + 1) % s.files_key.len();
                s.last_advance = Instant::now();
            }
        }
    }

    pub fn slideshow_prev(&self, source_id: SourceId) {
        if let Some(s) = self.slideshows.lock().get_mut(&source_id) {
            if !s.files_key.is_empty() {
                s.idx = if s.idx == 0 {
                    s.files_key.len() - 1
                } else {
                    s.idx - 1
                };
                s.last_advance = Instant::now();
            }
        }
    }

    pub fn browser_refresh(&self, source_id: SourceId) {
        if let Some(b) = self.browser_workers.lock().get(&source_id) {
            b.trigger_refresh();
        }
    }

    pub fn collect_video_frames(
        &self,
        collection: &SceneCollection,
    ) -> HashMap<SourceId, VideoFrame> {
        let mut frames = self.capture.latest_video_frames();

        for (id, w) in self.media_workers.lock().iter() {
            if let Some(f) = w.latest_frame.read().as_ref() {
                frames.insert(*id, f.clone());
            }
        }
        for (id, b) in self.browser_workers.lock().iter() {
            if let Some(f) = b.latest_frame.read().as_ref() {
                frames.insert(*id, f.clone());
            }
        }
        for (id, pf) in self.plugin_frames.lock().iter() {
            frames.insert(*id, pf.clone());
        }

        for (id, src) in &collection.sources {
            match (&src.kind, &src.properties) {
                (
                    SourceKind::ColorSource,
                    SourceProperties::ColorSource {
                        color_rgba,
                        width,
                        height,
                    },
                ) => {
                    let w = (*width).clamp(8, 3840);
                    let h = (*height).clamp(8, 2160);
                    let mut map = self.colors.lock();
                    let need_regen = map
                        .get(id)
                        .map(|c| c.color != *color_rgba || c.width != w || c.height != h)
                        .unwrap_or(true);
                    if need_regen {
                        let frame = VideoFrame::new_solid_rgba(w, h, *color_rgba, self.next_seq());
                        map.insert(
                            *id,
                            ColorCache {
                                color: *color_rgba,
                                width: w,
                                height: h,
                                frame,
                            },
                        );
                    }
                    if let Some(c) = map.get(id) {
                        frames.insert(*id, c.frame.clone());
                    }
                }
                (
                    SourceKind::TextSource,
                    SourceProperties::TextSource {
                        text,
                        font_family,
                        font_size_px,
                        color_rgba,
                        gradient_enabled,
                        gradient_bottom_rgba,
                        outline_enabled,
                        outline_size,
                        outline_color_rgba,
                        drop_shadow,
                        align,
                        read_from_file,
                        file_path,
                    },
                ) => {
                    let mut active_text = text.clone();
                    let mut mtime = None;
                    if *read_from_file && !file_path.trim().is_empty() {
                        if let Ok(meta) = fs::metadata(file_path) {
                            mtime = meta.modified().ok();
                        }
                        if let Ok(content) = fs::read_to_string(file_path) {
                            active_text = content;
                        }
                    }
                    let sig = format!(
                        "{active_text}|{font_family}|{font_size_px}|{color_rgba:?}|{gradient_enabled}|{gradient_bottom_rgba:?}|{outline_enabled}|{outline_size}|{outline_color_rgba:?}|{drop_shadow}|{align}"
                    );
                    let mut map = self.texts.lock();
                    let need_regen = map
                        .get(id)
                        .map(|c| c.signature != sig || c.last_file_mtime != mtime)
                        .unwrap_or(true);
                    if need_regen {
                        let frame = render_text_frame(
                            &active_text,
                            font_family,
                            *font_size_px,
                            *color_rgba,
                            *gradient_enabled,
                            *gradient_bottom_rgba,
                            *outline_enabled,
                            *outline_size,
                            *outline_color_rgba,
                            *drop_shadow,
                            align,
                            self.next_seq(),
                        );
                        map.insert(
                            *id,
                            TextCache {
                                signature: sig,
                                last_file_mtime: mtime,
                                frame,
                            },
                        );
                    }
                    if let Some(c) = map.get(id) {
                        frames.insert(*id, c.frame.clone());
                    }
                }
                (SourceKind::Image, SourceProperties::Image { file_path, .. }) => {
                    if let Some(f) = self.get_or_load_image(*id, file_path) {
                        frames.insert(*id, f);
                    }
                }
                (
                    SourceKind::ImageSlideshow,
                    SourceProperties::ImageSlideshow {
                        files,
                        slide_time_ms,
                        loop_slides,
                        randomize,
                        paused,
                        ..
                    },
                ) => {
                    if let Some(f) = self.get_slideshow_frame(
                        *id,
                        files,
                        *slide_time_ms,
                        *loop_slides,
                        *randomize,
                        *paused,
                    ) {
                        frames.insert(*id, f);
                    }
                }
                _ => {}
            }
        }

        frames
    }

    pub fn source_errors(&self) -> HashMap<SourceId, String> {
        let mut errs = self.capture.worker_errors();
        for (id, e) in self.non_capture_errors.lock().iter() {
            errs.insert(*id, e.clone());
        }
        for (id, w) in self.media_workers.lock().iter() {
            if let Some(e) = w.last_error.lock().as_ref() {
                errs.insert(*id, e.clone());
            }
        }
        for (id, b) in self.browser_workers.lock().iter() {
            if let Some(e) = b.last_error.lock().as_ref() {
                errs.insert(*id, e.clone());
            }
        }
        errs
    }

    pub fn stop_all(&self) {
        self.capture.stop_all();
        self.media_workers.lock().clear();
        self.browser_workers.lock().clear();
    }

    fn get_or_load_image(&self, id: SourceId, path: &str) -> Option<VideoFrame> {
        if path.trim().is_empty() {
            return None;
        }
        let mut map = self.images.lock();
        let need_load = map.get(&id).map(|c| c.path != path).unwrap_or(true);
        if need_load {
            match load_image_frames(path, self.next_seq()) {
                Ok(frames) => {
                    self.non_capture_errors.lock().remove(&id);
                    map.insert(
                        id,
                        StaticImageCache {
                            path: path.to_string(),
                            frames,
                            current_idx: 0,
                            last_switch: Instant::now(),
                            seq: self.next_seq(),
                        },
                    );
                }
                Err(e) => {
                    self.non_capture_errors.lock().insert(id, e);
                    return None;
                }
            }
        }
        let cache = map.get_mut(&id)?;
        if cache.frames.len() > 1 {
            let delay_ms = cache.frames[cache.current_idx].1.max(20) as u64;
            if cache.last_switch.elapsed() >= Duration::from_millis(delay_ms) {
                cache.current_idx = (cache.current_idx + 1) % cache.frames.len();
                cache.last_switch = Instant::now();
                cache.seq += 1;
            }
        }
        let mut f = cache.frames.get(cache.current_idx)?.0.clone();
        f.seq = cache.seq;
        Some(f)
    }

    fn get_slideshow_frame(
        &self,
        id: SourceId,
        files: &[String],
        slide_time_ms: u32,
        loop_slides: bool,
        randomize: bool,
        paused: bool,
    ) -> Option<VideoFrame> {
        if files.is_empty() {
            return None;
        }
        let mut map = self.slideshows.lock();
        let state = map.entry(id).or_insert_with(|| SlideshowState {
            files_key: files.to_vec(),
            slide_time_ms,
            randomize,
            loop_slides,
            paused,
            idx: 0,
            last_advance: Instant::now(),
            cached_frames: HashMap::new(),
        });

        if state.files_key != files {
            state.files_key = files.to_vec();
            state.idx = 0;
            state.last_advance = Instant::now();
        }
        state.slide_time_ms = slide_time_ms;
        state.loop_slides = loop_slides;
        state.randomize = randomize;
        state.paused = paused;

        if !paused
            && files.len() > 1
            && state.last_advance.elapsed() >= Duration::from_millis(slide_time_ms.max(300) as u64)
        {
            if randomize {
                state.idx = (state.idx + 7) % files.len();
            } else if state.idx + 1 < files.len() {
                state.idx += 1;
            } else if loop_slides {
                state.idx = 0;
            }
            state.last_advance = Instant::now();
        }

        let current_path = files.get(state.idx)?.clone();
        if !state.cached_frames.contains_key(&current_path) {
            if let Ok(mut list) = load_image_frames(&current_path, self.next_seq()) {
                if let Some((f, _)) = list.pop() {
                    state.cached_frames.insert(current_path.clone(), f);
                }
            }
        }
        state.cached_frames.get(&current_path).cloned()
    }
}

fn load_image_frames(path: &str, base_seq: u64) -> Result<Vec<(VideoFrame, u32)>, String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("Image not found: {path}"));
    }
    if p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("gif"))
        .unwrap_or(false)
    {
        if let Ok(file) = File::open(p) {
            if let Ok(decoder) = GifDecoder::new(BufReader::new(file)) {
                let frames_res = decoder.into_frames().collect_frames();
                if let Ok(frames) = frames_res {
                    let mut out = Vec::with_capacity(frames.len());
                    for (i, frame) in frames.into_iter().enumerate() {
                        let (num, den) = frame.delay().numer_denom_ms();
                        let delay_ms = num.checked_div(den).unwrap_or(100).max(20);
                        let buf = frame.into_buffer();
                        let (w, h) = buf.dimensions();
                        out.push((
                            VideoFrame::new_rgba(w, h, buf.into_raw(), 0, base_seq + i as u64),
                            delay_ms,
                        ));
                    }
                    if !out.is_empty() {
                        return Ok(out);
                    }
                }
            }
        }
    }
    let img = image::open(p).map_err(|e| format!("Failed decoding image {path}: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(vec![(
        VideoFrame::new_rgba(w, h, rgba.into_raw(), 0, base_seq),
        100,
    )])
}
