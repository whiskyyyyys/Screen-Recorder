# Phase 2: OxideStudio (OBS-Rust) Architecture

## 1. Cargo Workspace Topology

```text
obs-rust/
├── Cargo.toml
├── assets/
│   ├── icons/obs-rust.ico, obs-rust.png
│   └── services.json
├── crates/
│   ├── core/              # obs-core: typed IDs, monotonic clock, broadcast events, live stats, errors
│   ├── video/             # obs-video: pixel formats (RGBA/BGRA/NV12/I420), color spaces, scale filters, VideoFrame
│   ├── settings/          # obs-settings: 9-category persistent settings, Profiles, Scene Collections
│   ├── filters/           # obs-filters: Video filter definitions & pure-Rust real-time Audio DSP processors
│   ├── transitions/       # obs-transitions: Cut, Fade, Swipe, Slide, FadeToColor, LumaWipe, Stinger + T-Bar
│   ├── scenes/            # obs-scenes: shared source registry, scenes, groups, transforms, cycle-safe nesting
│   ├── audio/             # obs-audio: 6-track f32 mixer, sync delay ring buffer, VU peak/RMS meters, monitor bus
│   ├── platform-windows/  # obs-platform-windows: Win32 DXGI Duplication, PrintWindow/GDI, MF Webcam, WASAPI, Hotkeys
│   ├── capture/           # obs-capture: thread-safe capture managers & latest-frame lock-free slots
│   ├── sources/           # obs-sources: Color, Image, Slideshow, Text (fontdue), Media/Playlist, Browser, Capture bridges
│   ├── renderer/          # obs-renderer: wgpu GPU compositor, WGSL filters, Studio Mode, Multiview, GPU RGBA->NV12
│   ├── encoder/           # obs-encoder: FFmpeg locator, real 1-frame HW/SW encoder prober (AMF/NVENC/QSV/MF/x264/AV1)
│   ├── recorder/          # obs-recorder: Multi-track MKV/MP4/fMP4/MOV recorder, Pause/Resume, Remux, EBML Replay Buffer
│   ├── streamer/          # obs-streamer: Pure-Rust RTMP/RTMPS (rml_rtmp + Schannel), SRT, services, multi-output
│   ├── plugins/           # obs-plugins: Stable C-ABI v1 dynamic plugin loader (.dll discovery, version check, hooks)
│   ├── websocket/         # obs-websocket: Pure-Rust OBS WebSocket 5.x server (OpCodes 0-9, SHA-256 auth, 40+ RPCs)
│   ├── ui/                # obs-ui: eframe/egui/egui_dock desktop UI, zero-copy wgpu canvas, Studio Mode, 9-tab Settings
│   └── app/               # obs-rust: main binary orchestrating ObsEngine, threads, Tokio runtime, --self-test CLI
├── plugins/
│   └── sample-vignette-plugin/  # Working dynamic Rust plugin DLL implementing C-ABI v1 video filter
├── packaging/windows/     # Inno Setup 6 installer script & portable ZIP packager
├── tests/                 # End-to-end workspace integration test suite
└── .github/workflows/     # Windows CI (fmt, clippy, test, package, release)
```

## 2. Threading & Zero-Copy Pipeline

```text
  [DXGI / Window / Webcam / Media Threads] ──Arc<VideoFrame>──▶ [GpuCompositor (wgpu)]
                                                                   │         │
                                            (Zero-Copy TextureView)│         │ (NV12 GPU shader -> staging buffer
                                                                   ▼         │  ONLY when Rec/Stream/Replay active)
                                                            [egui_wgpu UI]   ▼
                                                                      [Encoder / Fan-out]
                                                                       ├──▶ [Recorder (Named Pipes -> MKV/MP4/fMP4)]
  [WASAPI / App Loopback / Media Audio] ──crossbeam──▶ [AudioMixer] ───┤──▶ [Replay Buffer (EBML Cluster Ring)]
                                                           │           └──▶ [Streamer (rml_rtmp/RTMPS/SRT Multi-Output)]
                                                           ▼
                                                  [WASAPI Monitor Out]
```

## 3. Technical Decisions & Documented FFI Boundaries

1. **GPU Compositor (`wgpu` over raw Direct3D11/HLSL)**:
   - On Windows, `wgpu 24` binds to Direct3D 12 / Direct3D 11 / Vulkan natively and integrates directly into `eframe`'s `egui_wgpu` render state, allowing the Preview and Program canvases to render directly in the UI viewport without copying rendered frames back through system RAM.
   - GPU-to-CPU readback happens via an async staging buffer running a GPU RGBA→NV12 shader (halving readback bandwidth from 4 bytes/px to 1.5 bytes/px) and is skipped entirely when neither recording, streaming, nor replay buffering is active.
2. **Capture Layer (`windows` 0.58 Win32/COM/DXGI FFI)**:
   - Pure Rust calls Win32 COM directly for DXGI Output Duplication, `PrintWindow` / GDI window capture, Media Foundation `IMFSourceReader` webcam capture, WASAPI device/loopback capture, and Windows 10/11 `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` per-application audio capture.
   - **Game Capture**: Uses non-invasive high-FPS cropped DXGI duplication with `PrintWindow` fallback rather than kernel/anti-cheat-sensitive DLL injection into DirectX swapchains.
3. **Encoding, Muxing & Replay Buffer**:
   - Pure-Rust hardware/software encoder probing verifies real 1-frame encoding at startup so the UI never advertises unusable encoders.
   - Recording and media decoding leverage bundled FFmpeg (`ffmpeg.exe` / `ffprobe.exe`) via isolated pipes (Windows Named Pipes for up to 6 independent audio tracks + stdin NV12 frames) so codec/container patents and massive C++ muxer maintenance stay cleanly decoupled while supporting NVENC, AMD AMF, Intel QSV, Media Foundation, x264, x265, SVT-AV1, AAC, and Opus.
   - Live RTMP/RTMPS publishing uses pure Rust (`rml_rtmp` + `tokio-native-tls` Windows Schannel) for low-latency async frame packetization and dropped-frame telemetry; Replay Buffer uses a pure-Rust EBML Matroska Cluster ring buffer.
4. **Plugin ABI**:
   - OxideStudio implements a clean, versioned C-ABI v1 dynamic plugin loader (`obs-plugins`). It does **not** load compiled C/C++ OBS Studio `.dll` binary plugins directly (which depend on upstream `libobs` ABI).
