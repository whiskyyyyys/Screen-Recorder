pub mod rtmp_client;
pub mod services;

pub use rtmp_client::{RtmpClient, RtmpPublishTarget};
pub use services::{
    load_streaming_services, ServiceRecommended, ServiceServer, StreamingServiceEntry,
};

use bytes::Bytes;
use crossbeam_channel::{bounded, Sender};
use obs_core::{ObsError, Result, StreamState, StreamingStats};
use obs_encoder::{build_video_encoder_args, locate_ffmpeg, VideoEncodeParams};
use obs_recorder::named_pipe::{PipeCancelToken, WindowsNamedPipe};
use obs_settings::{ExtraOutputConfig, StreamProtocol};
use obs_video::{ColorRange, ColorSpace};
use parking_lot::Mutex;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct StreamSessionConfig {
    pub service_name: String,
    pub server_url: String,
    pub stream_key: String,
    pub protocol: StreamProtocol,
    pub srt_latency_ms: u32,
    pub srt_passphrase: String,
    pub extra_outputs: Vec<ExtraOutputConfig>,

    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub video_encoder: String,
    pub audio_encoder: String,
    pub bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub keyint_sec: u32,
    pub preset: String,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,

    pub auto_reconnect: bool,
    pub reconnect_delay_sec: u32,
    pub max_retries: u32,
    pub dynamic_bitrate: bool,
}

/// AIMD congestion controller that lowers target encoding bitrate on frame drops / slow network sends
/// and gradually recovers toward the configured base bitrate when the connection stabilizes.
#[derive(Debug, Clone)]
pub struct DynamicBitrateController {
    pub enabled: bool,
    pub base_kbps: u32,
    pub min_kbps: u32,
    pub current_kbps: u32,
    pub stable_windows: u32,
}

impl DynamicBitrateController {
    pub fn new(enabled: bool, base_kbps: u32) -> Self {
        let base = base_kbps.max(500);
        let min = (base / 4).max(500);
        Self {
            enabled,
            base_kbps: base,
            min_kbps: min,
            current_kbps: base,
            stable_windows: 0,
        }
    }

    /// Evaluates a 2-second monitoring window; returns `Some(new_kbps)` when target bitrate changed.
    pub fn evaluate(&mut self, dropped_delta: u64, avg_send_ms: f64) -> Option<u32> {
        if !self.enabled {
            return None;
        }
        if dropped_delta >= 5 || avg_send_ms > 200.0 {
            self.stable_windows = 0;
            let next = ((self.current_kbps * 3) / 4).max(self.min_kbps);
            if next < self.current_kbps {
                self.current_kbps = next;
                return Some(next);
            }
            return None;
        }
        if dropped_delta == 0 && avg_send_ms < 80.0 && self.current_kbps < self.base_kbps {
            self.stable_windows += 1;
            if self.stable_windows >= 5 {
                self.stable_windows = 0;
                let step = (self.current_kbps / 10).max(100);
                let next = (self.current_kbps + step).min(self.base_kbps);
                if next > self.current_kbps {
                    self.current_kbps = next;
                    return Some(next);
                }
            }
        } else {
            self.stable_windows = 0;
        }
        None
    }
}

#[derive(Clone)]
struct FlvPacket {
    tag_type: u8,
    timestamp_ms: u32,
    payload: Bytes,
}

struct SpawnedEncoder {
    proc: EncoderProcess,
    video_tx: Sender<Vec<u8>>,
    audio_tx: Sender<Vec<u8>>,
}

struct EncoderProcess {
    running: Arc<AtomicBool>,
    pipe_cancel: Arc<PipeCancelToken>,
    child: Option<Child>,
    threads: Vec<JoinHandle<()>>,
}

