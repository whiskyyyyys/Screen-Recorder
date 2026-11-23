# OxideStudio (OBS-Rust)

**OxideStudio** is a native Windows 10/11 x64 live streaming and screen recording desktop studio rebuilt from the ground up in **Rust**. Inspired by the architecture and workflow of [OBS Studio](https://github.com/obsproject/obs-studio), OxideStudio combines a `wgpu` GPU compositor, Win32/DXGI/WASAPI/Media Foundation capture backends, real-time pure-Rust audio DSP, multi-track FFmpeg-backed recording, pure-Rust RTMP/RTMPS streaming, a dockable desktop UI, a C-ABI v1 dynamic plugin system, and an OBS WebSocket 5.x server inside a 19-crate Cargo workspace.

> **Honest Scope Statement (`v0.1.3`)**: Every subsystem listed as verified below is backed by real code and automated/live verification on Windows x64. Known differences from upstream C++ OBS Studio (no upstream C/C++ OBS `.dll` binary compatibility shim, no kernel/anti-cheat DLL swapchain hook injection, no native VST2/VST3 host, and headless Edge/Chrome frame capture instead of embedded CEF for Browser Source) are documented clearly in [docs/FEATURE_PARITY.md](docs/FEATURE_PARITY.md).

## Architecture Overview

OxideStudio is organized into 19 workspace crates plus a reference C-ABI plugin:

| Crate | Responsibility |
|---|---|
| `crates/core` | Strongly-typed IDs, error types, precision clock, statistics snapshots, lock-free event bus |
| `crates/video` | Video pixel formats, Rec.709/Rec.601/Rec.2100 color spaces, shared `VideoFrame` buffers, CPU NV12 helpers |
| `crates/settings` | Persistent JSON profiles & 9-tab settings model, portable-mode discovery |
| `crates/filters` | Video filter definitions + stateful pure-Rust audio DSP (Gain, Gate, Compressor, Limiter, Expander, Noise Suppression, 3-Band EQ, Polarity) |
| `crates/transitions` | Cut, Fade, Swipe, Slide, Fade-to-Color, Luma Wipe (`assets/luma_wipes/`), and Stinger state machines + T-Bar |
| `crates/scenes` | Unlimited scenes, scene collections, reference cloning, groups, DFS nesting cycle detection |
| `crates/audio` | 6-track `AudioMixer`, cubic fader curve, per-source sync delay lines, stereo/mono routing, VU peak/RMS/hold meters, monitor bus |
| `crates/platform-windows` | DXGI Desktop Duplication + GDI fallback, `PrintWindow`/GDI capture, non-invasive Game Capture, Media Foundation webcams & MFT probes, WASAPI Mic/Loopback/Process-PID capture & Monitor render, Win32 global hotkeys |
| `crates/capture` | Threaded capture orchestrator synchronizing platform capture workers |
| `crates/sources` | Text (`fontdue`), FFmpeg-decoded Media/Playlist sources, Image/Slideshow, Color, and Browser frame workers |
| `crates/renderer` | `wgpu` GPU compositor, multi-pass WGSL filter ping-pong pipeline, custom WGSL compilation, Studio Mode Preview/Program, Multiview grid, GPU RGBA-to-NV12 compute shader |
| `crates/encoder` | FFmpeg/FFprobe discovery & live 1-frame driver capability probing (`h264_nvenc`, `h264_amf`, `h264_qsv`, `h264_mf`, `libx264`, `libx265`, `libsvtav1`, etc.) |
| `crates/recorder` | Multi-track MKV/MP4/Fragmented MP4/MOV/FLV recording via Windows Named Pipes, pause/resume, lossless MP4 remux, EBML Cluster ring Replay Buffer |
| `crates/streamer` | Pure-Rust `rml_rtmp` + Schannel `tokio-native-tls` RTMP/RTMPS client, SRT via FFmpeg, 76 bundled OBS services (`assets/services.json`), auto-reconnect, multi-output |
| `crates/plugins` | Dynamic C-ABI v1 plugin loader with ABI version checks and video/audio/event hooks (`plugins/sample-vignette-plugin`) |
| `crates/websocket` | OBS WebSocket 5.x server (OpCodes 0–9, SHA-256 auth challenge, 40+ RPC requests, event PubSub) |
| `crates/ui` | `eframe` / `egui` / `egui_dock` desktop interface, interactive canvas transforms, Dark/Light/High-Contrast themes, all configuration modals |
| `crates/app` | `ObsEngine` orchestrator binary (`obs-rust.exe`), `--self-test` diagnostics, Windows `.rc`/COFF icon embedding, integration tests, and benchmarks |

Detailed architecture notes are available in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and OBS design notes in [docs/OBS_RESEARCH.md](docs/OBS_RESEARCH.md).

## Key Verified Capabilities (`v0.1.3`)

- **Scene Management & Studio Mode**: Unlimited scenes, scene collections with JSON import/export, scene nesting with cycle rejection, grouped items, source reference cloning, Studio Mode Preview/Program with manual T-Bar transitions, and dockable or pop-out Multiview.
- **GPU Compositing (`wgpu`)**: Hardware-accelerated compositing, crop/pad/rotate/scale transforms, Chroma/Color/Luma Key, Color Correction, Sharpen, Scroll, Image Mask/Blend, Render Delay, custom WGSL shaders, and GPU compute RGBA-to-NV12 conversion.
- **Multi-Track Recording & Replay Buffer**: Pause/resume support, 1–6 discrete audio tracks over Windows Named Pipes, MKV/MP4/Fragmented MP4/MOV/FLV containers, one-click lossless MKV-to-MP4 remux, and memory-bounded Matroska EBML Replay Buffer.
- **Live Streaming**: Native Rust RTMP/RTMPS client (`rml_rtmp` + Windows Schannel TLS), SRT output, 76 preconfigured streaming services, custom servers, automatic reconnection, and multi-destination output.
- **Audio Engine**: 44.1/48 kHz `AudioMixer` with 6 recording tracks, cubic volume faders, monitoring routing, audio sync offsets, VU meters, and 8 pure-Rust DSP filters.
- **WebSocket & Plugins**: OBS WebSocket 5.x server (`ws://127.0.0.1:4455`) with SHA-256 authentication plus C-ABI v1 native plugin loading (`plugins/sample_vignette_plugin.dll`).

## System Requirements

- **OS**: Windows 10 or Windows 11 (x64)
- **GPU**: DirectX 12 / Vulkan-compatible GPU for `wgpu` hardware acceleration (tested on AMD Radeon Graphics with Vulkan & DirectX 12)
- **Runtime Dependencies for End Users**: None. End users do **not** need Rust, Cargo, Python, or MinGW installed; `obs-rust.exe` links only standard Windows system/UCRT libraries and ships alongside bundled `ffmpeg.exe`, `ffprobe.exe`, `plugins/`, and `assets/`.

## Installation (Windows x64)

Release distributables are generated by [packaging/windows/package.ps1](packaging/windows/package.ps1) into `dist/`:

1. **Windows Installer (`OBS-Rust-Setup-x64.exe`)**: Double-click to install OxideStudio with Start Menu/Desktop shortcuts, bundled FFmpeg binaries, sample vignette plugin, and uninstaller.
2. **Portable Package (`OBS-Rust-Portable-x64.zip`)**: Extract anywhere and run `obs-rust.exe`. Includes `portable_mode.txt` so all settings, profiles, and scene collections remain self-contained inside the extracted folder.

To verify your installation from a terminal without launching the UI:

```powershell
.\obs-rust.exe --self-test
```

## Usage & Shortcuts

- **Canvas Interaction**: Drag sources to reposition, drag corner/edge handles to resize or crop (`Alt`-crop handles), drag the top rotation handle to spin, or select items in the **Sources** dock.
- **Studio Mode**: Toggle **Studio Mode** in **Controls** to edit in **Preview** while viewers stay on **Program**, then trigger transitions with **Transition** or drag the **T-Bar**.
- **WebSocket Control**: Enable and configure port/password in **Settings -> WebSocket Server (5.x)** and connect standard OBS WebSocket 5.x clients.
- **Global Hotkeys**: Bind keys for Start/Stop Recording, Pause/Resume Recording, Start/Stop Streaming, Save Replay, Studio Mode, and scene switching under **Settings -> Hotkeys**.

## Measured Performance Highlights (`1080p60`, AMD Ryzen 5 6600H / Radeon Graphics)

Full methodology and tables are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md):

