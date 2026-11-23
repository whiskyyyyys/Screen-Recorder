use obs_audio::AudioMixer;
use obs_core::SourceId;
use obs_encoder::{locate_ffprobe, probe_all_encoders};
use obs_filters::{AudioFilterInstance, AudioFilterKind, VideoFilterInstance, VideoFilterKind};
use obs_recorder::{RecordingConfig, RecordingSession};
use obs_renderer::GpuCompositor;
use obs_scenes::{
    AudioMonitoringType, SceneCollection, SceneItem, SourceAudioSettings, SourceKind,
    SourceProperties,
};
use obs_settings::RecordingFormat;
use obs_video::{rgba_to_nv12, ColorRange, ColorSpace, VideoConfig, VideoFrame};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::process::Command;
use std::time::Instant;

fn make_nv12_sequence(w: u32, h: u32, count: usize) -> Vec<Vec<u8>> {
    let mut seq = Vec::with_capacity(count);
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    for step in 0..count {
        let base_r = ((step * 7) % 256) as u8;
        let base_b = (255usize.saturating_sub((step * 5) % 256)) as u8;
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px[0] = base_r.wrapping_add((i & 0x1F) as u8);
            px[1] = 110u8.wrapping_add(((i >> 5) & 0x1F) as u8);
            px[2] = base_b;
            px[3] = 255;
        }
        seq.push(rgba_to_nv12(
            &rgba,
            w,
            h,
            ColorSpace::Rec709,
            ColorRange::Partial,
        ));
    }
    seq
}

