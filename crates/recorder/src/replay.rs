use crate::named_pipe::{PipeCancelToken, WindowsNamedPipe};
use crossbeam_channel::{bounded, Sender};
use obs_core::{ObsError, ReplayBufferState, ReplayStats, Result};
use obs_encoder::{build_video_encoder_args, locate_ffmpeg, VideoEncodeParams};
use obs_video::{ColorRange, ColorSpace};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const CLUSTER_ID: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];

#[derive(Clone)]
struct ReplayCluster {
    bytes: Vec<u8>,
    duration_sec: f64,
}

struct RingStore {
    header: Vec<u8>,
    clusters: VecDeque<ReplayCluster>,
    total_bytes: u64,
    total_seconds: f64,
}

pub struct ReplayBufferConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub encoder_id: String,
    pub bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub max_duration_sec: u32,
    pub max_memory_mb: u32,
}

pub struct ReplayBufferSession {
    video_tx: Sender<Vec<u8>>,
    audio_tx: Sender<Vec<u8>>,
    running: Arc<AtomicBool>,
    ring: Arc<Mutex<RingStore>>,
    state: Arc<Mutex<ReplayBufferState>>,
    last_saved: Arc<Mutex<Option<String>>>,
    ffmpeg_path: PathBuf,
    child: Arc<Mutex<Option<Child>>>,
    pipe_cancel: Arc<PipeCancelToken>,
    threads: Vec<JoinHandle<()>>,
}

