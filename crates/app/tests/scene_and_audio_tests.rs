use obs_audio::{cubic_fader_to_mul, mul_to_cubic_fader, AudioMixer};
use obs_core::{Clock, SourceId};
use obs_filters::{linear_to_db, AudioFilterInstance, AudioFilterKind};
use obs_scenes::{
    SceneCollection, SourceAudioSettings, SourceKind, SourceProperties, GLOBAL_DESKTOP_1_ID,
    GLOBAL_DESKTOP_2_ID, GLOBAL_MIC_1_ID, GLOBAL_MIC_2_ID,
};
use obs_settings::{AppSettings, ProfileManager, RecordingFormat};
use obs_sources::SourceManager;
use std::f32::consts::PI;
use std::sync::Arc;

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

#[test]
fn global_audio_devices_and_active_scene_audio_gating() {
    let mut col = SceneCollection::default_starter("AudioGatingTest");
    let s1 = col.active_scene_id;
    let s2 = col.create_scene("Secondary").unwrap();

    // QA-05: 4 Global audio slots reflect settings and preserve volume/filters on update
    col.sync_global_audio_devices("default", "Disabled", "mic-1-id", "mic-2-id");
    assert!(col.sources.contains_key(&GLOBAL_DESKTOP_1_ID));
    assert!(!col.sources.contains_key(&GLOBAL_DESKTOP_2_ID));
    assert!(col.sources.contains_key(&GLOBAL_MIC_1_ID));
    assert!(col.sources.contains_key(&GLOBAL_MIC_2_ID));

    col.sources
        .get_mut(&GLOBAL_MIC_1_ID)
        .unwrap()
        .audio
        .volume_mul = 0.75;
    col.sync_global_audio_devices("Disabled", "desk-2-id", "mic-1-updated", "Disabled");
    assert!(!col.sources.contains_key(&GLOBAL_DESKTOP_1_ID));
    assert!(col.sources.contains_key(&GLOBAL_DESKTOP_2_ID));
    assert!(!col.sources.contains_key(&GLOBAL_MIC_2_ID));
    assert!((col.sources[&GLOBAL_MIC_1_ID].audio.volume_mul - 0.75).abs() < 1e-6);
    let global_active = col.active_audio_source_ids(None);
    assert!(global_active.contains(&GLOBAL_DESKTOP_2_ID));
    assert!(global_active.contains(&GLOBAL_MIC_1_ID));

    // Disable global hardware WASAPI slots before deterministic mixer isolation check
    // so live OS audio hardware noise cannot affect synthetic tone assertions.
    col.sync_global_audio_devices("Disabled", "Disabled", "Disabled", "Disabled");

    // QA-11: Scene-specific audio sources only mix when their scene is active or transitioning
    let (s1_aud, _) = col
        .create_source_and_add(
            s1,
            "Scene 1 Media Audio",
            SourceKind::MediaSource,
            SourceProperties::default_for(SourceKind::MediaSource),
        )
        .unwrap();
    let (s2_aud, _) = col
        .create_source_and_add(
            s2,
            "Scene 2 Media Audio",
            SourceKind::MediaSource,
            SourceProperties::default_for(SourceKind::MediaSource),
        )
        .unwrap();

    let active_ids = col.active_audio_source_ids(None);
    assert!(active_ids.contains(&s1_aud));
    assert!(!active_ids.contains(&s2_aud));

    let mixer = Arc::new(AudioMixer::new(48000));
    let mgr = SourceManager::new(Clock::new(), Arc::clone(&mixer), 30);
    mgr.sync_collection(&col);

    let tone: Vec<f32> = vec![0.4f32; 480 * 2];
    mixer.push_interleaved_stereo(s1_aud, &tone, 48000);
    let blk1 = mixer.mix_block(480);
    assert!(blk1.tracks[0].iter().any(|&x| x.abs() > 0.2));

    // Pushing to Scene 2 audio while Scene 1 is active must be ignored by mixer
    mixer.push_interleaved_stereo(s2_aud, &tone, 48000);
    let blk_ignored = mixer.mix_block(480);
    assert!(blk_ignored.tracks[0].iter().all(|&x| x == 0.0));

    // During transition (s1, s2), both scenes' audio sources are active; after switching to s2, only s2
    mgr.sync_collection_with_transition(&col, Some((s1, s2)));
    mixer.push_interleaved_stereo(s2_aud, &tone, 48000);
    let blk_tr = mixer.mix_block(480);
    assert!(blk_tr.tracks[0].iter().any(|&x| x.abs() > 0.2));

    col.active_scene_id = s2;
    mgr.sync_collection(&col);
    mixer.push_interleaved_stereo(s1_aud, &tone, 48000);
    let blk_after = mixer.mix_block(480);
    assert!(blk_after.tracks[0].iter().all(|&x| x == 0.0));
}