impl EncoderProcess {
    fn stop_clean(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.pipe_cancel.cancel();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

pub struct StreamSession {
    config: StreamSessionConfig,
    video_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    audio_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<StreamState>>,
    bytes_sent: Arc<AtomicU64>,
    total_frames: Arc<AtomicU64>,
    dropped_frames: Arc<AtomicU64>,
    reconnect_attempts: Arc<AtomicU32>,
    dynamic_bitrate_kbps: Arc<AtomicU32>,
    rtt_ms_bits: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    active_extra_outputs: usize,
    start_instant: Instant,
    encoder_proc: Arc<Mutex<Option<EncoderProcess>>>,
    out_senders: Vec<tokio::sync::mpsc::Sender<FlvPacket>>,
    supervisor_thread: Option<JoinHandle<()>>,
}

impl StreamSession {
    pub fn start(
        config: StreamSessionConfig,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        let ffmpeg = locate_ffmpeg()?;

        let active_extras: Vec<ExtraOutputConfig> = config
            .extra_outputs
            .iter()
            .filter(|e| e.enabled && !e.url.trim().is_empty())
            .cloned()
            .collect();
        let active_extra_outputs = active_extras.len();

        let running = Arc::new(AtomicBool::new(true));
        let state = Arc::new(Mutex::new(StreamState::Connecting));
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let total_frames = Arc::new(AtomicU64::new(0));
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let reconnect_attempts = Arc::new(AtomicU32::new(0));
        let dynamic_bitrate_kbps = Arc::new(AtomicU32::new(config.bitrate_kbps));
        let rtt_ms_bits = Arc::new(AtomicU64::new(0.0f64.to_bits()));
        let last_error = Arc::new(Mutex::new(None));
        let timestamp_offset_ms = Arc::new(AtomicU32::new(0));
        let last_flv_ts_ms = Arc::new(AtomicU32::new(0));

        let use_srt_direct = config.protocol == StreamProtocol::Srt;
        let mut out_senders = Vec::new();

        if use_srt_direct {
            *state.lock() = StreamState::Streaming;
        } else {
            let mut targets = vec![(config.server_url.clone(), config.stream_key.clone(), true)];
            for extra in active_extras {
                targets.push((extra.url, extra.stream_key, false));
            }

            for (idx, (url, key, is_primary)) in targets.into_iter().enumerate() {
                let target = RtmpPublishTarget::parse(&url, &key)?;
                let (ptx, mut prx) = tokio::sync::mpsc::channel::<FlvPacket>(256);
                out_senders.push(ptx);

                let r_pub = Arc::clone(&running);
                let st_pub = Arc::clone(&state);
                let bs_pub = Arc::clone(&bytes_sent);
                let df_pub = Arc::clone(&dropped_frames);
                let ra_pub = Arc::clone(&reconnect_attempts);
                let rtt_pub = Arc::clone(&rtt_ms_bits);
                let err_pub = Arc::clone(&last_error);
                let auto_rec = config.auto_reconnect;
                let rec_delay = config.reconnect_delay_sec.max(1);
                let max_ret = config.max_retries;
                let meta_w = config.width;
                let meta_h = config.height;
                let meta_fps = config.fps;
                let meta_br = config.bitrate_kbps;

                tokio_handle.spawn(async move {
                    let mut retries = 0u32;
                    let mut cached_avc_seq: Option<FlvPacket> = None;
                    let mut cached_aac_seq: Option<FlvPacket> = None;

                    while r_pub.load(Ordering::Relaxed) {
                        if is_primary {
                            *st_pub.lock() = if retries == 0 {
                                StreamState::Connecting
                            } else {
                                StreamState::Reconnecting
                            };
                        }
                        let t_conn = Instant::now();
                        let mut client = match RtmpClient::connect_and_publish(&target).await {
                            Ok(c) => c,
                            Err(e) => {
                                if is_primary {
                                    *err_pub.lock() = Some(e.to_string());
                                }
                                if !auto_rec || retries >= max_ret || !r_pub.load(Ordering::Relaxed)
                                {
                                    if is_primary {
                                        *st_pub.lock() = StreamState::Stopped;
                                    }
                                    break;
                                }
                                retries += 1;
                                if is_primary {
                                    ra_pub.store(retries, Ordering::Relaxed);
                                }
                                tokio::time::sleep(Duration::from_secs(rec_delay as u64)).await;
                                continue;
                            }
                        };

                        let rtt = t_conn.elapsed().as_secs_f64() * 1000.0;
                        if is_primary {
                            rtt_pub.store(rtt.to_bits(), Ordering::Relaxed);
                            *st_pub.lock() = StreamState::Streaming;
                            *err_pub.lock() = None;
                        }

                        // Send initial stream metadata + cached sequence headers on reconnect
                        let meta = rml_rtmp::sessions::StreamMetadata {
                            video_width: Some(meta_w),
                            video_height: Some(meta_h),
                            video_codec_id: Some(7),
                            video_frame_rate: Some(meta_fps as f32),
                            video_bitrate_kbps: Some(meta_br),
                            audio_codec_id: Some(10),
                            audio_bitrate_kbps: Some(160),
                            audio_sample_rate: Some(48000),
                            audio_channels: Some(2),
                            audio_is_stereo: Some(true),
                            encoder: Some(format!("OxideStudio {idx}")),
                        };
                        let _ = client.send_metadata(meta).await;
                        if let Some(seq) = &cached_avc_seq {
                            let _ = client
                                .send_video_data(seq.payload.clone(), seq.timestamp_ms)
                                .await;
                        }
                        if let Some(seq) = &cached_aac_seq {
                            let _ = client
                                .send_audio_data(seq.payload.clone(), seq.timestamp_ms)
                                .await;
                        }

                        let mut last_bytes = 0u64;
                        let mut disconnected = false;
                        while r_pub.load(Ordering::Relaxed) {
                            match tokio::time::timeout(Duration::from_millis(250), prx.recv()).await
                            {
                                Ok(Some(pkt)) => {
                                    let t_send = Instant::now();
                                    let res = match pkt.tag_type {
                                        9 => {
                                            if pkt.payload.len() >= 2 && pkt.payload[1] == 0 {
                                                cached_avc_seq = Some(pkt.clone());
                                            }
                                            client
                                                .send_video_data(pkt.payload, pkt.timestamp_ms)
                                                .await
                                        }
                                        8 => {
                                            if pkt.payload.len() >= 2 && pkt.payload[1] == 0 {
                                                cached_aac_seq = Some(pkt.clone());
                                            }
                                            client
                                                .send_audio_data(pkt.payload, pkt.timestamp_ms)
                                                .await
                                        }
                                        _ => Ok(()),
                                    };
                                    if let Err(e) = res {
                                        if is_primary {
                                            *err_pub.lock() = Some(e.to_string());
                                        }
                                        df_pub.fetch_add(1, Ordering::Relaxed);
                                        disconnected = true;
                                        break;
                                    }
                                    if is_primary {
                                        let delta = client.bytes_sent.saturating_sub(last_bytes);
                                        last_bytes = client.bytes_sent;
                                        bs_pub.fetch_add(delta, Ordering::Relaxed);
                                        let send_ms = t_send.elapsed().as_secs_f64() * 1000.0;
                                        rtt_pub.store(send_ms.to_bits(), Ordering::Relaxed);
                                    }
                                }
                                Ok(None) => {
                                    r_pub.store(false, Ordering::Relaxed);
                                    break;
                                }
                                Err(_) => {
                                    if let Err(e) = client.poll_inbound_nonblocking().await {
                                        if is_primary {
                                            *err_pub.lock() = Some(e.to_string());
                                        }
                                        disconnected = true;
                                        break;
                                    }
                                }
                            }
                        }
                        if !disconnected || !auto_rec || retries >= max_ret {
                            break;
                        }
                        retries += 1;
                        if is_primary {
                            ra_pub.store(retries, Ordering::Relaxed);
                        }
                        tokio::time::sleep(Duration::from_secs(rec_delay as u64)).await;
                    }
                });
            }
        }

        let spawned = spawn_stream_encoder(
            &ffmpeg,
            &config,
            config.bitrate_kbps,
            use_srt_direct,
            &out_senders,
            &total_frames,
            &dropped_frames,
            &timestamp_offset_ms,
            &last_flv_ts_ms,
        )?;

        let video_tx = Arc::new(Mutex::new(Some(spawned.video_tx)));
        let audio_tx = Arc::new(Mutex::new(Some(spawned.audio_tx)));
        let encoder_proc = Arc::new(Mutex::new(Some(spawned.proc)));

        let supervisor_thread = if config.dynamic_bitrate && !use_srt_direct {
            let sup_run = Arc::clone(&running);
            let sup_proc = Arc::clone(&encoder_proc);
            let sup_vtx = Arc::clone(&video_tx);
            let sup_atx = Arc::clone(&audio_tx);
            let sup_drop = Arc::clone(&dropped_frames);
            let sup_rtt = Arc::clone(&rtt_ms_bits);
            let sup_dyn_br = Arc::clone(&dynamic_bitrate_kbps);
            let sup_tot = Arc::clone(&total_frames);
            let sup_off = Arc::clone(&timestamp_offset_ms);
            let sup_last_ts = Arc::clone(&last_flv_ts_ms);
            let sup_senders = out_senders.clone();
            let sup_cfg = config.clone();
            let sup_ffmpeg = ffmpeg.clone();

            Some(
                thread::Builder::new()
                    .name("obs-stream-dynbr".into())
                    .spawn(move || {
                        let mut ctrl = DynamicBitrateController::new(true, sup_cfg.bitrate_kbps);
                        let mut prev_dropped = sup_drop.load(Ordering::Relaxed);
                        let mut ticks = 0u32;

                        while sup_run.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(200));
                            ticks += 1;
                            if ticks < 10 {
                                continue;
                            }
                            ticks = 0;

                            let now_dropped = sup_drop.load(Ordering::Relaxed);
                            let dropped_delta = now_dropped.saturating_sub(prev_dropped);
                            prev_dropped = now_dropped;
                            let send_ms = f64::from_bits(sup_rtt.load(Ordering::Relaxed));

                            if let Some(new_kbps) = ctrl.evaluate(dropped_delta, send_ms) {
                                *sup_vtx.lock() = None;
                                *sup_atx.lock() = None;
                                if let Some(mut old) = sup_proc.lock().take() {
                                    old.stop_clean();
                                }
                                if !sup_run.load(Ordering::Relaxed) {
                                    break;
                                }
                                let next_off =
                                    sup_last_ts.load(Ordering::Acquire).saturating_add(33);
                                sup_off.store(next_off, Ordering::Release);
                                if let Ok(next_enc) = spawn_stream_encoder(
                                    &sup_ffmpeg,
                                    &sup_cfg,
                                    new_kbps,
                                    false,
                                    &sup_senders,
                                    &sup_tot,
                                    &sup_drop,
                                    &sup_off,
                                    &sup_last_ts,
                                ) {
                                    sup_dyn_br.store(new_kbps, Ordering::Relaxed);
                                    *sup_proc.lock() = Some(next_enc.proc);
                                    *sup_vtx.lock() = Some(next_enc.video_tx);
                                    *sup_atx.lock() = Some(next_enc.audio_tx);
                                }
                            }
                        }
                    })
                    .map_err(ObsError::Io)?,
            )
        } else {
            None
        };

        Ok(Self {
            config,
            video_tx,
            audio_tx,
            running,
            state,
            bytes_sent,
            total_frames,
            dropped_frames,
            reconnect_attempts,
            dynamic_bitrate_kbps,
            rtt_ms_bits,
            last_error,
            active_extra_outputs,
            start_instant: Instant::now(),
            encoder_proc,
            out_senders,
            supervisor_thread,
        })
    }