- **GPU 1080p Composite Only** (`wgpu` Vulkan, 2 layers + Chroma Key): **1.20 ms/frame (835.9 FPS)**
- **GPU 1080p Composite + WGSL NV12 + Staging Readback**: **3.54 ms/frame (282.6 FPS)** vs. **12.98 ms/frame (77.1 FPS)** CPU NV12 conversion
- **Audio Mixer + 20 Active DSP Filters** (4 channels x 5 filters @ 48 kHz -> 6 tracks): **61.0 us / 10 ms block (0.610% real-time CPU)**
- **1080p60 Recording Throughput (180 frames + AAC audio)**:
  - `hevc_amf` (Hardware): **680.0 FPS (11.33x realtime)**
  - `h264_amf` (Hardware): **630.5 FPS (10.51x realtime)**
  - `h264_mf` (Hardware MF): **567.4 FPS (9.46x realtime)**
  - `libx264` (Software): **540.7 FPS (9.01x realtime)**
  - `libsvtav1` (Software): **425.5 FPS (7.09x realtime)**
  - `libx265` (Software): **325.1 FPS (5.42x realtime)**
- **Live GUI Working Set Memory (4s post-launch)**: **512.32 MB**

## Building & Testing From Source

Prerequisites on Windows x64:
- Rust stable toolchain (`1.85+` / `1.100.0` GNU tested with WinLibs MinGW-w64 UCRT, or MSVC toolchain)
- Optional: Inno Setup 6 (`ISCC.exe`) for building `OBS-Rust-Setup-x64.exe`
- Portable FFmpeg GPL build (`ffmpeg.exe`, `ffprobe.exe`) placed under `.tools/ffmpeg/`

```powershell
# Format check & strict Clippy lints
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Build release workspace & reference plugin DLL
cargo build --workspace --release

# Run all 5 integration test suites sequentially
cargo test --workspace -- --test-threads=1

# Run headless engine self-test (GPU, capture, encoders, 1s MKV recording)
.\target\release\obs-rust.exe --self-test

# Run release performance benchmarks
cargo run --release -p obs-rust --example bench_suite

# Build Portable ZIP + Inno Setup Installer into dist/
.\packaging\windows\package.ps1
```

## Licensing & Attribution

- OxideStudio source code is licensed under the **GNU General Public License v2.0** ([LICENSE](LICENSE)), preserving compatibility with GPL-licensed media tooling and OBS architectural heritage.
- Bundled FFmpeg GPL builds, luma wipe bitmaps (`assets/luma_wipes/`), service definitions (`assets/services.json`), and Rust crate licenses are documented in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
