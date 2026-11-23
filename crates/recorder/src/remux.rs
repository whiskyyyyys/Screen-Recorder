use obs_core::{ObsError, Result};
use obs_encoder::locate_ffmpeg;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Losslessly remuxes an MKV/FLV/MOV recording into fast-start MP4 (`-c copy -map 0`).
pub fn remux_to_mp4(input_path: &Path, output_path: Option<&Path>) -> Result<PathBuf> {
    if !input_path.exists() {
        return Err(ObsError::Recorder(format!(
            "Remux source file does not exist: {}",
            input_path.display()
        )));
    }
    let dst = match output_path {
        Some(p) => p.to_path_buf(),
        None => input_path.with_extension("mp4"),
    };
    if dst == input_path {
        return Err(ObsError::Recorder(
            "Destination path matches source path for remux".to_string(),
        ));
    }

    let ffmpeg = locate_ffmpeg()?;
    let mut cmd = Command::new(ffmpeg);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &input_path.to_string_lossy(),
            "-map",
            "0",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            &dst.to_string_lossy(),
        ])
        .output()?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(ObsError::Recorder(format!("Remux failed: {err}")));
    }
    Ok(dst)
}
