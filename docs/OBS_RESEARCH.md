# Phase 1: OBS Studio Architecture Research

This document captures the technical findings from studying the official `obsproject/obs-studio` repository (`master` branch) prior to designing and implementing OxideStudio (OBS-Rust).

## 1. Repository & Subsystem Breakdown

| OBS Directory | Language | Responsibilities | Rust Rebuild Mapping |
|---|---|---|---|
| `libobs/` | C99 | Core context, scene graph (`obs-scene.c`), source abstraction (`obs-source.c`), audio mixer & resampler (`obs-audio.c`, `media-io/`), video output thread (`obs-video.c`), graphics abstraction (`graphics/`), hotkeys, module loader (`obs-module.c`), encoder/output/service primitives | `crates/core`, `crates/scenes`, `crates/audio`, `crates/video`, `crates/settings`, `crates/plugins` |
| `libobs-d3d11/` | C++ | Direct3D 11 graphics subsystem, swapchains, `.effect` shader execution, shared GPU texture encoding | `crates/renderer` (`wgpu` on D3D12/D3D11/Vulkan) |
| `libobs-winrt/` | C++/WinRT | Windows Graphics Capture (`Windows.Graphics.Capture`) helper DLL | `crates/platform-windows` (DXGI Output Duplication + PrintWindow + GDI + MediaFoundation + WASAPI) |
| `frontend/` | C++ / Qt6 | Main window, dockable panels, Studio Mode, Multiview, Settings dialogs, Profiles, Scene Collections, Auto-Config Wizard, Remux tool | `crates/ui` (`eframe` / `egui` / `egui_dock` / `egui_wgpu`) + `crates/app` |
| `plugins/win-capture/` | C | Display capture (`duplicator-monitor-capture.c`, `monitor-capture.c`), Window capture (`window-capture.c`), Game capture (`game-capture.c` + `graphics-hook`), Application Audio Capture | `crates/platform-windows` + `crates/capture` |
| `plugins/win-dshow/` | C++ | DirectShow webcam/capture card enumeration and capture | `crates/platform-windows` (Media Foundation `IMFSourceReader` video capture) |
| `plugins/win-wasapi/` | C++ | WASAPI mic input, desktop loopback, per-PID process loopback | `crates/platform-windows` (`wasapi.rs`) |
| `plugins/obs-filters/` | C / HLSL | Chroma Key, Color Key, Luma Key, Color Correction, Crop/Pad, Scale/Aspect, Scroll, Sharpen, Image Mask/Blend, GPU Delay, Gain, Noise Gate, Compressor, Limiter, Expander, Noise Suppression, EQ, Polarity | `crates/filters` (WGSL GPU shaders + real-time pure-Rust audio DSP) |
| `plugins/obs-transitions/` | C / HLSL | Cut, Fade, Swipe, Slide, Fade to Color, Luma Wipe, Stinger | `crates/transitions` + `crates/renderer` |
| `plugins/obs-outputs/` | C | RTMP/RTMPS (`librtmp` + FLV mux), MP4/Fragmented MP4/MOV muxers, Null output | `crates/streamer` (`rml_rtmp` + `tokio-native-tls`), `crates/recorder` |
| `plugins/obs-ffmpeg/` | C / C++ | NVENC, AMD AMF, Intel QSV, VAAPI, x264, SVT-AV1, AAC/Opus, Media Source (`obs-ffmpeg-source.c`), SRT/RIST, `ffmpeg-mux` helper process | `crates/encoder`, `crates/recorder`, `crates/sources` |
| `plugins/image-source/` | C | Image source, Image Slideshow, Color source | `crates/sources` |
| `plugins/obs-text/` | C++ | GDI+ text source & FreeType2 text source | `crates/sources` (`fontdue` + Windows system TTF fonts) |
| `plugins/vlc-video/` | C | Playlist video source | `crates/sources` (`PlaylistSource` via FFmpeg) |
| `plugins/obs-browser/` | C++ / CEF | Chromium Embedded Framework web source | `crates/sources` (Microsoft Edge / WebView2 headless capture) |
| `plugins/rtmp-services/` | JSON / C | `services.json` streaming service presets and ingest servers | `assets/services.json` + `crates/streamer` |
| `plugins/obs-websocket/` | C++ | OBS WebSocket 5.x RPC server (OpCodes 0–9, SHA-256 auth, PubSub events) | `crates/websocket` (pure Rust `tokio-tungstenite`) |

## 2. Key Architectural Patterns Observed in `obs-studio`

1. **Decoupled Render vs. Encode Cadence (`libobs/obs-video.c`)**:
   - Sources produce frames asynchronously on capture threads into latest-frame slots.
   - A fixed-cadence video compositor renders the active scene graph at configured FPS (`fps_num / fps_den`) onto the GPU canvas texture, applies source filters, scales to output resolution, and passes frames to active encoders.
2. **Global Source Reference Model (`obs-source.c`, `obs-scene.c`)**:
   - Sources are owned globally by name/UUID. Scene items (`obs_sceneitem_t`) hold transforms (position, scale, rotation, crop, bounds, blend mode, opacity, visibility, lock) referencing a shared `obs_source_t`, enabling true reference cloning across multiple scenes and groups.
3. **Multi-Track Audio Mixing (`obs-audio.c`, `obs-audio-controls.h`)**:
   - Each audio source produces floating-point PCM, passes through its audio filter chain (`Gain -> Noise Suppression -> Noise Gate -> Expander -> Compressor -> Limiter`), applies sync offset delay, updates stereo peak/RMS meters (cubic or IEC 60-268-18 fader scaling), and routes into a 6-bit track mask (`MAX_AUDIO_MIXES = 6`) plus an optional local monitoring output.
4. **Separate Muxer Process Pattern (`plugins/obs-ffmpeg/ffmpeg-mux`)**:
   - Even upstream OBS Studio spawns an external `obs-ffmpeg-mux` process fed over pipes for advanced muxing isolation. In OxideStudio, `crates/recorder` adopts a clean piped FFmpeg muxer architecture (using Windows Named Pipes for 1–6 independent audio tracks + stdin for NV12 video frames) while keeping RTMP/RTMPS streaming (`rml_rtmp` + Schannel TLS) and Matroska Replay Buffer ring parsing (`EBML Cluster` parser) natively in Rust.
5. **Licensing Constraints**:
   - `obs-studio` is GPL-2.0 (`COPYING`). FFmpeg Essentials builds with `libx264`/`libx265` are GPL-licensed. OxideStudio adopts **GPL-2.0** (`LICENSE`) with clear third-party attributions (`THIRD_PARTY_LICENSES.md`) and avoids claiming official OBS endorsement or binary compatibility with compiled C/C++ OBS `.dll` plugins.
