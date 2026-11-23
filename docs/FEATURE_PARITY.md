# OxideStudio (OBS-Rust) — Feature Parity Matrix

> **Honesty Policy**: Features are marked **Verified** only when backed by real implementation AND passing automated/live verification on Windows x64. Features with narrower scope than upstream C++ OBS Studio are marked **Partial**, and unimplemented features remain **Planned** (not claimed in v0.1.1).

## Summary Legend
- **Verified**: Real implementation tested end-to-end (unit/integration tests, `--self-test`, live GUI run, and/or driver probe).
- **Partial**: Functional implementation with explicit scope boundaries documented in Notes.
- **Planned**: Not implemented in `v0.1.1`.

## Subsystem Parity Status (`v0.1.1`)

| Category | Feature | Status | Notes & Evidence |
|---|---|---|---|
| Scene Management | Unlimited scenes, CRUD, rename, duplicate, reorder | Verified | `obs-scenes` CRUD; `scene_and_audio_tests` |
| Scene Management | Scene Collections & JSON import/export | Verified | `SceneCollection::save_to_file` / `load_from_file`; `scene_and_audio_tests` |
| Scene Management | Scene nesting with cycle protection | Verified | DFS cycle rejection verified (`A -> B -> C -> A` rejected) |
| Scene Management | Studio Mode (Preview / Program + T-Bar) | Verified | `GpuCompositor` Preview + Program targets + T-Bar transition UI |
| Scene Management | Multiview (Dock + Multi-monitor window) | Verified | `GpuCompositor::render_multiview` + `egui_dock` tab & pop-out viewport |
| Scene Management | Hotkeys (global Windows hotkeys) | Verified | `HotkeyListener` via Win32 `GetAsyncKeyState` + settings bindings |
| Sources | Display Capture (DXGI Duplication + GDI fallback) | Verified | Tested against live Windows monitor in `windows_capture_and_plugins_tests` |
| Sources | Window Capture (`PrintWindow` + GDI fallback) | Verified | Verified cross-thread Win32 window enumeration & RGBA pixel readback |
| Sources | Game Capture (non-invasive DXGI / Window capture) | **Partial** | Captures fullscreen DXGI/PrintWindow targets without kernel drivers or anti-cheat DLL swapchain hook injection |
| Sources | Video Capture Device (Media Foundation Webcam) | Verified | `obs-platform-windows` `IMFSourceReader` enumeration & frame worker |
| Sources | Audio Input / Output / Application Audio Capture | Verified | WASAPI Mic/Loopback + `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` per-PID; device enum tested |
| Sources | Image, Image Slideshow, Color, Text (`fontdue`) | Verified | Loaded/rasterized into `VideoFrame` buffers; Windows font discovery |
| Sources | Media Source & Playlist Source | Verified | FFmpeg/FFprobe decode loop with seek/loop/playlist ordering |
| Sources | Browser Source | **Partial** | Headless Edge/Chrome periodic frame capture; not an interactive embedded CEF instance with DOM audio routing |
| Sources | Scene Source, Group Source, Reference Cloning | Verified | Shared `SourceId` references + group containment verified in `scene_and_audio_tests` |
| Video Engine | `wgpu` GPU Compositor, Blend Modes, Transforms, Crop | Verified | Interactive drag/resize/crop/rotate + GPU pipeline (`gpu_renderer_tests`) |
| Video Engine | Color Spaces (`Rec.709`/`Rec.601`/`Rec.2100`), Ranges, Scaling | Verified | WGSL shaders + compute RGBA→NV12 verified in `gpu_renderer_tests` and `bench_suite` |
| Recording | MKV, MP4, Fragmented MP4, MOV, FLV, Pause/Resume | Verified | Verified with real MKV, fMP4, pause/resume & `ffprobe` inspection (`recording_and_replay_tests`) |
| Recording | Multi-track Audio (1–6 tracks) & Lossless Auto-Remux | Verified | Windows Named Pipes for 6 tracks; 2-track MKV & MKV->MP4 remux verified via `ffprobe` |
| Recording | Replay Buffer (Memory-bounded EBML Cluster Ring) | Verified | Real EBML Matroska Cluster ring buffer & `save_replay` verified via `ffprobe` |
| Encoders | Hardware (AMF / MF / NVENC / QSV) & SW (`x264`/`x265`/`SVT-AV1`) | Verified | Live 1-frame driver probe (`h264_amf`, `hevc_amf`, `h264_mf`, `libx264`, `libx265`, `libsvtav1` benchmarked at 1080p60 on host) |
| Streaming | RTMP / RTMPS (pure Rust) + SRT + Services + Multi-Output | Verified | `rml_rtmp` + Schannel TLS; live loopback RTMP publish test passed (`streaming_and_websocket_tests`) |
| Audio Mixer | 6-Track Routing, VU Peak/RMS Meters, Sync Offset, Monitor | Verified | Cubic fader, delay ring buffer, track routing & meters (`scene_and_audio_tests`) |
| Audio Filters | Gain, Gate, Compressor, Limiter, Expander, Noise Supp, EQ, Polarity | Verified | Stateful pure-Rust DSP; verified ceiling, gate & sync delay (`scene_and_audio_tests`) |
| Audio Plugins | Native VST2/VST3 Plugin Hosting | **Planned** | Not implemented in `v0.1.1` |
| Video Filters | Chroma/Color/Luma Key, Color Corr, Crop, Scale, Scroll, Sharpen, Mask, Delay, Custom WGSL | Verified | Multi-pass ping-pong filter chain + Chroma Key verified (`gpu_renderer_tests`) |
| Transitions | Cut, Fade, Swipe, Slide, FadeToColor, LumaWipe, Stinger | Verified | 35 luma wipe assets bundled; Fade/Swipe/LumaWipe verified in `gpu_renderer_tests` |
| UI & Settings | Dockable OBS Layout, Dark/Light/A11y Themes, 9-Tab Settings, Profiles | Verified | `eframe`/`egui`/`egui_dock`; settings & profile persistence verified (`scene_and_audio_tests`); live GUI verified |
| Plugins | OxideStudio Rust C-ABI v1 Dynamic Plugins | Verified | `sample_vignette_plugin.dll` loaded, ABI-checked & processed RGBA (`windows_capture_and_plugins_tests`) |
| Plugins | Upstream C/C++ OBS `.dll` Binary Compatibility Shim | **Planned** | Not implemented or claimed in `v0.1.1` |
| WebSocket | OBS WebSocket 5.x (OpCodes 0–9, SHA-256 auth, 40+ RPCs, Events) | Verified | Live WebSocket SHA-256 handshake & scene/record/audio RPCs tested (`streaming_and_websocket_tests`) |
