use crate::ffmpeg_locator::locate_ffmpeg;
use obs_platform_windows::{probe_mf_video_encoders, MfEncoderInfo};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::H264 => "H.264 / AVC",
            Self::Hevc => "HEVC / H.265",
            Self::Av1 => "AV1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncoderVendor {
    Nvidia,
    Amd,
    Intel,
    Microsoft,
    Software,
}

impl EncoderVendor {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA NVENC",
            Self::Amd => "AMD AMF",
            Self::Intel => "Intel QSV",
            Self::Microsoft => "Windows Media Foundation",
            Self::Software => "Software (CPU)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoEncoderInfo {
    pub id: String,
    pub display_name: String,
    pub codec: VideoCodec,
    pub vendor: EncoderVendor,
    pub hardware: bool,
    pub available: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEncoderInfo {
    pub id: String,
    pub display_name: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EncoderProbeReport {
    pub ffmpeg_found: bool,
    pub ffmpeg_path: String,
    pub video_encoders: Vec<VideoEncoderInfo>,
    pub audio_encoders: Vec<AudioEncoderInfo>,
    pub mf_hardware_mfts: Vec<MfEncoderInfo>,
}

/// Probes real encoder availability by executing a 1-frame encode test against the OS/GPU driver.
/// Never marks an unverified hardware encoder as available.
pub fn probe_all_encoders() -> EncoderProbeReport {
    let mf_hardware_mfts = probe_mf_video_encoders();
    let Ok(ffmpeg) = locate_ffmpeg() else {
        return EncoderProbeReport {
            ffmpeg_found: false,
            ffmpeg_path: String::new(),
            video_encoders: Vec::new(),
            audio_encoders: Vec::new(),
            mf_hardware_mfts,
        };
    };

    let candidates = [
        (
            "h264_nvenc",
            "NVIDIA NVENC H.264",
            VideoCodec::H264,
            EncoderVendor::Nvidia,
            true,
        ),
        (
            "hevc_nvenc",
            "NVIDIA NVENC HEVC",
            VideoCodec::Hevc,
            EncoderVendor::Nvidia,
            true,
        ),
        (
            "av1_nvenc",
            "NVIDIA NVENC AV1",
            VideoCodec::Av1,
            EncoderVendor::Nvidia,
            true,
        ),
        (
            "h264_amf",
            "AMD HW H.264 (AMF)",
            VideoCodec::H264,
            EncoderVendor::Amd,
            true,
        ),
        (
            "hevc_amf",
            "AMD HW HEVC (AMF)",
            VideoCodec::Hevc,
            EncoderVendor::Amd,
            true,
        ),
        (
            "av1_amf",
            "AMD HW AV1 (AMF)",
            VideoCodec::Av1,
            EncoderVendor::Amd,
            true,
        ),
        (
            "h264_qsv",
            "Intel QuickSync H.264",
            VideoCodec::H264,
            EncoderVendor::Intel,
            true,
        ),
        (
            "hevc_qsv",
            "Intel QuickSync HEVC",
            VideoCodec::Hevc,
            EncoderVendor::Intel,
            true,
        ),
        (
            "av1_qsv",
            "Intel QuickSync AV1",
            VideoCodec::Av1,
            EncoderVendor::Intel,
            true,
        ),
        (
            "h264_mf",
            "Windows Media Foundation H.264",
            VideoCodec::H264,
            EncoderVendor::Microsoft,
            true,
        ),
        (
            "hevc_mf",
            "Windows Media Foundation HEVC",
            VideoCodec::Hevc,
            EncoderVendor::Microsoft,
            true,
        ),
        (
            "libx264",
            "x264 (Software H.264)",
            VideoCodec::H264,
            EncoderVendor::Software,
            false,
        ),
        (
            "libx265",
            "x265 (Software HEVC)",
            VideoCodec::Hevc,
            EncoderVendor::Software,
            false,
        ),
        (
            "libsvtav1",
            "SVT-AV1 (Software AV1)",
            VideoCodec::Av1,
            EncoderVendor::Software,
            false,
        ),
    ];

    let mut video_encoders = Vec::with_capacity(candidates.len());
    for (id, display_name, codec, vendor, hardware) in candidates {
        let (available, reason) = test_video_encoder(&ffmpeg, id);
        video_encoders.push(VideoEncoderInfo {
            id: id.to_string(),
            display_name: display_name.to_string(),
            codec,
            vendor,
            hardware,
            available,
            reason,
        });
    }

    let audio_candidates = [("aac", "FFmpeg AAC"), ("libopus", "Opus")];
    let mut audio_encoders = Vec::with_capacity(audio_candidates.len());
    for (id, name) in audio_candidates {
        let available = test_audio_encoder(&ffmpeg, id);
        audio_encoders.push(AudioEncoderInfo {
            id: id.to_string(),
            display_name: name.to_string(),
            available,
        });
    }

    EncoderProbeReport {
        ffmpeg_found: true,
        ffmpeg_path: ffmpeg.to_string_lossy().to_string(),
        video_encoders,
        audio_encoders,
        mf_hardware_mfts,
    }
}

pub fn resolve_best_video_encoder(requested: &str, report: &EncoderProbeReport) -> String {
    if requested != "auto" && !requested.trim().is_empty() {
        if let Some(found) = report
            .video_encoders
            .iter()
            .find(|e| e.id == requested && e.available)
        {
            return found.id.clone();
        }
    }
    // Prefer verified H.264 hardware encoder first, then libx264 for broad compatibility
    let preference = [
        "h264_nvenc",
        "h264_amf",
        "h264_qsv",
        "h264_mf",
        "libx264",
        "hevc_nvenc",
        "hevc_amf",
        "libx265",
    ];
    for p in preference {
        if report
            .video_encoders
            .iter()
            .any(|e| e.id == p && e.available)
        {
            return p.to_string();
        }
    }
    "libx264".to_string()
}

fn test_video_encoder(ffmpeg: &std::path::Path, encoder_id: &str) -> (bool, String) {
    let mut cmd = Command::new(ffmpeg);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=256x256:r=30:d=0.05",
            "-pix_fmt",
            "nv12",
            "-frames:v",
            "1",
            "-c:v",
            encoder_id,
            "-f",
            "null",
            "-",
        ])
        .output();

    match out {
        Ok(res) if res.status.success() => (true, "Verified".to_string()),
        Ok(res) => {
            let err = String::from_utf8_lossy(&res.stderr).trim().to_string();
            let short = err.lines().last().unwrap_or("Unavailable").to_string();
            (false, short)
        }
        Err(e) => (false, format!("Probe error: {e}")),
    }
}

fn test_audio_encoder(ffmpeg: &std::path::Path, encoder_id: &str) -> bool {
    let mut cmd = Command::new(ffmpeg);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo:d=0.05",
        "-frames:a",
        "1",
        "-c:a",
        encoder_id,
        "-f",
        "null",
        "-",
    ])
    .output()
    .map(|r| r.status.success())
    .unwrap_or(false)
}
