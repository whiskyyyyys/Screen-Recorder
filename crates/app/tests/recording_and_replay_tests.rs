use obs_core::ReplayBufferState;
use obs_encoder::{locate_ffprobe, probe_all_encoders, resolve_best_video_encoder};
use obs_recorder::{
    remux_to_mp4, RecordingConfig, RecordingSession, ReplayBufferConfig, ReplayBufferSession,
};
use obs_rust::engine::ObsEngine;
use obs_settings::RecordingFormat;
use obs_ui::StudioBackend;
use obs_video::{rgba_to_nv12, ColorRange, ColorSpace};
use std::process::Command;
use std::time::Duration;

fn ffprobe_json(path: &std::path::Path) -> serde_json::Value {
    let ffprobe = locate_ffprobe().expect("ffprobe.exe must exist");
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
            &path.to_string_lossy(),
        ])
        .output()
        .expect("ffprobe failed");
    assert!(
        out.status.success(),
        "ffprobe error on {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn synthetic_nv12_frame(w: u32, h: u32, step: u8) -> Vec<u8> {
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = step.wrapping_mul(8);
        px[1] = 120;
        px[2] = 220u8.saturating_sub(step.wrapping_mul(4));
        px[3] = 255;
    }
    rgba_to_nv12(&rgba, w, h, ColorSpace::Rec709, ColorRange::Partial)
}

