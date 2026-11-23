use obs_platform_windows::{
    enumerate_audio_devices, enumerate_monitors, enumerate_windows, spawn_test_capture_window,
    AudioEndpointKind, DisplayCapturer, WindowCapturer,
};
use obs_plugins::PluginManager;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn windows_display_capture_and_device_enumeration() {
    let monitors = enumerate_monitors();
    assert!(!monitors.is_empty(), "Expected at least 1 Windows monitor");

    let mut capturer =
        DisplayCapturer::new(0, true, false).expect("DisplayCapturer initialization failed");
    let frame = capturer
        .capture_frame(100, 0)
        .expect("capture_frame failed")
        .expect("Initial display capture frame must be Some");
    assert_eq!(frame.width, monitors[0].width);
    assert_eq!(frame.height, monitors[0].height);
    assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);

    // Spawn a real Win32 target window so enumeration & PrintWindow/BitBlt capture work
    // reliably even inside isolated/headless CI desktop sessions.
    let target_title = "OxideStudio Window Capture Target";
    let _test_win = spawn_test_capture_window(target_title)
        .expect("Failed to spawn Win32 target window for capture test");
    std::thread::sleep(Duration::from_millis(80));

    let wins = enumerate_windows();
    assert!(
        wins.iter()
            .any(|w| w.title == target_title && w.width >= 100 && w.height >= 100),
        "Expected spawned Win32 window '{target_title}' in enumerate_windows(); got {:?}",
        wins.iter().map(|w| &w.title).collect::<Vec<_>>()
    );

    let mut win_cap = WindowCapturer::new(target_title.to_string(), String::new(), false, true);
    let mut win_frame = None;
    for attempt in 0..6u64 {
        if let Some(f) = win_cap
            .capture_frame((attempt + 1) * 33_333)
            .expect("WindowCapturer::capture_frame errored")
        {
            win_frame = Some(f);
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let wf = win_frame.expect("Expected WindowCapturer to capture spawned Win32 window");
    assert!(wf.width >= 100 && wf.height >= 100);
    assert_eq!(wf.data.len(), (wf.width * wf.height * 4) as usize);
    let center_idx = (((wf.height / 2) * wf.width + (wf.width / 2)) * 4) as usize;
    let (r, g, b, a) = (
        wf.data[center_idx],
        wf.data[center_idx + 1],
        wf.data[center_idx + 2],
        wf.data[center_idx + 3],
    );
    assert!(
        g > 150 && r < 80 && b < 80 && a == 255,
        "Expected solid green captured window pixel, got rgba=({r}, {g}, {b}, {a})"
    );

    let _inputs = enumerate_audio_devices(AudioEndpointKind::Input);
    let _outputs = enumerate_audio_devices(AudioEndpointKind::Output);
}

#[test]
fn dynamic_plugin_dll_loads_verifies_abi_and_processes_rgba() {
    let dll_candidates = [
        PathBuf::from("../../target/debug/sample_vignette_plugin.dll"),
        PathBuf::from("../../target/release/sample_vignette_plugin.dll"),
        PathBuf::from("target/debug/sample_vignette_plugin.dll"),
        PathBuf::from("target/release/sample_vignette_plugin.dll"),
    ];
    let Some(dll) = dll_candidates.into_iter().find(|p| p.exists()) else {
        panic!("sample_vignette_plugin.dll must be built before running plugin loader test");
    };

    let mgr = PluginManager::new();
    mgr.load_dll(&dll, &[], &HashMap::new());
    let list = mgr.list_plugins();
    assert_eq!(list.len(), 1);
    assert!(list[0].loaded, "Plugin failed to load: {:?}", list[0].error);
    assert_eq!(list[0].id, "oxidestudio.sample.vignette");

    let mut buf = vec![200u8; 64 * 64 * 4];
    mgr.process_video_filter(
        "oxidestudio.sample.vignette",
        "cinema_vignette",
        &serde_json::json!({ "strength": 0.9, "radius": 0.7 }),
        &mut buf,
        64,
        64,
    );

    // Center pixel remains bright; corner pixel gets darkened by vignette!
    let center = buf[(32 * 64 + 32) * 4];
    let corner = buf[0];
    assert!(
        center > corner + 40,
        "Expected vignette plugin to darken corners ({corner}) vs center ({center})"
    );
}
