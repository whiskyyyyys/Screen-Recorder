# Third-Party Licenses and Attribution

OxideStudio (OBS-Rust) is free and open-source software distributed under the terms of the **GNU General Public License v2.0 (GPL-2.0)** — see [LICENSE](LICENSE).

## Architectural Reference & Compliance

- **OBS Studio** (`https://github.com/obsproject/obs-studio`)  
  - License: **GNU General Public License v2.0 (GPL-2.0)**
  - OxideStudio is an **independent Rust rebuild** inspired by and architected around the open-source capabilities, workflows, and protocol specifications of OBS Studio. It is **not affiliated with, endorsed by, or an official release of** the OBS Project.
  - Shader equations (Chroma Key YCbCr box filter, Color Key, Luma Key, Color Correction, Sharpen), fader curves (cubic / IEC 60-268-18), audio DSP behavior, and `services.json` streaming ingest definitions were studied from OBS Studio under GPL-2.0 and reimplemented in Rust/WGSL while preserving GPL-2.0 licensing on this project.
- **obs-websocket 5.x Protocol** (`https://github.com/obsproject/obs-websocket`)  
  - License: GPL-2.0
  - Protocol specification (OpCodes 0–9, SHA-256 + Base64 challenge-salt authentication, request/event schemas) implemented in pure Rust in `crates/websocket`.

## Bundled Runtime Tools

- **FFmpeg** (`ffmpeg.exe`, `ffprobe.exe` — BtbN FFmpeg GPL Win64 release build)
  - License: **GNU General Public License v3.0 / v2.0+** (with LGPL/GPL components such as `libx264`, `libx265`, `libsvtav1`, etc.)
  - Used for hardware/software media encoding, container muxing (MKV, MP4, Fragmented MP4, MOV, FLV), fast lossless remuxing, Replay Buffer cluster rebasing, Media Source decoding, and SRT transport fallback.
  - Distributed alongside `obs-rust.exe` with full source-availability notice (`https://ffmpeg.org` / `https://github.com/BtbN/FFmpeg-Builds`).

## Rust Crate Dependencies (from `crates.io`)

Major Rust dependencies used under permissive open-source licenses (MIT / Apache-2.0 / BSD-2-Clause / Zlib / ISC):
- `eframe`, `egui`, `egui-wgpu`, `egui_dock` (MIT / Apache-2.0)
- `wgpu`, `naga`, `winit` (MIT / Apache-2.0)
- `windows`, `windows-core` (MIT / Apache-2.0)
- `tokio`, `tokio-tungstenite`, `tokio-native-tls`, `native-tls`, `bytes`, `futures-util` (MIT / Apache-2.0)
- `rml_rtmp` (MIT)
- `serde`, `serde_json`, `uuid`, `chrono`, `thiserror`, `anyhow`, `tracing`, `parking_lot`, `crossbeam-channel`, `bytemuck`, `glam`, `sha2`, `base64`, `rand`, `url`, `directories`, `rfd`, `libloading` (MIT / Apache-2.0)
- `image` (MIT / Apache-2.0), `fontdue` (MIT / Apache-2.0 / Zlib)
