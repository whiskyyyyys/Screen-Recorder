use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceServer {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRecommended {
    #[serde(rename = "keyint")]
    pub keyint: Option<u32>,
    #[serde(rename = "max video bitrate")]
    pub max_video_bitrate: Option<u32>,
    #[serde(rename = "max audio bitrate")]
    pub max_audio_bitrate: Option<u32>,
    #[serde(rename = "output")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingServiceEntry {
    pub name: String,
    #[serde(default)]
    pub common: bool,
    pub servers: Vec<ServiceServer>,
    pub recommended: Option<ServiceRecommended>,
}

#[derive(Debug, Clone, Deserialize)]
struct ServicesFile {
    services: Vec<StreamingServiceEntry>,
}

pub fn load_streaming_services() -> Vec<StreamingServiceEntry> {
    let mut candidates = vec![PathBuf::from("assets").join("services.json")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.insert(0, dir.join("assets").join("services.json"));
        }
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut p = PathBuf::from(manifest);
        for _ in 0..4 {
            candidates.push(p.join("assets").join("services.json"));
            if !p.pop() {
                break;
            }
        }
    }

    for p in candidates {
        if p.exists() {
            if let Ok(raw) = std::fs::read_to_string(&p) {
                if let Ok(parsed) = serde_json::from_str::<ServicesFile>(&raw) {
                    return parsed.services;
                }
            }
        }
    }

    // Minimal fallback if assets/services.json is missing
    let _ = HashMap::<String, String>::new();
    vec![
        StreamingServiceEntry {
            name: "Twitch".to_string(),
            common: true,
            servers: vec![ServiceServer {
                name: "Auto (Recommended)".to_string(),
                url: "rtmp://live.twitch.tv/app".to_string(),
            }],
            recommended: Some(ServiceRecommended {
                keyint: Some(2),
                max_video_bitrate: Some(6000),
                max_audio_bitrate: Some(320),
                output: None,
            }),
        },
        StreamingServiceEntry {
            name: "YouTube - RTMPS".to_string(),
            common: true,
            servers: vec![
                ServiceServer {
                    name: "Primary YouTube ingest server".to_string(),
                    url: "rtmps://a.rtmps.youtube.com:443/live2".to_string(),
                },
                ServiceServer {
                    name: "Backup YouTube ingest server".to_string(),
                    url: "rtmps://b.rtmps.youtube.com:443/live2?backup=1".to_string(),
                },
            ],
            recommended: Some(ServiceRecommended {
                keyint: Some(2),
                max_video_bitrate: Some(51000),
                max_audio_bitrate: Some(160),
                output: None,
            }),
        },
    ]
}
