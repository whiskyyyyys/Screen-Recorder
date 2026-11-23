use obs_core::{ObsError, Result};
use std::path::PathBuf;

pub fn locate_ffmpeg() -> Result<PathBuf> {
    locate_tool("ffmpeg.exe").ok_or_else(|| {
        ObsError::Encoder(
            "ffmpeg.exe not found beside executable, in .tools/ffmpeg, bin/, or PATH".to_string(),
        )
    })
}

pub fn locate_ffprobe() -> Result<PathBuf> {
    locate_tool("ffprobe.exe").ok_or_else(|| {
        ObsError::Encoder(
            "ffprobe.exe not found beside executable, in .tools/ffmpeg, bin/, or PATH".to_string(),
        )
    })
}

fn locate_tool(name: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let direct = dir.join(name);
            if direct.exists() {
                return Some(direct);
            }
            let sub_ffmpeg = dir.join("ffmpeg").join(name);
            if sub_ffmpeg.exists() {
                return Some(sub_ffmpeg);
            }
        }
    }
    let cwd_tools = PathBuf::from(".tools").join("ffmpeg").join(name);
    if cwd_tools.exists() {
        return Some(cwd_tools);
    }
    let cwd_bin = PathBuf::from("bin").join(name);
    if cwd_bin.exists() {
        return Some(cwd_bin);
    }
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut cur = PathBuf::from(manifest_dir);
        for _ in 0..4 {
            let cand = cur.join(".tools").join("ffmpeg").join(name);
            if cand.exists() {
                return Some(cand);
            }
            if !cur.pop() {
                break;
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(name);
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}
