use obs_video::{ColorRange, ColorSpace};

#[derive(Debug, Clone)]
pub struct VideoEncodeParams {
    pub encoder_id: String,
    pub bitrate_kbps: u32,
    pub rate_control: String, // "CBR" | "VBR" | "CQP" | "CRF"
    pub crf: u8,
    pub preset: String,
    pub keyint_sec: u32,
    pub fps: u32,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub streaming_low_latency: bool,
}

pub fn build_video_encoder_args(p: &VideoEncodeParams) -> Vec<String> {
    let mut args = vec!["-c:v".to_string(), p.encoder_id.clone()];
    let gop = (p.fps.max(1) * p.keyint_sec.max(1)).to_string();
    let br = format!("{}k", p.bitrate_kbps.max(300));
    let maxrate = format!("{}k", (p.bitrate_kbps.max(300) as f32 * 1.15) as u32);
    let bufsize = format!("{}k", p.bitrate_kbps.max(300) * 2);

    let rc_upper = p.rate_control.to_ascii_uppercase();

    match p.encoder_id.as_str() {
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
            let preset = match p.preset.as_str() {
                "speed" | "veryfast" => "p2",
                "quality" | "slow" => "p6",
                _ => "p4",
            };
            args.extend(["-preset".into(), preset.into()]);
            if p.streaming_low_latency {
                args.extend([
                    "-tune".into(),
                    "ll".into(),
                    "-zerolatency".into(),
                    "1".into(),
                ]);
            }
            if rc_upper == "CQP" || rc_upper == "CRF" {
                args.extend([
                    "-rc".into(),
                    "constqp".into(),
                    "-qp".into(),
                    p.crf.to_string(),
                ]);
            } else if rc_upper == "VBR" {
                args.extend([
                    "-rc".into(),
                    "vbr".into(),
                    "-b:v".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            } else {
                args.extend([
                    "-rc".into(),
                    "cbr".into(),
                    "-b:v".into(),
                    br.clone(),
                    "-minrate".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            }
        }
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            let quality = match p.preset.as_str() {
                "speed" | "veryfast" => "speed",
                "quality" | "slow" => "quality",
                _ => "balanced",
            };
            args.extend(["-quality".into(), quality.into()]);
            if p.streaming_low_latency {
                args.extend(["-usage".into(), "ultralowlatency".into()]);
            } else {
                args.extend(["-usage".into(), "transcoding".into()]);
            }
            if rc_upper == "CQP" || rc_upper == "CRF" {
                args.extend([
                    "-rc".into(),
                    "cqp".into(),
                    "-qp_i".into(),
                    p.crf.to_string(),
                    "-qp_p".into(),
                    p.crf.to_string(),
                ]);
            } else if rc_upper == "VBR" {
                args.extend([
                    "-rc".into(),
                    "vbr_peak".into(),
                    "-b:v".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            } else {
                args.extend([
                    "-rc".into(),
                    "cbr".into(),
                    "-b:v".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            }
        }
        "h264_qsv" | "hevc_qsv" | "av1_qsv" => {
            let preset = match p.preset.as_str() {
                "speed" | "veryfast" => "veryfast",
                "quality" | "slow" => "veryslow",
                _ => "medium",
            };
            args.extend(["-preset".into(), preset.into()]);
            if rc_upper == "CQP" || rc_upper == "CRF" {
                args.extend(["-global_quality".into(), p.crf.to_string()]);
            } else {
                args.extend([
                    "-b:v".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            }
        }
        "h264_mf" | "hevc_mf" => {
            args.extend([
                "-b:v".into(),
                br,
                "-rate_control".into(),
                if rc_upper == "CBR" { "cbr" } else { "quality" }.into(),
            ]);
        }
        "libx264" | "libx265" => {
            let preset = match p.preset.as_str() {
                "speed" => "ultrafast",
                "quality" => "medium",
                "balanced" => "veryfast",
                other => other,
            };
            args.extend(["-preset".into(), preset.into()]);
            if p.streaming_low_latency {
                args.extend(["-tune".into(), "zerolatency".into()]);
            }
            if rc_upper == "CQP" || rc_upper == "CRF" {
                args.extend(["-crf".into(), p.crf.to_string()]);
            } else if rc_upper == "VBR" {
                args.extend([
                    "-b:v".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
            } else {
                args.extend([
                    "-b:v".into(),
                    br.clone(),
                    "-minrate".into(),
                    br,
                    "-maxrate".into(),
                    maxrate,
                    "-bufsize".into(),
                    bufsize,
                ]);
                if p.encoder_id == "libx264" {
                    args.extend(["-x264-params".into(), "nal-hrd=cbr:scenecut=0".into()]);
                }
            }
        }
        "libsvtav1" => {
            args.extend(["-preset".into(), "8".into()]);
            if rc_upper == "CQP" || rc_upper == "CRF" {
                args.extend(["-crf".into(), p.crf.clamp(12, 50).to_string()]);
            } else {
                args.extend(["-b:v".into(), br]);
            }
        }
        _ => {
            args.extend(["-b:v".into(), br]);
        }
    }

    args.extend([
        "-g".into(),
        gop,
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-colorspace".into(),
        p.color_space.ffmpeg_colorspace().into(),
        "-color_trc".into(),
        p.color_space.ffmpeg_color_trc().into(),
        "-color_range".into(),
        p.color_range.ffmpeg_color_range().into(),
    ]);

    args
}