#[test]
fn real_multitrack_mkv_pause_resume_remux_and_hw_encoder() {
    let report = probe_all_encoders();
    assert!(report.ffmpeg_found, "ffmpeg.exe must be detected");
    let best_enc = resolve_best_video_encoder("auto", &report);
    if report
        .video_encoders
        .iter()
        .any(|e| e.hardware && e.available && e.codec == obs_encoder::VideoCodec::H264)
    {
        assert_ne!(
            best_enc, "libx264",
            "Expected hardware H.264 encoder to take precedence over libx264 when available"
        );
    }

    let tmp_dir = std::env::temp_dir().join("obs_rust_rec_tests");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let mkv_path = tmp_dir.join("multitrack_pause.mkv");
    let _ = std::fs::remove_file(&mkv_path);

    let session = RecordingSession::start(RecordingConfig {
        output_path: mkv_path.clone(),
        format: RecordingFormat::Mkv,
        width: 320,
        height: 240,
        fps: 30,
        sample_rate: 48000,
        video_encoder: best_enc.clone(),
        audio_encoder: "aac".to_string(),
        rate_control: "CBR".to_string(),
        bitrate_kbps: 2000,
        crf: 20,
        preset: "speed".to_string(),
        keyint_sec: 1,
        audio_track_mask: 0b0000_0011, // 2 independent audio tracks!
        audio_bitrate_kbps: 128,
        color_space: ColorSpace::Rec709,
        color_range: ColorRange::Partial,
        auto_remux_to_mp4: false,
    })
    .expect("RecordingSession::start failed");

    let audio_1600 = vec![0.05f32; 1600 * 2]; // 33.3ms per frame at 48kHz
    let tracks = [
        audio_1600.clone(),
        audio_1600,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ];

    for i in 0..25u8 {
        session.push_video_nv12(&synthetic_nv12_frame(320, 240, i));
        session.push_audio_tracks(&tracks);
        std::thread::sleep(Duration::from_millis(20));
    }

    // Pause, push frames that MUST be ignored, then resume
    session.pause();
    assert!(session.is_paused());
    for i in 0..10u8 {
        session.push_video_nv12(&synthetic_nv12_frame(320, 240, i));
    }
    session.resume();
    assert!(!session.is_paused());

    for i in 25..45u8 {
        session.push_video_nv12(&synthetic_nv12_frame(320, 240, i));
        session.push_audio_tracks(&tracks);
        std::thread::sleep(Duration::from_millis(20));
    }

    let final_mkv = session.stop().expect("Recording stop failed");
    assert!(final_mkv.exists());
    let probe = ffprobe_json(&final_mkv);
    let streams = probe["streams"].as_array().unwrap();
    let v_count = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .count();
    let a_count = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .count();
    assert_eq!(v_count, 1, "Expected 1 video stream in MKV");
    assert_eq!(a_count, 2, "Expected 2 independent audio tracks in MKV");
    let v_codec = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("");
    assert_eq!(v_codec, "h264", "Expected H.264 encoded video stream");

    // Lossless Remux MKV -> MP4
    let remuxed_mp4 = remux_to_mp4(&final_mkv, None).expect("Remux to MP4 failed");
    let mp4_probe = ffprobe_json(&remuxed_mp4);
    assert_eq!(mp4_probe["streams"].as_array().unwrap().len(), 3);

    // Fragmented MP4 recording check
    let fmp4_path = tmp_dir.join("frag_test.mp4");
    let _ = std::fs::remove_file(&fmp4_path);
    let fmp4_sess = RecordingSession::start(RecordingConfig {
        output_path: fmp4_path.clone(),
        format: RecordingFormat::FragmentedMp4,
        width: 320,
        height: 240,
        fps: 30,
        sample_rate: 48000,
        video_encoder: "libx264".to_string(),
        audio_encoder: "aac".to_string(),
        rate_control: "CBR".to_string(),
        bitrate_kbps: 1500,
        crf: 22,
        preset: "speed".to_string(),
        keyint_sec: 1,
        audio_track_mask: 1,
        audio_bitrate_kbps: 128,
        color_space: ColorSpace::Rec709,
        color_range: ColorRange::Partial,
        auto_remux_to_mp4: false,
    })
    .unwrap();
    for i in 0..20u8 {
        fmp4_sess.push_video_nv12(&synthetic_nv12_frame(320, 240, i));
        fmp4_sess.push_audio_tracks(&tracks);
        std::thread::sleep(Duration::from_millis(15));
    }
    let fmp4_out = fmp4_sess.stop().unwrap();
    let fmp4_probe = ffprobe_json(&fmp4_out);
    assert!(fmp4_probe["streams"].as_array().unwrap().len() >= 2);

    // Replay Buffer EBML cluster ring & Save Replay
    let mut rb = ReplayBufferSession::start(ReplayBufferConfig {
        width: 320,
        height: 240,
        fps: 30,
        sample_rate: 48000,
        encoder_id: "libx264".to_string(),
        bitrate_kbps: 1500,
        audio_bitrate_kbps: 128,
        color_space: ColorSpace::Rec709,
        color_range: ColorRange::Partial,
        max_duration_sec: 10,
        max_memory_mb: 64,
    })
    .expect("ReplayBufferSession::start failed");

    // QA-09: Early save error before clusters accumulate must return Err and leave state Active
    let early_err = rb.save_replay(&tmp_dir.join("early_fail.mp4"));
    assert!(early_err.is_err());
    assert_eq!(rb.stats().state, ReplayBufferState::Active);

    for i in 0..65u8 {
        rb.push_video_nv12(&synthetic_nv12_frame(320, 240, i));
        rb.push_audio_f32(&tracks[0]);
        std::thread::sleep(Duration::from_millis(20));
    }

    // QA-09: Post-buffer mux failure (invalid extension) must still reset state back to Active
    let mux_err = rb.save_replay(&tmp_dir.join("bad_mux.unsupported_ext_xyz"));
    assert!(mux_err.is_err());
    assert_eq!(rb.stats().state, ReplayBufferState::Active);

    let replay_out = tmp_dir.join("saved_replay.mp4");
    let _ = std::fs::remove_file(&replay_out);
    let saved_rb = rb.save_replay(&replay_out).expect("save_replay failed");
    let rb_probe = ffprobe_json(&saved_rb);
    assert!(rb_probe["streams"].as_array().unwrap().len() >= 2);
    rb.stop();

    // QA-02: Existing output file must not be overwritten on duplicate filename
    let existing = tmp_dir.join("do_not_overwrite.mkv");
    std::fs::write(&existing, b"SENTINEL_KEEP_BYTES").unwrap();
    let dup_res = RecordingSession::start(RecordingConfig {
        output_path: existing.clone(),
        format: RecordingFormat::Mkv,
        width: 320,
        height: 240,
        fps: 30,
        sample_rate: 48000,
        video_encoder: "libx264".to_string(),
        audio_encoder: "aac".to_string(),
        rate_control: "CBR".to_string(),
        bitrate_kbps: 1500,
        crf: 22,
        preset: "speed".to_string(),
        keyint_sec: 1,
        audio_track_mask: 1,
        audio_bitrate_kbps: 128,
        color_space: ColorSpace::Rec709,
        color_range: ColorRange::Partial,
        auto_remux_to_mp4: false,
    });
    assert!(
        dup_res.is_err(),
        "Expected RecordingSession::start to refuse overwriting existing file"
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"SENTINEL_KEEP_BYTES");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn engine_locks_output_format_settings_while_recording_active() {
    let eng = ObsEngine::initialize(true);
    let orig = eng.settings();
    let tmp_rec_dir = std::env::temp_dir().join("obs_rust_lock_test");
    let _ = std::fs::create_dir_all(&tmp_rec_dir);

    let mut cfg = orig.clone();
    cfg.output.rec_path = tmp_rec_dir.to_string_lossy().to_string();
    cfg.output.rec_filename_format = "lock_probe".to_string();
    cfg.output.rec_format = RecordingFormat::Mkv;
    cfg.output.rec_video_encoder = "libx264".to_string();
    let _ = std::fs::remove_file(tmp_rec_dir.join("lock_probe.mkv"));

    eng.apply_settings(cfg.clone())
        .expect("Applying settings while idle should succeed");
    eng.start_recording()
        .expect("Starting recording on headless engine should succeed");
    assert!(eng.has_active_output());

    // Changing video resolution/FPS or audio sample rate while active MUST be rejected
    let mut bad_video = cfg.clone();
    bad_video.video.config.output_width = 640;
    bad_video.video.config.output_height = 360;
    assert!(eng.apply_settings(bad_video).is_err());

    let mut bad_audio = cfg.clone();
    bad_audio.audio.sample_rate = if cfg.audio.sample_rate == 48000 {
        44100
    } else {
        48000
    };
    assert!(eng.apply_settings(bad_audio).is_err());

    // Non-format setting (e.g. confirm dialog) while active should still succeed
    let mut safe_change = cfg;
    safe_change.general.confirm_on_record_stop = !safe_change.general.confirm_on_record_stop;
    assert!(eng.apply_settings(safe_change).is_ok());

    let recorded = eng.stop_recording().expect("Stop recording failed");
    assert!(!eng.has_active_output());
    let _ = eng.apply_settings(orig);
    eng.shutdown();
    let _ = std::fs::remove_file(recorded);
    let _ = std::fs::remove_dir_all(&tmp_rec_dir);
}
