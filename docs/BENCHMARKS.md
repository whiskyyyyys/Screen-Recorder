# OxideStudio (OBS-Rust) v0.1.0 — Performance Benchmarks

All benchmarks below were measured on real hardware using the release build (`--release`) via the reproducible suite in [crates/app/examples/bench_suite.rs](../crates/app/examples/bench_suite.rs) and live application execution of [obs-rust.exe](../target/release/obs-rust.exe).

> **Note on Official OBS Comparison**: Official C++ OBS Studio was not installed on this benchmark host, so no side-by-side OBS Studio numbers are fabricated or included here.

## Test Environment

| Component | Measured Value |
|---|---|
| Operating System | Microsoft Windows 11 Pro (Build 26200, 64-bit) |
| CPU | AMD Ryzen 5 6600H with Radeon Graphics (6 Cores / 12 Logical) |
| System Memory | 13.7 GB |
| Primary GPU | AMD Radeon(TM) Graphics (Driver `32.0.21045.26001`) |
| `wgpu` Active Backend | `Vulkan` (`AMD proprietary driver 26.11.1`) |
| Bundled FFmpeg | BtbN FFmpeg GPL Build (`N-129219-g359c825152-20261122`) |

### Reproduce Locally

```powershell
cargo run --release -p obs-rust --example bench_suite
```

## 1. GPU Compositor & Pixel Conversion (`1920x1080`)

Measured across 120 GPU frames compositing 2 1080p layers with an active Chroma Key WGSL filter pass, compared against CPU `obs_video::rgba_to_nv12` across 30 frames (Rec.709, Partial Range).

| Pipeline Stage | Iterations | Total Time | Time / Frame | Throughput | Notes |
|---|---|---|---|---|---|
| GPU 1080p Composite Only | 120 frames | 0.144 s | 1.20 ms | 835.9 FPS | `need_nv12_readback = false` |
| GPU 1080p Composite + WGSL NV12 + Readback | 120 frames | 0.425 s | 3.54 ms | 282.6 FPS | Compute conversion + staging readback |
| CPU 1080p RGBA to NV12 Conversion | 30 frames | 0.389 s | 12.98 ms | 77.1 FPS | Single-thread `rgba_to_nv12` |

- At 1080p60 (16.67 ms/frame budget), the full GPU composite + WGSL NV12 conversion + CPU readback consumes **3.54 ms/frame** (21.2% of a 60 FPS frame interval) and achieves **3.67x** higher throughput than CPU RGBA-to-NV12 conversion alone.

## 2. Audio Mixer & Real-Time DSP Engine (`48 kHz`, 6 Tracks)

Measured across 1,000 blocks of 480 samples (10 ms/block = **10.0 seconds** of stereo 48 kHz audio) mixing **4 active sources**, each running **5 stacked real-time filters** (`NoiseSuppression`, `NoiseGate`, `Gain`, `ThreeBandEq`, `Limiter`) across 6 output tracks:

| Workload | Total Processing Time | Mean Time / 10 ms Block | Real-Time CPU Share |
|---|---|---|---|
| 4 sources x 5 DSP filters -> 6-track mix | 60.95 ms | 61.0 us | 0.610% |

## 3. Video Encoder Throughput (`1920x1080 @ 60 FPS`, 6000 kbps CBR)

Each available encoder received 180 pre-generated 1080p NV12 frames (3.0 seconds of video) alongside 48 kHz stereo AAC audio through `RecordingSession` into MKV containers, then verified via `ffprobe`.

| Encoder ID | Kind | Available on Host | Total Time (180f) | Throughput | Realtime Factor | Output Size | Verified Codec |
|---|---|---|---|---|---|---|---|
| `h264_amf` | Hardware (AMD AMF) | Yes | 0.285 s | 630.5 FPS | 10.51x | 324 KB | `h264` |
| `hevc_amf` | Hardware (AMD AMF) | Yes | 0.265 s | 680.0 FPS | 11.33x | 258 KB | `hevc` |
| `h264_mf` | Hardware (Media Foundation) | Yes | 0.317 s | 567.4 FPS | 9.46x | 706 KB | `h264` |
| `libx264` | Software (x264) | Yes | 0.333 s | 540.7 FPS | 9.01x | 996 KB | `h264` |
| `libx265` | Software (x265) | Yes | 0.554 s | 325.1 FPS | 5.42x | 204 KB | `hevc` |
| `libsvtav1` | Software (SVT-AV1) | Yes | 0.423 s | 425.5 FPS | 7.09x | 57 KB | `av1` |

Encoders probed on this host that reported `available = false` during the 1-frame live driver check (hardware/driver absent on AMD Radeon 6600H laptop): `h264_nvenc`, `hevc_nvenc`, `av1_nvenc`, `av1_amf`, `h264_qsv`, `hevc_qsv`, `av1_qsv`, `hevc_mf`.

## 4. Application Footprint & Cold GUI Launch

| Metric | Measured Value |
|---|---|
| `obs-rust.exe` release binary size | 14,253,056 bytes (~13.6 MiB) |
| `sample_vignette_plugin.dll` size | 265,216 bytes (~259 KiB) |
| Non-system runtime DLL dependencies | None (static Rust + standard Windows 10/11 system/UCRT DLLs; bundled `ffmpeg.exe`/`ffprobe.exe` side-by-side) |
| Live GUI Working Set (4s after launch) | 512.32 MB |
| Live GUI Private Memory (4s after launch) | 501.69 MB |
