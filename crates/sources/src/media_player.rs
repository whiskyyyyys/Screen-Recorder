use obs_audio::AudioMixer;
use obs_core::{Clock, SourceId};
use obs_video::VideoFrame;
use parking_lot::{Mutex, RwLock};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn find_media_tool(name: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join(name);
            if c.exists() {
                return Some(c);
            }
        }
    }
    let cwd_tools = PathBuf::from(".tools").join("ffmpeg").join(name);
    if cwd_tools.exists() {
        return Some(cwd_tools);
    }
    let cwd_bin = PathBuf::from("bin").join(name);
    if cwd_bin.exists() {
        return Some(cwd_bin);
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(name);
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSpec {
    pub files: Vec<String>,
    pub loop_playback: bool,
    pub shuffle: bool,
    pub speed_percent: u32,
    pub paused: bool,
}

pub struct MediaWorker {
    pub spec: MediaSpec,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    skip_next: Arc<AtomicBool>,
    restart_req: Arc<AtomicBool>,
    pub latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    pub last_error: Arc<Mutex<Option<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl MediaWorker {
    pub fn start(
        source_id: SourceId,
        spec: MediaSpec,
        clock: Clock,
        audio_mixer: Arc<AudioMixer>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(spec.paused));
        let skip_next = Arc::new(AtomicBool::new(false));
        let restart_req = Arc::new(AtomicBool::new(false));
        let latest_frame = Arc::new(RwLock::new(None));
        let last_error = Arc::new(Mutex::new(None));

        let r = Arc::clone(&running);
        let p = Arc::clone(&paused);
        let sn = Arc::clone(&skip_next);
        let rr = Arc::clone(&restart_req);
        let lf = Arc::clone(&latest_frame);
        let le = Arc::clone(&last_error);
        let files = spec.files.clone();
        let looping = spec.loop_playback;
        let speed = spec.speed_percent.clamp(25, 400);

        let handle = thread::Builder::new()
            .name(format!("obs-media-{}", &source_id.to_string()[..8]))
            .spawn(move || {
                media_loop(
                    source_id,
                    files,
                    looping,
                    speed,
                    r,
                    p,
                    sn,
                    rr,
                    lf,
                    le,
                    clock,
                    audio_mixer,
                );
            })
            .ok();

        Self {
            spec,
            running,
            paused,
            skip_next,
            restart_req,
            latest_frame,
            last_error,
            handle,
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn trigger_next(&self) {
        self.skip_next.store(true, Ordering::Relaxed);
    }

    pub fn trigger_restart(&self) {
        self.restart_req.store(true, Ordering::Relaxed);
    }
}

impl Drop for MediaWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn media_loop(
    source_id: SourceId,
    files: Vec<String>,
    looping: bool,
    speed_percent: u32,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    skip_next: Arc<AtomicBool>,
    restart_req: Arc<AtomicBool>,
    latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    last_error: Arc<Mutex<Option<String>>>,
    clock: Clock,
    audio_mixer: Arc<AudioMixer>,
) {
    let Some(ffmpeg) = find_media_tool("ffmpeg.exe") else {
        *last_error.lock() = Some("ffmpeg.exe not found for Media Source".to_string());
        return;
    };
    let Some(ffprobe) = find_media_tool("ffprobe.exe") else {
        *last_error.lock() = Some("ffprobe.exe not found for Media Source".to_string());
        return;
    };

    if files.is_empty() || files.iter().all(|f| f.trim().is_empty()) {
        return;
    }

    let mut idx = 0usize;
    let mut seq = 0u64;

    while running.load(Ordering::Relaxed) {
        if idx >= files.len() {
            if looping {
                idx = 0;
            } else {
                thread::sleep(Duration::from_millis(100));
                if restart_req.swap(false, Ordering::Relaxed) {
                    idx = 0;
                }
                continue;
            }
        }
        let target = &files[idx];
        if target.trim().is_empty() {
            idx += 1;
            continue;
        }
        if !target.contains("://") && !Path::new(target).exists() {
            *last_error.lock() = Some(format!("Media file not found: {target}"));
            thread::sleep(Duration::from_millis(400));
            idx += 1;
            continue;
        }

        let probe = probe_media_stream(&ffprobe, target);
        let (vw, vh, has_video, has_audio) = match probe {
            Ok(info) => info,
            Err(e) => {
                *last_error.lock() = Some(e);
                thread::sleep(Duration::from_millis(300));
                idx += 1;
                continue;
            }
        };

        *last_error.lock() = None;
        let speed_f = (speed_percent as f64) / 100.0;

        // Spawn audio decoder thread if audio stream exists
        let audio_stop = Arc::new(AtomicBool::new(false));
        let audio_handle = if has_audio {
            let ff_clone = ffmpeg.clone();
            let tgt_clone = target.clone();
            let r_clone = Arc::clone(&running);
            let p_clone = Arc::clone(&paused);
            let s_clone = Arc::clone(&audio_stop);
            let mixer_clone = Arc::clone(&audio_mixer);
            let sr = mixer_clone.sample_rate();
            Some(thread::spawn(move || {
                decode_audio_loop(
                    source_id,
                    &ff_clone,
                    &tgt_clone,
                    sr,
                    speed_f,
                    r_clone,
                    p_clone,
                    s_clone,
                    mixer_clone,
                );
            }))
        } else {
            None
        };

        if has_video && vw > 0 && vh > 0 {
            let out_w = vw.min(1920);
            let out_h = ((vh as f64) * (out_w as f64 / vw as f64)).round() as u32;
            let mut cmd = Command::new(&ffmpeg);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd.args(["-hide_banner", "-loglevel", "error", "-re", "-i", target]);
            if (speed_f - 1.0).abs() > 0.02 {
                cmd.args(["-filter:v", &format!("setpts={}*PTS", 1.0 / speed_f)]);
            }
            cmd.args([
                "-vf",
                &format!("scale={out_w}:{out_h}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-an",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

            if let Ok(mut child) = cmd.spawn() {
                if let Some(mut stdout) = child.stdout.take() {
                    let frame_bytes = (out_w as usize) * (out_h as usize) * 4;
                    let mut buf = vec![0u8; frame_bytes];
                    while running.load(Ordering::Relaxed)
                        && !skip_next.load(Ordering::Relaxed)
                        && !restart_req.load(Ordering::Relaxed)
                    {
                        while paused.load(Ordering::Relaxed)
                            && running.load(Ordering::Relaxed)
                            && !skip_next.load(Ordering::Relaxed)
                        {
                            thread::sleep(Duration::from_millis(30));
                        }
                        if stdout.read_exact(&mut buf).is_err() {
                            break;
                        }
                        seq += 1;
                        *latest_frame.write() = Some(VideoFrame::new_rgba(
                            out_w,
                            out_h,
                            buf.clone(),
                            clock.now_us(),
                            seq,
                        ));
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        } else {
            // Audio-only media item: keep alive while audio thread runs
            while running.load(Ordering::Relaxed)
                && !skip_next.load(Ordering::Relaxed)
                && !restart_req.load(Ordering::Relaxed)
            {
                if let Some(ah) = &audio_handle {
                    if ah.is_finished() {
                        break;
                    }
                } else {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }

        audio_stop.store(true, Ordering::Relaxed);
        if let Some(ah) = audio_handle {
            let _ = ah.join();
        }

        if restart_req.swap(false, Ordering::Relaxed) {
            idx = 0;
        } else {
            skip_next.store(false, Ordering::Relaxed);
            idx += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_audio_loop(
    source_id: SourceId,
    ffmpeg: &Path,
    target: &str,
    sample_rate: u32,
    speed: f64,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    mixer: Arc<AudioMixer>,
) {
    let mut cmd = Command::new(ffmpeg);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.args(["-hide_banner", "-loglevel", "error", "-re", "-i", target]);
    if (speed - 1.0).abs() > 0.02 {
        cmd.args(["-filter:a", &format!("atempo={}", speed.clamp(0.5, 2.0))]);
    }
    cmd.args([
        "-vn",
        "-f",
        "f32le",
        "-ac",
        "2",
        "-ar",
        &sample_rate.to_string(),
        "pipe:1",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    let Ok(mut child): std::io::Result<Child> = cmd.spawn() else {
        return;
    };
    if let Some(mut stdout) = child.stdout.take() {
        let chunk_samples = 1024 * 2;
        let mut raw = vec![0u8; chunk_samples * 4];
        while running.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            while paused.load(Ordering::Relaxed)
                && running.load(Ordering::Relaxed)
                && !stop.load(Ordering::Relaxed)
            {
                thread::sleep(Duration::from_millis(30));
            }
            if stdout.read_exact(&mut raw).is_err() {
                break;
            }
            let mut f32_buf = Vec::with_capacity(chunk_samples);
            for b in raw.as_chunks::<4>().0 {
                f32_buf.push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            }
            mixer.push_interleaved_stereo(source_id, &f32_buf, sample_rate);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn probe_media_stream(
    ffprobe: &Path,
    target: &str,
) -> std::result::Result<(u32, u32, bool, bool), String> {
    let mut cmd = Command::new(ffprobe);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,width,height",
            "-of",
            "json",
            target,
        ])
        .output()
        .map_err(|e| format!("ffprobe failed: {e}"))?;

    if !out.status.success() {
        return Err("ffprobe failed to open media source".to_string());
    }
    let val: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe JSON error: {e}"))?;
    let streams = val["streams"].as_array().cloned().unwrap_or_default();
    let mut w = 0u32;
    let mut h = 0u32;
    let mut has_v = false;
    let mut has_a = false;
    for s in streams {
        match s["codec_type"].as_str() {
            Some("video") => {
                has_v = true;
                w = s["width"].as_u64().unwrap_or(0) as u32;
                h = s["height"].as_u64().unwrap_or(0) as u32;
            }
            Some("audio") => {
                has_a = true;
            }
            _ => {}
        }
    }
    Ok((w, h, has_v, has_a))
}
