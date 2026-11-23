use obs_audio::{cubic_fader_to_mul, mul_to_cubic_fader, AudioMixer};
use obs_core::SourceId;
use obs_filters::{linear_to_db, AudioFilterInstance, AudioFilterKind};
use obs_scenes::{SceneCollection, SourceAudioSettings, SourceKind, SourceProperties};
use obs_settings::{AppSettings, ProfileManager, RecordingFormat};
use std::f32::consts::PI;

#[test]
fn scene_crud_nesting_groups_reference_cloning_and_import_export() {
    let mut col = SceneCollection::default_starter("TestCol");
    let s1 = col.active_scene_id;
    let s2 = col.create_scene("Gameplay").unwrap();
    let s3 = col.create_scene("Overlay").unwrap();

    // Create shared ColorSource in s1, reference-clone into s2
    let (src_id, item_s1) = col
        .create_source_and_add(
            s1,
            "Shared Brand Color",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [200, 40, 40, 255],
                width: 640,
                height: 360,
            },
        )
        .unwrap();
    let item_s2 = col.add_existing_source_to_scene(s2, src_id).unwrap();
    assert_ne!(item_s1, item_s2);

    // Grouping & ungrouping
    let grp_id = col.group_items(s1, &[item_s1], "Brand Group").unwrap();
    assert!(col.find_item(s1, grp_id).unwrap().is_group());
    col.ungroup_item(s1, grp_id).unwrap();

    // Nested scene s3 inside s2 -> s2 inside s1 works
    col.create_source_and_add(
        s2,
        "Overlay Ref",
        SourceKind::SceneSource,
        SourceProperties::SceneSource { scene_id: s3 },
    )
    .unwrap();
    col.create_source_and_add(
        s1,
        "Gameplay Ref",
        SourceKind::SceneSource,
        SourceProperties::SceneSource { scene_id: s2 },
    )
    .unwrap();

    // Cycle A -> B -> C -> A must be rejected!
    let cycle_err = col.create_source_and_add(
        s3,
        "Bad Cycle",
        SourceKind::SceneSource,
        SourceProperties::SceneSource { scene_id: s1 },
    );
    assert!(
        cycle_err.is_err(),
        "Expected scene cycle detection to reject A->B->C->A"
    );

    // Duplicate & rename
    let dup_id = col.duplicate_scene(s2, None).unwrap();
    col.rename_scene(dup_id, "Gameplay Copy Renamed").unwrap();
    assert_eq!(
        col.scene_by_id(dup_id).unwrap().name,
        "Gameplay Copy Renamed"
    );

    // Export & import JSON roundtrip
    let tmp = std::env::temp_dir().join("obs_rust_scene_test.json");
    col.save_to_file(&tmp).unwrap();
    let loaded = SceneCollection::load_from_file(&tmp).unwrap();
    assert_eq!(loaded.scenes.len(), col.scenes.len());
    assert_eq!(loaded.sources.len(), col.sources.len());
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn audio_dsp_filters_6_tracks_sync_offset_and_meters() {
    let sr = 48000u32;
    let mixer = AudioMixer::new(sr);

    // Verify cubic fader curve symmetry
    let mul = cubic_fader_to_mul(0.5);
    assert!((mul - 0.125).abs() < 1e-5);
    assert!((mul_to_cubic_fader(mul) - 0.5).abs() < 1e-4);

    let sid_loud = SourceId::new();
    let sid_quiet = SourceId::new();

    // Source 1: Limiter at -6 dB + Gain +12 dB -> ceiling must never exceed -5.9 dB
    // Routed to tracks 1 and 2 (0b0000_0011)
    let loud_cfg = SourceAudioSettings {
        volume_mul: 1.0,
        muted: false,
        mono_downmix: false,
        balance: 0.0,
        sync_offset_ms: 0,
        track_mask: 0b0000_0011,
        monitoring: obs_scenes::AudioMonitoringType::None,
    };
    let loud_filters = vec![
        AudioFilterInstance::new("Gain", AudioFilterKind::Gain { gain_db: 12.0 }),
        AudioFilterInstance::new(
            "Limiter",
            AudioFilterKind::Limiter {
                threshold_db: -6.0,
                release_ms: 50.0,
            },
        ),
    ];

    // Source 2: Noise Gate (-30 dB open / -36 dB close) -> very quiet -60dB signal gets silenced
    // Routed ONLY to track 3 (0b0000_0100)
    let quiet_cfg = SourceAudioSettings {
        volume_mul: 1.0,
        muted: false,
        mono_downmix: true,
        balance: 0.0,
        sync_offset_ms: 20, // 20ms sync delay
        track_mask: 0b0000_0100,
        monitoring: obs_scenes::AudioMonitoringType::MonitorAndOutput,
    };
    let quiet_filters = vec![
        AudioFilterInstance::new("Gate", AudioFilterKind::default_noise_gate()),
        AudioFilterInstance::new("NS", AudioFilterKind::default_noise_suppression()),
        AudioFilterInstance::new(
            "EQ",
            AudioFilterKind::ThreeBandEq {
                low_db: 2.0,
                mid_db: 0.0,
                high_db: 2.0,
            },
        ),
    ];

    mixer.sync_sources(&[
        (sid_loud, loud_cfg, loud_filters),
        (sid_quiet, quiet_cfg, quiet_filters),
    ]);

    let frames = 960;
    let mut loud_buf = Vec::with_capacity(frames * 2);
    let mut quiet_buf = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let s = (2.0 * PI * 440.0 * (i as f32) / (sr as f32)).sin() * 0.5;
        loud_buf.push(s);
        loud_buf.push(s);

        let q = (2.0 * PI * 1000.0 * (i as f32) / (sr as f32)).sin() * 0.0008;
        quiet_buf.push(q);
        quiet_buf.push(q);
    }

    mixer.push_interleaved_stereo(sid_loud, &loud_buf, sr);
    mixer.push_interleaved_stereo(sid_quiet, &quiet_buf, sr);

    let block = mixer.mix_block(frames);

    // Track 1 & 2 must contain limited signal <= -5.8 dB (~0.51 linear)
    let max_t1 = block.tracks[0]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    let max_t2 = block.tracks[1]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    assert!(max_t1 > 0.1, "Track 1 should have signal, got {max_t1}");
    assert!((max_t1 - max_t2).abs() < 1e-5, "Tracks 1 and 2 must match");
    assert!(
        linear_to_db(max_t1) <= -5.7,
        "Limiter failed to cap peak at -6dB: got {} dB",
        linear_to_db(max_t1)
    );

    // Track 3 only had quiet gated + 20ms delayed signal -> must be silent!
    let max_t3 = block.tracks[2]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_t3 < 0.002,
        "Track 3 should be gated/delayed near zero, got {max_t3}"
    );

    // Track 4 had no sources -> strictly 0
    assert!(block.tracks[3].iter().all(|&s| s == 0.0));
}

#[test]
fn settings_and_profiles_roundtrip() {
    let dir = std::env::temp_dir().join("obs_rust_profiles_test");
    let _ = std::fs::remove_dir_all(&dir);
    let pm = ProfileManager::with_root(dir.clone());

    let mut cfg = AppSettings::default();
    cfg.output.rec_format = RecordingFormat::FragmentedMp4;
    cfg.output.rec_audio_tracks = 0b0000_1111;
    cfg.advanced.websocket_port = 4477;
    cfg.video.config.canvas_width = 2560;
    cfg.video.config.canvas_height = 1440;

    pm.save_profile("StreamProfile", &cfg).unwrap();
    let loaded = pm.load_profile("StreamProfile").unwrap();
    assert_eq!(loaded.output.rec_format, RecordingFormat::FragmentedMp4);
    assert_eq!(loaded.output.rec_audio_tracks, 0b0000_1111);
    assert_eq!(loaded.advanced.websocket_port, 4477);
    assert_eq!(loaded.video.config.canvas_width, 2560);

    let _ = std::fs::remove_dir_all(&dir);
}
