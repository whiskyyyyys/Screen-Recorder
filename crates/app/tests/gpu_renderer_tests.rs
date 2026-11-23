use obs_core::SourceId;
use obs_filters::{VideoFilterInstance, VideoFilterKind};
use obs_renderer::GpuCompositor;
use obs_scenes::{SceneCollection, SceneItem, SourceKind, SourceProperties};
use obs_transitions::{ActiveTransition, TransitionConfig, TransitionKind};
use obs_video::{VideoConfig, VideoFrame};
use std::collections::HashMap;

#[test]
fn gpu_compositor_chroma_key_transition_and_nv12_readback() {
    let cfg = VideoConfig {
        canvas_width: 320,
        canvas_height: 240,
        output_width: 320,
        output_height: 240,
        fps_num: 30,
        fps_den: 1,
        ..Default::default()
    };
    let mut comp = GpuCompositor::new_headless(cfg).expect("Headless wgpu GPU must initialize");

    let mut col = SceneCollection::default_starter("GpuTest");
    let s1 = col.active_scene_id;
    let s2 = col.create_scene("Target").unwrap();

    // Base red background in Scene 1
    let (bg_id, _) = col
        .create_source_and_add(
            s1,
            "RedBg",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [220, 20, 20, 255],
                width: 320,
                height: 240,
            },
        )
        .unwrap();

    // Foreground pure-green source with ChromaKey filter -> should key out revealing red underneath!
    let (fg_id, _) = col
        .create_source_and_add(
            s1,
            "GreenKeyed",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [0, 255, 0, 255],
                width: 320,
                height: 240,
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

    // Replace s1 items cleanly with RedBg + GreenKeyed
    col.scene_by_id_mut(s1).unwrap().items = vec![SceneItem::new(bg_id), SceneItem::new(fg_id)];

    // Pure blue background in Scene 2
    let (blue_id, _) = col
        .create_source_and_add(
            s2,
            "BlueBg",
            SourceKind::ColorSource,
            SourceProperties::ColorSource {
                color_rgba: [20, 40, 230, 255],
                width: 320,
                height: 240,
            },
        )
        .unwrap();
    col.scene_by_id_mut(s2).unwrap().items = vec![SceneItem::new(blue_id)];

    let mut frames = HashMap::new();
    frames.insert(
        bg_id,
        VideoFrame::new_solid_rgba(320, 240, [220, 20, 20, 255], 1),
    );
    frames.insert(
        fg_id,
        VideoFrame::new_solid_rgba(320, 240, [0, 255, 0, 255], 2),
    );
    frames.insert(
        blue_id,
        VideoFrame::new_solid_rgba(320, 240, [20, 40, 230, 255], 3),
    );

    // 1. Render Scene 1 + NV12 readback
    let nv12 = comp
        .render_frame(&col, &frames, None, true, true, true)
        .expect("NV12 readback expected")
        .to_vec();
    assert_eq!(nv12.len(), 320 * 240 * 3 / 2);

    let rgba = comp.read_program_rgba().unwrap();
    let mid_idx = (120 * 320 + 160) * 4;
    let r = rgba[mid_idx];
    let g = rgba[mid_idx + 1];
    let b = rgba[mid_idx + 2];
    assert!(
        r > 180 && g < 60 && b < 60,
        "Expected ChromaKey to key green foreground revealing red background, got RGBA=({r},{g},{b})"
    );

    // 2. Render 50% Fade transition between Red Scene 1 and Blue Scene 2
    let tr_cfg = TransitionConfig {
        id: Default::default(),
        name: "Fade".into(),
        kind: TransitionKind::Fade,
        duration_ms: 300,
    };
    let active_tr = ActiveTransition::set_t_bar(tr_cfg, s1, s2, 0.5);
    let _ = comp.render_frame(&col, &frames, Some(&active_tr), false, false, false);
    let tr_rgba = comp.read_program_rgba().unwrap();
    let tr_r = tr_rgba[mid_idx];
    let tr_b = tr_rgba[mid_idx + 2];
    assert!(
        tr_r > 80 && tr_b > 80,
        "Expected 50% fade mix of red and blue, got R={tr_r}, B={tr_b}"
    );
    let _ = SourceId::new();
}