    pub fn push_video_nv12(&self, nv12: &[u8]) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        let tx_opt = self.video_tx.lock().clone();
        if let Some(tx) = tx_opt {
            if tx.try_send(nv12.to_vec()).is_err() {
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn push_audio_f32(&self, stereo_f32: &[f32]) {
        if !self.running.load(Ordering::Relaxed) || stereo_f32.is_empty() {
            return;
        }
        let tx_opt = self.audio_tx.lock().clone();
        if let Some(tx) = tx_opt {
            let mut bytes = Vec::with_capacity(stereo_f32.len() * 4);
            for &s in stereo_f32 {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let _ = tx.try_send(bytes);
        }
    }

    pub fn stats(&self) -> StreamingStats {
        let secs = self.start_instant.elapsed().as_secs_f64().max(0.001);
        let bytes = self.bytes_sent.load(Ordering::Relaxed);
        let total = self.total_frames.load(Ordering::Relaxed);
        let dropped = self.dropped_frames.load(Ordering::Relaxed);
        let dropped_percent = if total > 0 {
            (dropped as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        StreamingStats {
            state: *self.state.lock(),
            service_name: self.config.service_name.clone(),
            server_url: self.config.server_url.clone(),
            duration_ms: self.start_instant.elapsed().as_millis() as u64,
            bytes_sent: bytes,
            total_frames: total,
            dropped_frames: dropped,
            dropped_percent,
            bitrate_kbps: ((bytes as f64) * 8.0 / 1000.0) / secs,
            dynamic_bitrate_kbps: self.dynamic_bitrate_kbps.load(Ordering::Relaxed),
            rtt_ms: f64::from_bits(self.rtt_ms_bits.load(Ordering::Relaxed)),
            reconnect_attempts: self.reconnect_attempts.load(Ordering::Relaxed),
            active_extra_outputs: self.active_extra_outputs,
            last_error: self.last_error.lock().clone(),
        }
    }

    pub fn stop(&mut self) {
        *self.state.lock() = StreamState::Stopping;
        self.running.store(false, Ordering::SeqCst);
        *self.video_tx.lock() = None;
        *self.audio_tx.lock() = None;
        if let Some(sup) = self.supervisor_thread.take() {
            let _ = sup.join();
        }
        if let Some(mut proc) = self.encoder_proc.lock().take() {
            proc.stop_clean();
        }
        self.out_senders.clear();
        *self.state.lock() = StreamState::Stopped;
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        if self.running.load(Ordering::Relaxed) {
            self.stop();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_stream_encoder(
    ffmpeg: &Path,
    config: &StreamSessionConfig,
    bitrate_kbps: u32,
    use_srt_direct: bool,
    out_senders: &[tokio::sync::mpsc::Sender<FlvPacket>],
    total_frames: &Arc<AtomicU64>,
    dropped_frames: &Arc<AtomicU64>,
    timestamp_offset_ms: &Arc<AtomicU32>,
    last_flv_ts_ms: &Arc<AtomicU32>,
) -> Result<SpawnedEncoder> {
    let pipe_name = format!("obs-rust-stream-{}", Uuid::new_v4());
    let mut audio_pipe = WindowsNamedPipe::create(&pipe_name)?;
    let pipe_cancel = audio_pipe.cancel_token();
    let pipe_path = audio_pipe.path.clone();

    let (video_tx, video_rx) = bounded::<Vec<u8>>(16);
    let (audio_tx, audio_rx) = bounded::<Vec<u8>>(128);

    let mut cmd = Command::new(ffmpeg);
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
        &format!("{}x{}", config.width, config.height),
        "-r",
        &config.fps.max(1).to_string(),
        "-i",
        "pipe:0",
        "-f",
        "f32le",
        "-ar",
        &config.sample_rate.to_string(),
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
        encoder_id: config.video_encoder.clone(),
        bitrate_kbps,
        rate_control: "CBR".to_string(),
        crf: 21,
        preset: config.preset.clone(),
        keyint_sec: config.keyint_sec,
        fps: config.fps,
        color_space: config.color_space,
        color_range: config.color_range,
        streaming_low_latency: true,
    });
    cmd.args(v_args);
    cmd.args([
        "-c:a",
        "aac",
        "-b:a",
        &format!("{}k", config.audio_bitrate_kbps.clamp(64, 320)),
    ]);

    if use_srt_direct {
        let mut url = config.server_url.clone();
        if !url.contains("latency=") {
            let sep = if url.contains('?') { '&' } else { '?' };
            url.push_str(&format!("{sep}latency={}", config.srt_latency_ms * 1000));
        }
        if !config.srt_passphrase.trim().is_empty() && !url.contains("passphrase=") {
            url.push_str(&format!("&passphrase={}", config.srt_passphrase.trim()));
        }
        cmd.args(["-f", "mpegts", &url]);
    } else {
        cmd.args(["-f", "flv", "pipe:1"]);
    }

    cmd.stdin(Stdio::piped())
        .stdout(if use_srt_direct {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ObsError::Stream(format!("Failed spawning stream encoder: {e}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ObsError::Stream("Missing stream encoder stdin".into()))?;
    let stdout = child.stdout.take();

    let proc_running = Arc::new(AtomicBool::new(true));
    let mut threads = Vec::new();

    // Video encoder feeder
    let r_v = Arc::clone(&proc_running);
    let tf_v = Arc::clone(total_frames);
    let df_v = Arc::clone(dropped_frames);
    threads.push(
        thread::Builder::new()
            .name("obs-stream-vid".into())
            .spawn(move || {
                use std::io::Write;
                while let Ok(frame) = video_rx.recv() {
                    if stdin.write_all(&frame).is_err() {
                        df_v.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                    tf_v.fetch_add(1, Ordering::Relaxed);
                    if !r_v.load(Ordering::Relaxed) && video_rx.is_empty() {
                        break;
                    }
                }
                let _ = stdin.flush();
            })
            .map_err(ObsError::Io)?,
    );

    // Audio named pipe feeder
    let r_a = Arc::clone(&proc_running);
    threads.push(
        thread::Builder::new()
            .name("obs-stream-aud".into())
            .spawn(move || {
                if audio_pipe.connect().is_err() {
                    return;
                }
                while let Ok(bytes) = audio_rx.recv() {
                    if audio_pipe.write_all(&bytes).is_err() {
                        break;
                    }
                    if !r_a.load(Ordering::Relaxed) && audio_rx.is_empty() {
                        break;
                    }
                }
                audio_pipe.close();
            })
            .map_err(ObsError::Io)?,
    );

    if let Some(flv_stdout) = stdout {
        let r_flv = Arc::clone(&proc_running);
        let df_flv = Arc::clone(dropped_frames);
        let off_flv = Arc::clone(timestamp_offset_ms);
        let last_flv = Arc::clone(last_flv_ts_ms);
        let senders_flv = out_senders.to_vec();
        threads.push(
            thread::Builder::new()
                .name("obs-flv-demux".into())
                .spawn(move || {
                    pump_flv_tags(flv_stdout, senders_flv, r_flv, df_flv, off_flv, last_flv);
                })
                .map_err(ObsError::Io)?,
        );
    }

    let proc = EncoderProcess {
        running: proc_running,
        pipe_cancel,
        child: Some(child),
        threads,
    };
    Ok(SpawnedEncoder {
        proc,
        video_tx,
        audio_tx,
    })
}

fn pump_flv_tags(
    mut reader: std::process::ChildStdout,
    senders: Vec<tokio::sync::mpsc::Sender<FlvPacket>>,
    running: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    timestamp_offset_ms: Arc<AtomicU32>,
    last_flv_ts_ms: Arc<AtomicU32>,
) {
    // FLV Header: 9 bytes + 4 bytes PreviousTagSize0
    let mut header = [0u8; 13];
    if reader.read_exact(&mut header).is_err() {
        return;
    }
    let mut tag_hdr = [0u8; 11];
    let mut prev_sz = [0u8; 4];
    let base_offset = timestamp_offset_ms.load(Ordering::Acquire);

    while running.load(Ordering::Relaxed) {
        if reader.read_exact(&mut tag_hdr).is_err() {
            break;
        }
        let tag_type = tag_hdr[0] & 0x1F;
        let data_size =
            ((tag_hdr[1] as usize) << 16) | ((tag_hdr[2] as usize) << 8) | (tag_hdr[3] as usize);
        let ts_low = ((tag_hdr[4] as u32) << 16) | ((tag_hdr[5] as u32) << 8) | (tag_hdr[6] as u32);
        let ts_ext = (tag_hdr[7] as u32) << 24;
        let raw_ts = ts_low | ts_ext;
        let timestamp_ms = base_offset.saturating_add(raw_ts);
        last_flv_ts_ms.store(timestamp_ms, Ordering::Release);

        let mut payload = vec![0u8; data_size];
        if reader.read_exact(&mut payload).is_err() {
            break;
        }
        if reader.read_exact(&mut prev_sz).is_err() {
            break;
        }

        if tag_type == 8 || tag_type == 9 {
            let pkt = FlvPacket {
                tag_type,
                timestamp_ms,
                payload: Bytes::from(payload),
            };
            for s in &senders {
                if s.try_send(pkt.clone()).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_bitrate_disabled_returns_none() {
        let mut ctrl = DynamicBitrateController::new(false, 6000);
        assert_eq!(ctrl.evaluate(20, 500.0), None);
        assert_eq!(ctrl.current_kbps, 6000);
    }

    #[test]
    fn dynamic_bitrate_steps_down_on_congestion_and_recovers() {
        let mut ctrl = DynamicBitrateController::new(true, 4000);
        assert_eq!(ctrl.min_kbps, 1000);

        // Congestion via dropped frames steps down 25%
        assert_eq!(ctrl.evaluate(5, 20.0), Some(3000));
        // Congestion via high RTT/send latency steps down again
        assert_eq!(ctrl.evaluate(0, 250.0), Some(2250));
        // Steps down until min_kbps floor (1000)
        assert_eq!(ctrl.evaluate(8, 10.0), Some(1687));
        assert_eq!(ctrl.evaluate(8, 10.0), Some(1265));
        assert_eq!(ctrl.evaluate(8, 10.0), Some(1000));
        assert_eq!(ctrl.evaluate(8, 10.0), None);

        // 4 clean windows don't increase yet; 5th stable window steps up (+10% = +100)
        for _ in 0..4 {
            assert_eq!(ctrl.evaluate(0, 15.0), None);
        }
        assert_eq!(ctrl.evaluate(0, 15.0), Some(1100));
    }
}
