pub mod named_pipe;
pub mod remux;
pub mod replay;

pub use remux::remux_to_mp4;
pub use replay::{ReplayBufferConfig, ReplayBufferSession};

use crossbeam_channel::{bounded, Sender};
use named_pipe::{PipeCancelToken, WindowsNamedPipe};
use obs_core::{ObsError, RecordState, RecordingStats, Result};
use obs_encoder::{build_video_encoder_args, locate_ffmpeg, VideoEncodeParams};
use obs_platform_windows::HighResTimerGuard;
use obs_settings::RecordingFormat;
use obs_video::{ColorRange, ColorSpace};
use parking_lot::{Condvar, Mutex};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub type SharedVideoSlot = Arc<(Mutex<Option<Arc<Vec<u8>>>>, Condvar)>;

pub fn new_shared_video_slot() -> SharedVideoSlot {
    Arc::new((Mutex::new(None), Condvar::new()))
}

pub fn publish_shared_video_frame(slot: &SharedVideoSlot, frame: Arc<Vec<u8>>) {
    let mut guard = slot.0.lock();
    *guard = Some(frame);
    slot.1.notify_all();
}

/// Pumps latest NV12 frames into an FFmpeg rawvideo stdin pipe at strict monotonic wall-clock CFR,
/// duplicating the latest frame during UI/capture slowdowns so output playback stays 1.0x speed.
pub fn run_cfr_video_pipe_loop<W: Write>(
    mut sink: W,
    slot: SharedVideoSlot,
    running: Arc<AtomicBool>,
    paused: Option<Arc<AtomicBool>>,
    fps: u32,
    frames_written: Arc<AtomicU64>,
    frames_dropped: Arc<AtomicU64>,
) {
    let _timer = HighResTimerGuard::new(1);
    let frame_dur = Duration::from_secs_f64(1.0 / (fps.max(1) as f64));

    // Wait for the very first frame (or cancellation on early shutdown)
    let first_frame: Option<Arc<Vec<u8>>> = {
        let mut guard = slot.0.lock();
        while running.load(Ordering::Relaxed) {
            if paused.as_ref().is_some_and(|p| p.load(Ordering::Relaxed)) {
                let _ = slot.1.wait_for(&mut guard, Duration::from_millis(10));
                continue;
            }
            if guard.is_some() {
                break;
            }
            let _ = slot.1.wait_for(&mut guard, Duration::from_millis(10));
        }
        if running.load(Ordering::Relaxed) {
            guard.clone()
        } else {
            None
        }
    };

    let Some(f0) = first_frame else {
        let _ = sink.flush();
        return;
    };
    if sink.write_all(&f0).is_err() {
        frames_dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    frames_written.fetch_add(1, Ordering::Relaxed);

    let mut next_due = Instant::now() + frame_dur;
    let mut was_paused = false;

    while running.load(Ordering::Relaxed) {
        if paused.as_ref().is_some_and(|p| p.load(Ordering::Relaxed)) {
            was_paused = true;
            let mut guard = slot.0.lock();
            let _ = slot.1.wait_for(&mut guard, Duration::from_millis(10));
            continue;
        }
        if was_paused {
            was_paused = false;
            next_due = Instant::now() + frame_dur;
        }

        let frame_to_write: Option<Arc<Vec<u8>>> = {
            loop {
                if !running.load(Ordering::Relaxed) {
                    break None;
                }
                if paused.as_ref().is_some_and(|p| p.load(Ordering::Relaxed)) {
                    break None;
                }
                let now = Instant::now();
                if now >= next_due {
                    let guard = slot.0.lock();
                    break guard.clone();
                }
                let rem = next_due - now;
                if rem > Duration::from_millis(2) {
                    let mut guard = slot.0.lock();
                    let _ = slot.1.wait_for(&mut guard, rem - Duration::from_millis(2));
                } else {
                    thread::sleep(Duration::from_micros(400));
                }
            }
        };

        if !running.load(Ordering::Relaxed) {
            break;
        }
        let Some(frame) = frame_to_write else {
            continue;
        };

        let now = Instant::now();
        let behind = now.saturating_duration_since(next_due);
        let due_slots = 1u64 + (behind.as_secs_f64() / frame_dur.as_secs_f64()).floor() as u64;

        let emit_count = if behind > Duration::from_millis(500) {
            // Encoder stalled >500ms; credit missed slots and resync clock cleanly
            frames_dropped.fetch_add(due_slots.saturating_sub(1), Ordering::Relaxed);
            next_due = now + frame_dur;
            1u64
        } else {
            let capped = due_slots.min(4);
            if due_slots > capped {
                frames_dropped.fetch_add(due_slots - capped, Ordering::Relaxed);
            }
            next_due += Duration::from_secs_f64(frame_dur.as_secs_f64() * (due_slots as f64));
            capped
        };

        for _ in 0..emit_count {
            if sink.write_all(&frame).is_err() {
                frames_dropped.fetch_add(1, Ordering::Relaxed);
                running.store(false, Ordering::Relaxed);
                break;
            }
            frames_written.fetch_add(1, Ordering::Relaxed);
        }
    }

    let _ = sink.flush();
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct RecordingConfig {
    pub output_path: PathBuf,
    pub format: RecordingFormat,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub video_encoder: String,
    pub audio_encoder: String,
    pub rate_control: String,
    pub bitrate_kbps: u32,
    pub crf: u8,
    pub preset: String,
    pub keyint_sec: u32,
    pub audio_track_mask: u8,
    pub audio_bitrate_kbps: u32,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub auto_remux_to_mp4: bool,
}

pub fn expand_recording_filename(dir: &str, pattern: &str, format: RecordingFormat) -> PathBuf {
    let now = chrono::Local::now();
    let mut name = pattern.to_string();
    let replacements = [
        ("%CCYY", "%Y"),
        ("%YY", "%y"),
        ("%MM", "%m"),
        ("%DD", "%d"),
        ("%hh", "%H"),
        ("%mm", "%M"),
        ("%ss", "%S"),
    ];
    for (obs_tok, chrono_tok) in replacements {
        name = name.replace(obs_tok, chrono_tok);
    }
    let mut formatted = String::new();
    if std::fmt::write(&mut formatted, format_args!("{}", now.format(&name))).is_err() {
        formatted.clear();
    }
    let safe_name = if formatted.trim().is_empty() {
        now.format("%Y-%m-%d %H-%M-%S").to_string()
    } else {
        formatted
    };
    PathBuf::from(dir).join(format!("{safe_name}.{}", format.extension()))
}

pub struct RecordingSession {
    config: RecordingConfig,
    active_track_indices: Vec<usize>,
    video_slot: SharedVideoSlot,
    audio_txs: Vec<(usize, Sender<Vec<u8>>)>,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    state: Arc<Mutex<RecordState>>,
    start_instant: Instant,
    paused_accum: Mutex<Duration>,
    pause_started: Mutex<Option<Instant>>,
    encoded_frames: Arc<AtomicU64>,
    skipped_frames: Arc<AtomicU64>,
    child: Mutex<Option<Child>>,
    pipe_cancels: Vec<Arc<PipeCancelToken>>,
    threads: Vec<JoinHandle<()>>,
    stderr_log: Arc<Mutex<String>>,
}

impl RecordingSession {
    pub fn start(config: RecordingConfig) -> Result<Self> {
        validate_recording_format(config.format, config.audio_track_mask)?;
        if config.output_path.exists() {
            return Err(ObsError::Recorder(format!(
                "Recording already exists: {}. Choose a different filename.",
                config.output_path.display()
            )));
        }
        let ffmpeg = locate_ffmpeg()?;
        if let Some(parent) = config.output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut active_track_indices = Vec::new();
        for bit in 0..6usize {
            if (config.audio_track_mask & (1 << bit)) != 0 {
                active_track_indices.push(bit);
            }
        }
        if active_track_indices.is_empty() {
            active_track_indices.push(0);
        }

        let mut pipes = Vec::with_capacity(active_track_indices.len());
        let session_uuid = Uuid::new_v4();
        for &track_idx in &active_track_indices {
            let pipe =
                WindowsNamedPipe::create(&format!("obs-rust-rec-{session_uuid}-t{track_idx}"))?;
            pipes.push((track_idx, pipe));
        }

        let mut cmd = Command::new(&ffmpeg);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.args([
            "-n",
            "-hide_banner",
            "-loglevel",
            "error",
            "-probesize",
            "32",
            "-analyzeduration",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "nv12",
            "-s",
            &format!("{}x{}", config.width, config.height),
            "-r",
            &config.fps.max(1).to_string(),
            "-i",
            "pipe:0",
        ]);

        for (_, pipe) in &pipes {
            cmd.args([
                "-probesize",
                "32",
                "-analyzeduration",
                "1",
                "-f",
                "f32le",
                "-ar",
                &config.sample_rate.to_string(),
                "-ac",
                "2",
                "-i",
                &pipe.path,
            ]);
        }

        cmd.args(["-map", "0:v:0"]);
        for i in 0..pipes.len() {
            cmd.args(["-map", &format!("{}:a:0", i + 1)]);
        }

        let v_args = build_video_encoder_args(&VideoEncodeParams {
            encoder_id: config.video_encoder.clone(),
            bitrate_kbps: config.bitrate_kbps,
            rate_control: config.rate_control.clone(),
            crf: config.crf,
            preset: config.preset.clone(),
            keyint_sec: config.keyint_sec,
            fps: config.fps,
            color_space: config.color_space,
            color_range: config.color_range,
            streaming_low_latency: false,
        });
        cmd.args(v_args);

        let acodec = if config.audio_encoder == "libopus" {
            "libopus"
        } else {
            "aac"
        };
        cmd.args([
            "-c:a",
            acodec,
            "-b:a",
            &format!("{}k", config.audio_bitrate_kbps.clamp(64, 512)),
        ]);

        if config.format == RecordingFormat::FragmentedMp4 {
            cmd.args(["-movflags", "frag_keyframe+empty_moov+default_base_moof"]);
        } else if config.format == RecordingFormat::Mp4 || config.format == RecordingFormat::Mov {
            cmd.args(["-movflags", "+faststart"]);
        }

        cmd.args([
            "-f",
            config.format.ffmpeg_format(),
            &config.output_path.to_string_lossy(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ObsError::Recorder(format!("Failed spawning recorder FFmpeg: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ObsError::Recorder("Missing recorder stdin".into()))?;
        let stderr = child.stderr.take();

        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let encoded_frames = Arc::new(AtomicU64::new(0));
        let skipped_frames = Arc::new(AtomicU64::new(0));
        let stderr_log = Arc::new(Mutex::new(String::new()));
        let mut threads = Vec::new();

        if let Some(mut err_pipe) = stderr {
            let log_arc = Arc::clone(&stderr_log);
            threads.push(
                thread::Builder::new()
                    .name("obs-rec-stderr".into())
                    .spawn(move || {
                        use std::io::Read;
                        let mut s = String::new();
                        let _ = err_pipe.read_to_string(&mut s);
                        if !s.trim().is_empty() {
                            *log_arc.lock() = s;
                        }
                    })
                    .map_err(ObsError::Io)?,
            );
        }

        let video_slot = new_shared_video_slot();
        let slot_vid = Arc::clone(&video_slot);
        let r_vid = Arc::clone(&running);
        let p_vid = Arc::clone(&paused);
        let enc_frames = Arc::clone(&encoded_frames);
        let skip_frames = Arc::clone(&skipped_frames);
        let fps = config.fps.max(1);
        threads.push(
            thread::Builder::new()
                .name("obs-rec-vid".into())
                .spawn(move || {
                    run_cfr_video_pipe_loop(
                        stdin,
                        slot_vid,
                        r_vid,
                        Some(p_vid),
                        fps,
                        enc_frames,
                        skip_frames,
                    );
                })
                .map_err(ObsError::Io)?,
        );

        let mut audio_txs = Vec::with_capacity(pipes.len());
        let mut pipe_cancels = Vec::with_capacity(pipes.len());
        for (track_idx, mut pipe) in pipes {
            pipe_cancels.push(pipe.cancel_token());
            let (tx, rx) = bounded::<Vec<u8>>(128);
            let r_aud = Arc::clone(&running);
            threads.push(
                thread::Builder::new()
                    .name(format!("obs-rec-aud-{track_idx}"))
                    .spawn(move || {
                        if pipe.connect().is_err() {
                            return;
                        }
                        while let Ok(bytes) = rx.recv() {
                            if pipe.write_all(&bytes).is_err() {
                                break;
                            }
                            if !r_aud.load(Ordering::Relaxed) && rx.is_empty() {
                                break;
                            }
                        }
                        pipe.close();
                    })
                    .map_err(ObsError::Io)?,
            );
            audio_txs.push((track_idx, tx));
        }

        Ok(Self {
            config,
            active_track_indices,
            video_slot,
            audio_txs,
            running,
            paused,
            state: Arc::new(Mutex::new(RecordState::Recording)),
            start_instant: Instant::now(),
            paused_accum: Mutex::new(Duration::ZERO),
            pause_started: Mutex::new(None),
            encoded_frames,
            skipped_frames,
            child: Mutex::new(Some(child)),
            pipe_cancels,
            threads,
            stderr_log,
        })
    }

    pub fn push_video_nv12(&self, nv12: &[u8]) {
        self.push_video_nv12_shared(Arc::new(nv12.to_vec()));
    }

    pub fn push_video_nv12_shared(&self, nv12: Arc<Vec<u8>>) {
        if self.paused.load(Ordering::Relaxed) || !self.running.load(Ordering::Relaxed) {
            return;
        }
        publish_shared_video_frame(&self.video_slot, nv12);
    }

    pub fn push_audio_tracks(&self, tracks: &[Vec<f32>; 6]) {
        if self.paused.load(Ordering::Relaxed) || !self.running.load(Ordering::Relaxed) {
            return;
        }
        for (track_idx, tx) in &self.audio_txs {
            if let Some(samples) = tracks.get(*track_idx) {
                let mut bytes = Vec::with_capacity(samples.len() * 4);
                for &s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let _ = tx.try_send(bytes);
            }
        }
    }

    pub fn pause(&self) {
        if !self.paused.swap(true, Ordering::SeqCst) {
            *self.pause_started.lock() = Some(Instant::now());
            *self.state.lock() = RecordState::Paused;
            self.video_slot.1.notify_all();
        }
    }

    pub fn resume(&self) {
        if self.paused.swap(false, Ordering::SeqCst) {
            if let Some(t) = self.pause_started.lock().take() {
                *self.paused_accum.lock() += t.elapsed();
            }
            *self.state.lock() = RecordState::Recording;
            self.video_slot.1.notify_all();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn active_tracks(&self) -> &[usize] {
        &self.active_track_indices
    }

    pub fn stats(&self) -> RecordingStats {
        let mut paused_dur = *self.paused_accum.lock();
        if let Some(p) = *self.pause_started.lock() {
            paused_dur += p.elapsed();
        }
        let active_dur = self.start_instant.elapsed().saturating_sub(paused_dur);
        let bytes_written = fs::metadata(&self.config.output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let secs = active_dur.as_secs_f64().max(0.001);
        let bitrate_kbps = ((bytes_written as f64) * 8.0 / 1000.0) / secs;

        RecordingStats {
            state: *self.state.lock(),
            output_path: self.config.output_path.to_string_lossy().to_string(),
            duration_ms: active_dur.as_millis() as u64,
            bytes_written,
            encoded_frames: self.encoded_frames.load(Ordering::Relaxed),
            skipped_frames: self.skipped_frames.load(Ordering::Relaxed),
            bitrate_kbps,
            encoder_name: self.config.video_encoder.clone(),
            format_name: self.config.format.display_name().to_string(),
        }
    }

    pub fn stop(mut self) -> Result<PathBuf> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<PathBuf> {
        *self.state.lock() = RecordState::Stopping;
        self.running.store(false, Ordering::SeqCst);
        self.video_slot.1.notify_all();
        self.audio_txs.clear();

        let mut child_status: Option<std::process::ExitStatus> = None;
        if let Some(mut child) = self.child.lock().take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(st)) => {
                        child_status = Some(st);
                        break;
                    }
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) | Err(_) => {
                        for c in &self.pipe_cancels {
                            c.cancel();
                        }
                        let _ = child.kill();
                        child_status = child.wait().ok();
                        break;
                    }
                }
            }
        }

        for c in &self.pipe_cancels {
            c.cancel();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }

        *self.state.lock() = RecordState::Stopped;
        if let Some(status) = child_status {
            if !status.success() {
                let err = self.stderr_log.lock().clone();
                return Err(ObsError::Recorder(format!(
                    "FFmpeg recorder exited with {status}: {err}"
                )));
            }
        }

        let out_path = self.config.output_path.clone();
        if self.config.auto_remux_to_mp4 && self.config.format == RecordingFormat::Mkv {
            let mp4 = remux_to_mp4(&out_path, None)?;
            return Ok(mp4);
        }
        Ok(out_path)
    }
}

fn validate_recording_format(format: RecordingFormat, mask: u8) -> Result<()> {
    if format == RecordingFormat::Flv && (mask & 0x3f).count_ones() > 1 {
        return Err(ObsError::InvalidArgument(
            "FLV supports only one audio track. Select one track or use MKV/MP4.".into(),
        ));
    }
    Ok(())
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        if self.running.load(Ordering::Relaxed) {
            let _ = self.stop_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flv_rejects_multiple_audio_tracks() {
        assert!(validate_recording_format(RecordingFormat::Flv, 3).is_err());
        assert!(validate_recording_format(RecordingFormat::Flv, 2).is_ok());
        assert!(validate_recording_format(RecordingFormat::Mkv, 3).is_ok());
    }

    #[test]
    fn invalid_filename_pattern_does_not_panic() {
        let path = expand_recording_filename(".", "%Q", RecordingFormat::Mkv);
        assert_eq!(path.extension().and_then(|s| s.to_str()), Some("mkv"));
        assert!(!path.file_stem().unwrap().is_empty());
    }

    #[test]
    fn named_pipe_connect_unblocks_on_cancel() {
        let mut pipe =
            WindowsNamedPipe::create(&format!("obs-rust-cancel-test-{}", Uuid::new_v4())).unwrap();
        let cancel = pipe.cancel_token();
        let t0 = Instant::now();
        let j = thread::spawn(move || pipe.connect());
        thread::sleep(Duration::from_millis(40));
        cancel.cancel();
        let res = j.join().expect("thread panicked");
        assert!(res.is_err(), "Cancelled connect should return Err");
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "Cancelled connect took too long: {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn recording_stop_does_not_hang_when_ffmpeg_fails_early() {
        let tmp = std::env::temp_dir().join(format!("obs_rust_fail_stop_{}.mkv", Uuid::new_v4()));
        let _ = fs::remove_file(&tmp);
        let t0 = Instant::now();
        let sess = RecordingSession::start(RecordingConfig {
            output_path: tmp.clone(),
            format: RecordingFormat::Mkv,
            width: 320,
            height: 240,
            fps: 30,
            sample_rate: 48000,
            video_encoder: "nonexistent_encoder_xyz".to_string(),
            audio_encoder: "aac".to_string(),
            rate_control: "CBR".to_string(),
            bitrate_kbps: 1500,
            crf: 20,
            preset: "speed".to_string(),
            keyint_sec: 1,
            audio_track_mask: 0b0000_0011,
            audio_bitrate_kbps: 128,
            color_space: ColorSpace::Rec709,
            color_range: ColorRange::Partial,
            auto_remux_to_mp4: false,
        });
        if let Ok(session) = sess {
            thread::sleep(Duration::from_millis(80));
            let res = session.stop();
            assert!(
                res.is_err(),
                "Expected invalid encoder session stop to return Err"
            );
        }
        let _ = fs::remove_file(&tmp);
        assert!(
            t0.elapsed() < Duration::from_secs(6),
            "Early-failed FFmpeg stop hung: {:?}",
            t0.elapsed()
        );
    }
}