impl ReplayBufferSession {
    pub fn start(cfg: ReplayBufferConfig) -> Result<Self> {
        let ffmpeg = locate_ffmpeg()?;
        let pipe_name = format!("obs-rust-replay-{}", Uuid::new_v4());
        let mut pipe = WindowsNamedPipe::create(&pipe_name)?;
        let pipe_cancel = pipe.cancel_token();
        let pipe_path = pipe.path.clone();

        let (audio_tx, audio_rx) = bounded::<Vec<u8>>(128);
        let (video_tx, video_rx) = bounded::<Vec<u8>>(8);
        let running = Arc::new(AtomicBool::new(true));

        let mut cmd = Command::new(&ffmpeg);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "nv12",
            "-s",
            &format!("{}x{}", cfg.width, cfg.height),
            "-r",
            &cfg.fps.max(1).to_string(),
            "-i",
            "pipe:0",
            "-f",
            "f32le",
            "-ar",
            &cfg.sample_rate.to_string(),
            "-ac",
            "2",
            "-i",
            &pipe_path,
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
        ]);

        let v_args = build_video_encoder_args(&VideoEncodeParams {
            encoder_id: cfg.encoder_id.clone(),
            bitrate_kbps: cfg.bitrate_kbps,
            rate_control: "CBR".to_string(),
            crf: 18,
            preset: "speed".to_string(),
            keyint_sec: 1,
            fps: cfg.fps,
            color_space: cfg.color_space,
            color_range: cfg.color_range,
            streaming_low_latency: true,
        });
        cmd.args(v_args);
        cmd.args([
            "-c:a",
            "aac",
            "-b:a",
            &format!("{}k", cfg.audio_bitrate_kbps.clamp(64, 320)),
            "-f",
            "matroska",
            "-live",
            "1",
            "-cluster_time_limit",
            "1000",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| ObsError::Recorder(format!("Failed spawning Replay FFmpeg: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ObsError::Recorder("Missing replay stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ObsError::Recorder("Missing replay stdout".into()))?;

        let mut threads = Vec::new();

        // Video writer thread
        let r_vid = Arc::clone(&running);
        threads.push(
            thread::Builder::new()
                .name("obs-replay-vid".into())
                .spawn(move || {
                    while r_vid.load(Ordering::Relaxed) {
                        match video_rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(frame) => {
                                if stdin.write_all(&frame).is_err() {
                                    break;
                                }
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    let _ = stdin.flush();
                })
                .map_err(ObsError::Io)?,
        );

        // Audio named pipe writer thread
        let r_aud = Arc::clone(&running);
        threads.push(
            thread::Builder::new()
                .name("obs-replay-aud".into())
                .spawn(move || {
                    if pipe.connect().is_err() {
                        return;
                    }
                    while r_aud.load(Ordering::Relaxed) {
                        match audio_rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(bytes) => {
                                if pipe.write_all(&bytes).is_err() {
                                    break;
                                }
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    pipe.close();
                })
                .map_err(ObsError::Io)?,
        );

        // Matroska EBML Cluster ring parser thread
        let ring = Arc::new(Mutex::new(RingStore {
            header: Vec::new(),
            clusters: VecDeque::new(),
            total_bytes: 0,
            total_seconds: 0.0,
        }));
        let ring_clone = Arc::clone(&ring);
        let r_rd = Arc::clone(&running);
        let max_sec = cfg.max_duration_sec.max(5) as f64;
        let max_bytes = (cfg.max_memory_mb.max(32) as u64) * 1024 * 1024;
        threads.push(
            thread::Builder::new()
                .name("obs-replay-ebml".into())
                .spawn(move || {
                    read_ebml_clusters(stdout, ring_clone, r_rd, max_sec, max_bytes);
                })
                .map_err(ObsError::Io)?,
        );

        Ok(Self {
            video_tx,
            audio_tx,
            running,
            ring,
            state: Arc::new(Mutex::new(ReplayBufferState::Active)),
            last_saved: Arc::new(Mutex::new(None)),
            ffmpeg_path: ffmpeg,
            child: Arc::new(Mutex::new(Some(child))),
            pipe_cancel,
            threads,
        })
    }

    pub fn push_video_nv12(&self, nv12: &[u8]) {
        let _ = self.video_tx.try_send(nv12.to_vec());
    }

    pub fn push_audio_f32(&self, stereo_f32: &[f32]) {
        if stereo_f32.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(stereo_f32.len() * 4);
        for &s in stereo_f32 {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let _ = self.audio_tx.try_send(bytes);
    }

    pub fn stats(&self) -> ReplayStats {
        let r = self.ring.lock();
        ReplayStats {
            state: *self.state.lock(),
            buffered_seconds: r.total_seconds,
            buffered_bytes: r.total_bytes,
            cluster_count: r.clusters.len(),
            last_saved_path: self.last_saved.lock().clone(),
        }
    }

    pub fn save_replay(&self, out_path: &Path) -> Result<PathBuf> {
        struct SavingStateGuard<'a> {
            state: &'a Mutex<ReplayBufferState>,
            running: &'a AtomicBool,
            tmp_path: Option<PathBuf>,
        }
        impl Drop for SavingStateGuard<'_> {
            fn drop(&mut self) {
                if let Some(p) = &self.tmp_path {
                    let _ = fs::remove_file(p);
                }
                let next = if self.running.load(Ordering::Relaxed) {
                    ReplayBufferState::Active
                } else {
                    ReplayBufferState::Stopped
                };
                *self.state.lock() = next;
            }
        }

        *self.state.lock() = ReplayBufferState::Saving;
        let tmp_mkv = out_path.with_extension("replay_tmp.mkv");
        let _guard = SavingStateGuard {
            state: &self.state,
            running: &self.running,
            tmp_path: Some(tmp_mkv.clone()),
        };

        let (header, clusters): (Vec<u8>, Vec<ReplayCluster>) = {
            let r = self.ring.lock();
            if r.header.is_empty() || r.clusters.is_empty() {
                return Err(ObsError::Recorder(
                    "Replay buffer does not yet have enough buffered video clusters".to_string(),
                ));
            }
            (r.header.clone(), r.clusters.iter().cloned().collect())
        };

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        {
            let mut f = File::create(&tmp_mkv)?;
            f.write_all(&header)?;
            for c in &clusters {
                f.write_all(&c.bytes)?;
            }
            f.flush()?;
        }

        // Rebase timestamps cleanly from 0 using FFmpeg stream copy
        let mut cmd = Command::new(&self.ffmpeg_path);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                &tmp_mkv.to_string_lossy(),
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                &out_path.to_string_lossy(),
            ])
            .output()?;

        if !out.status.success() {
            return Err(ObsError::Recorder(format!(
                "Failed saving replay: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let saved_str = out_path.to_string_lossy().to_string();
        *self.last_saved.lock() = Some(saved_str);
        Ok(out_path.to_path_buf())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.pipe_cancel.cancel();
        *self.state.lock() = ReplayBufferState::Stopped;
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for ReplayBufferSession {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_ebml_clusters(
    mut stdout: std::process::ChildStdout,
    ring: Arc<Mutex<RingStore>>,
    running: Arc<AtomicBool>,
    max_seconds: f64,
    max_bytes: u64,
) {
    let mut acc: Vec<u8> = Vec::with_capacity(512 * 1024);
    let mut chunk = [0u8; 32768];
    let mut header_captured = false;
    let mut cluster_start_time = Instant::now();

    while running.load(Ordering::Relaxed) {
        let n = match stdout.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        acc.extend_from_slice(&chunk[..n]);

        if !header_captured {
            if let Some(pos) = find_cluster_marker(&acc, 0) {
                let hdr = acc[..pos].to_vec();
                acc.drain(..pos);
                ring.lock().header = hdr;
                header_captured = true;
                cluster_start_time = Instant::now();
            }
            continue;
        }

        // Look for the next Cluster header starting after offset 16
        while acc.len() > 16 {
            let Some(next_pos) = find_cluster_marker(&acc, 16) else {
                break;
            };
            // Verify byte right after CLUSTER_ID looks like a valid EBML VINT length
            // or Timecode element 0xE7 follows near the header
            let cluster_bytes = acc[..next_pos].to_vec();
            acc.drain(..next_pos);

            let dur = cluster_start_time.elapsed().as_secs_f64().clamp(0.2, 2.0);
            cluster_start_time = Instant::now();

            let mut r = ring.lock();
            r.total_bytes += cluster_bytes.len() as u64;
            r.total_seconds += dur;
            r.clusters.push_back(ReplayCluster {
                bytes: cluster_bytes,
                duration_sec: dur,
            });
            while (r.total_seconds > max_seconds || r.total_bytes > max_bytes)
                && r.clusters.len() > 1
            {
                if let Some(old) = r.clusters.pop_front() {
                    r.total_bytes = r.total_bytes.saturating_sub(old.bytes.len() as u64);
                    r.total_seconds = (r.total_seconds - old.duration_sec).max(0.0);
                }
            }
        }
    }
}

fn find_cluster_marker(buf: &[u8], start: usize) -> Option<usize> {
    if buf.len() < start + 6 {
        return None;
    }
    for i in start..=(buf.len() - 6) {
        if buf[i..i + 4] == CLUSTER_ID {
            // Confirm VINT size byte is valid and Timecode tag (0xE7) appears within first 14 bytes
            if buf[i + 4] != 0 {
                let scan_end = (i + 16).min(buf.len());
                if buf[i + 4..scan_end].contains(&0xE7) {
                    return Some(i);
                }
            }
        }
    }
    None
}