fn ffprobe_video_codec(path: &std::path::Path) -> String {
    let Ok(ffprobe) = locate_ffprobe() else {
        return "unknown".to_string();
    };
    let Ok(out) = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
    else {
        return "unknown".to_string();
    };
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    println!(
        "=== OxideStudio (OBS-Rust) v{} Benchmark Suite ===",
        env!("CARGO_PKG_VERSION")
    );

    // 1. GPU Compositor 1080p Benchmark
    let cfg = VideoConfig {
        canvas_width: 1920,
        canvas_height: 1080,
        output_width: 1920,
        output_height: 1080,
        fps_num: 60,
        fps_den: 1,
        ..Default::default()
    };
    let mut comp = GpuCompositor::new_headless(cfg).expect("Failed to create headless wgpu GPU");
    println!(
        "GPU Adapter: {} (Backend: {})",
        comp.adapter_name(),
        comp.backend_name()
    );

    let mut col = SceneCollection::default_starter("BenchScene");
    let s1 = col.active_scene_id;
    let (bg_id, _) = col
        .create_source_and_add(
            s1,
            "BenchBg",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [35, 65, 160, 255],
                width: 1920,
                height: 1080,
            },
        )
        .unwrap();
    let (fg_id, _) = col
        .create_source_and_add(
            s1,
            "BenchKeyedOverlay",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [0, 255, 0, 255],
                width: 1920,
                height: 1080,
            },
        )
        .unwrap();
    col.sources
        .get_mut(&fg_id)
        .unwrap()
        .video_filters
        .push(VideoFilterInstance::new(
            "Chroma",
            VideoFilterKind::default_chroma_key(),
        ));
    col.scene_by_id_mut(s1).unwrap().items = vec![SceneItem::new(bg_id), SceneItem::new(fg_id)];

    let mut frames = HashMap::new();
    frames.insert(
        bg_id,
        VideoFrame::new_solid_rgba(1920, 1080, [35, 65, 160, 255], 1),
    );
    frames.insert(
        fg_id,
        VideoFrame::new_solid_rgba(1920, 1080, [0, 255, 0, 255], 2),
    );

    // Warmup
    let _ = comp.render_frame(&col, &frames, None, false, false, true);

    let gpu_iters = 120usize;
    let t0 = Instant::now();
    for i in 0..gpu_iters {
        if i % 4 == 0 {
            if let Some(f) = frames.get_mut(&bg_id) {
                f.seq += 1;
            }
        }
        let _ = comp.render_frame(&col, &frames, None, false, false, false);
    }
    let gpu_no_rb = t0.elapsed();

    let t1 = Instant::now();
    for i in 0..gpu_iters {
        if i % 4 == 0 {
            if let Some(f) = frames.get_mut(&bg_id) {
                f.seq += 1;
            }
        }
        let _ = comp.render_frame(&col, &frames, None, false, false, true);
    }
    let gpu_with_rb = t1.elapsed();

    println!(
        "[GPU 1080p Composite Only]     {} frames in {:.3}s => {:.2} ms/frame ({:.1} FPS)",
        gpu_iters,
        gpu_no_rb.as_secs_f64(),
        gpu_no_rb.as_secs_f64() * 1000.0 / (gpu_iters as f64),
        (gpu_iters as f64) / gpu_no_rb.as_secs_f64()
    );
    println!(
        "[GPU 1080p Composite+NV12+RB]  {} frames in {:.3}s => {:.2} ms/frame ({:.1} FPS)",
        gpu_iters,
        gpu_with_rb.as_secs_f64(),
        gpu_with_rb.as_secs_f64() * 1000.0 / (gpu_iters as f64),
        (gpu_iters as f64) / gpu_with_rb.as_secs_f64()
    );

    // 2. CPU RGBA -> NV12 Conversion Benchmark
    let cpu_rgba = vec![128u8; 1920 * 1080 * 4];
    let cpu_iters = 30usize;
    let t_cpu = Instant::now();
    let mut cpu_checksum = 0usize;
    for _ in 0..cpu_iters {
        let out = rgba_to_nv12(
            &cpu_rgba,
            1920,
            1080,
            ColorSpace::Rec709,
            ColorRange::Partial,
        );
        cpu_checksum = cpu_checksum.wrapping_add(out[0] as usize);
    }
    let cpu_dt = t_cpu.elapsed();
    println!(
        "[CPU 1080p RGBA->NV12]         {} frames in {:.3}s => {:.2} ms/frame ({:.1} FPS, chk={})",
        cpu_iters,
        cpu_dt.as_secs_f64(),
        cpu_dt.as_secs_f64() * 1000.0 / (cpu_iters as f64),
        (cpu_iters as f64) / cpu_dt.as_secs_f64(),
        cpu_checksum
    );

    // 3. Audio Mixer + DSP Chain Benchmark (48 kHz, 6 tracks, 4 active sources w/ 5 filters each, 10s audio)
    let sr = 48_000u32;
    let mixer = AudioMixer::new(sr);
    let mut sids = Vec::new();
    let mut sync_list = Vec::new();
    for ch in 0..4usize {
        let sid = SourceId::new();
        sids.push(sid);
        let cfg = SourceAudioSettings {
            volume_mul: 1.0,
            muted: false,
            mono_downmix: ch == 0,
            balance: 0.0,
            sync_offset_ms: 0,
            track_mask: 0b0011_1111,
            monitoring: AudioMonitoringType::None,
        };
        let filters = vec![
            AudioFilterInstance::new("NS", AudioFilterKind::default_noise_suppression()),
            AudioFilterInstance::new("Gate", AudioFilterKind::default_noise_gate()),
            AudioFilterInstance::new("Gain", AudioFilterKind::Gain { gain_db: 3.0 }),
            AudioFilterInstance::new(
                "EQ",
                AudioFilterKind::ThreeBandEq {
                    low_db: 1.5,
                    mid_db: 0.0,
                    high_db: 2.0,
                },
            ),
            AudioFilterInstance::new(
                "Limiter",
                AudioFilterKind::Limiter {
                    threshold_db: -3.0,
                    release_ms: 50.0,
                },
            ),
        ];
        sync_list.push((sid, cfg, filters));
    }
    mixer.sync_sources(&sync_list);

    let block_frames = 480usize; // 10 ms at 48 kHz
    let mut test_pcm = Vec::with_capacity(block_frames * 2);
    for i in 0..block_frames {
        let s = (2.0 * PI * 440.0 * (i as f32) / (sr as f32)).sin() * 0.25;
        test_pcm.push(s);
        test_pcm.push(s);
    }

    let blocks = 1000usize; // 10 seconds of audio
    let t_aud = Instant::now();
    let mut aud_acc = 0.0f32;
    for _ in 0..blocks {
        for &sid in &sids {
            mixer.push_interleaved_stereo(sid, &test_pcm, sr);
        }
        let mixed = mixer.mix_block(block_frames);
        aud_acc += mixed.tracks[0][0];
    }
    let aud_dt = t_aud.elapsed();
    let cpu_pct = (aud_dt.as_secs_f64() / 10.0) * 100.0;
    println!(
        "[Audio Mixer+DSP 4ch x 5flt]   1000 x 10ms blocks (10.0s audio) in {:.2}ms => {:.1} us/block ({:.3}% real-time CPU, acc={:.4})",
        aud_dt.as_secs_f64() * 1000.0,
        aud_dt.as_secs_f64() * 1_000_000.0 / (blocks as f64),
        cpu_pct,
        aud_acc
    );

    // 4. Video Encoders Benchmark (1920x1080 @ 60fps, 180 frames = 3.0s)
    let report = probe_all_encoders();
    println!("Encoder availability (1-frame live driver probe):");
    for e in &report.video_encoders {
        println!(
            "  - {:12} | hw={:5} | avail={:5} | {}",
            e.id, e.hardware, e.available, e.display_name
        );
    }

    let enc_w = 1920u32;
    let enc_h = 1080u32;
    let enc_fps = 60u32;
    let enc_frames = 180usize;
    let nv12_seq = make_nv12_sequence(enc_w, enc_h, 30);
    let audio_800 = vec![0.02f32; 800 * 2];
    let tracks = [
        audio_800,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ];

    let tmp_dir = std::env::temp_dir().join("obs_rust_bench");
    let _ = std::fs::create_dir_all(&tmp_dir);

    let targets = [
        "h264_amf",
        "hevc_amf",
        "h264_nvenc",
        "hevc_nvenc",
        "h264_qsv",
        "h264_mf",
        "libx264",
        "libx265",
        "libsvtav1",
    ];

    for enc_id in targets {
        let is_avail = report
            .video_encoders
            .iter()
            .any(|e| e.id == enc_id && e.available);
        if !is_avail {
            continue;
        }
        let out_path = tmp_dir.join(format!("bench_{enc_id}.mkv"));
        let _ = std::fs::remove_file(&out_path);
        let t_start = Instant::now();
        let sess = RecordingSession::start(RecordingConfig {
            output_path: out_path.clone(),
            format: RecordingFormat::Mkv,
            width: enc_w,
            height: enc_h,
            fps: enc_fps,
            sample_rate: sr,
            video_encoder: enc_id.to_string(),
            audio_encoder: "aac".to_string(),
            rate_control: "CBR".to_string(),
            bitrate_kbps: 6000,
            crf: 21,
            preset: "speed".to_string(),
            keyint_sec: 2,
            audio_track_mask: 1,
            audio_bitrate_kbps: 160,
            color_space: ColorSpace::Rec709,
            color_range: ColorRange::Partial,
            auto_remux_to_mp4: false,
        })
        .expect("RecordingSession::start failed in benchmark");

        for i in 0..enc_frames {
            sess.push_video_nv12(&nv12_seq[i % nv12_seq.len()]);
            sess.push_audio_tracks(&tracks);
        }
        let final_file = sess
            .stop()
            .expect("RecordingSession::stop failed in benchmark");
        let elapsed = t_start.elapsed().as_secs_f64();
        let size_kb = std::fs::metadata(&final_file)
            .map(|m| m.len() / 1024)
            .unwrap_or(0);
        let fps = (enc_frames as f64) / elapsed;
        let realtime_x = fps / (enc_fps as f64);
        let codec = ffprobe_video_codec(&final_file);
        println!(
            "[Encoder 1080p60 {:10}] 180 frames in {:.3}s => {:6.1} FPS ({:.2}x realtime) | size={:5} KB | codec={}",
            enc_id, elapsed, fps, realtime_x, size_kb, codec
        );
        let _ = std::fs::remove_file(&final_file);
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
