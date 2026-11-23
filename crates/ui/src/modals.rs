use crate::backend::StudioBackend;
use egui::{Color32, RichText, Ui};
use obs_core::{SourceId, TransitionId};
use obs_filters::{
    AudioFilterInstance, AudioFilterKind, ChromaKeyPreset, ExpanderPreset, MaskBlendType,
    NoiseSuppressionMethod, VideoFilterInstance, VideoFilterKind,
};
use obs_scenes::{AudioMonitoringType, SceneCollection, SourceKind, SourceProperties};
use obs_settings::{
    AppSettings, OutputMode, RecordingFormat, RecordingQualityPreset, StreamProtocol, ThemeMode,
};
use obs_transitions::{TransitionDirection, TransitionKind};
use obs_video::{ColorRange, ColorSpace, ScaleFilter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Stream,
    Output,
    Audio,
    Video,
    Hotkeys,
    Accessibility,
    Advanced,
    Plugins,
}

pub struct SettingsModalState {
    pub open: bool,
    pub tab: SettingsTab,
    pub draft: AppSettings,
    pub recording_hotkey_idx: Option<usize>,
    pub new_extra_name: String,
    pub new_extra_url: String,
    pub new_extra_key: String,
}

impl Default for SettingsModalState {
    fn default() -> Self {
        Self {
            open: false,
            tab: SettingsTab::General,
            draft: AppSettings::default(),
            recording_hotkey_idx: None,
            new_extra_name: String::new(),
            new_extra_url: "rtmp://".to_string(),
            new_extra_key: String::new(),
        }
    }
}

pub fn render_settings_modal(
    ctx: &egui::Context,
    st: &mut SettingsModalState,
    backend: &dyn StudioBackend,
    toast: &mut crate::ToastQueue,
) {
    if !st.open {
        return;
    }
    let mut open = st.open;
    egui::Window::new("Settings")
        .open(&mut open)
        .default_size([840.0, 580.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Left category sidebar
                ui.vertical(|ui| {
                    ui.set_min_width(145.0);
                    for (tab, label) in [
                        (SettingsTab::General, "General"),
                        (SettingsTab::Stream, "Stream"),
                        (SettingsTab::Output, "Output"),
                        (SettingsTab::Audio, "Audio"),
                        (SettingsTab::Video, "Video"),
                        (SettingsTab::Hotkeys, "Hotkeys"),
                        (SettingsTab::Accessibility, "Accessibility"),
                        (SettingsTab::Advanced, "Advanced"),
                        (SettingsTab::Plugins, "Plugins"),
                    ] {
                        if ui.selectable_label(st.tab == tab, label).clicked() {
                            st.tab = tab;
                            st.recording_hotkey_idx = None;
                        }
                    }
                });
                ui.separator();

                // Right settings body
                egui::ScrollArea::vertical()
                    .max_height(475.0)
                    .show(ui, |ui| {
                        ui.set_min_width(640.0);
                        match st.tab {
                            SettingsTab::General => render_tab_general(ui, &mut st.draft),
                            SettingsTab::Stream => render_tab_stream(ui, st, backend),
                            SettingsTab::Output => render_tab_output(ui, &mut st.draft, backend),
                            SettingsTab::Audio => render_tab_audio(ui, &mut st.draft, backend),
                            SettingsTab::Video => render_tab_video(ui, &mut st.draft, backend),
                            SettingsTab::Hotkeys => render_tab_hotkeys(ctx, ui, st),
                            SettingsTab::Accessibility => render_tab_a11y(ui, &mut st.draft),
                            SettingsTab::Advanced => render_tab_advanced(ui, &mut st.draft),
                            SettingsTab::Plugins => render_tab_plugins(ui, &mut st.draft, backend),
                        }
                    });
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    match backend.apply_settings(st.draft.clone()) {
                        Ok(()) => toast.push_info("Settings saved and applied."),
                        Err(e) => toast.push_error(format!("Settings error: {e}")),
                    }
                }
                if ui.button("OK").clicked() {
                    match backend.apply_settings(st.draft.clone()) {
                        Ok(()) => {
                            st.open = false;
                            toast.push_info("Settings saved.");
                        }
                        Err(e) => toast.push_error(format!("Settings error: {e}")),
                    }
                }
                if ui.button("Cancel").clicked() {
                    st.open = false;
                }
            });
        });
    if !open {
        st.open = false;
    }
}

fn render_tab_general(ui: &mut Ui, s: &mut AppSettings) {
    ui.heading("General");
    ui.add_space(6.0);
    egui::Grid::new("gen_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Theme:");
            egui::ComboBox::from_id_salt("theme_sel")
                .selected_text(match s.general.theme {
                    ThemeMode::Dark => "Yami Dark",
                    ThemeMode::Light => "Crisp Light",
                    ThemeMode::HighContrast => "High Contrast",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.general.theme, ThemeMode::Dark, "Yami Dark");
                    ui.selectable_value(&mut s.general.theme, ThemeMode::Light, "Crisp Light");
                    ui.selectable_value(
                        &mut s.general.theme,
                        ThemeMode::HighContrast,
                        "High Contrast",
                    );
                });
            ui.end_row();
        });

    ui.separator();
    ui.label(RichText::new("Output Confirmations & Behavior").strong());
    ui.checkbox(
        &mut s.general.confirm_on_stream_start,
        "Show confirmation dialog when starting streams",
    );
    ui.checkbox(
        &mut s.general.confirm_on_stream_stop,
        "Show confirmation dialog when stopping streams",
    );
    ui.checkbox(
        &mut s.general.confirm_on_record_stop,
        "Show confirmation dialog when stopping recording",
    );
    ui.checkbox(
        &mut s.general.auto_record_when_streaming,
        "Automatically record when streaming",
    );
    ui.checkbox(
        &mut s.general.keep_recording_when_stream_stops,
        "Keep recording when stream stops",
    );
    ui.checkbox(
        &mut s.general.open_stats_on_startup,
        "Open Stats panel on startup",
    );

    ui.separator();
    ui.label(RichText::new("Source Alignment Snapping").strong());
    ui.checkbox(
        &mut s.general.snap_enabled,
        "Enable source alignment snapping",
    );
    ui.horizontal(|ui| {
        ui.label("Snap Sensitivity (px):");
        ui.add(egui::Slider::new(
            &mut s.general.snap_distance_px,
            2.0..=30.0,
        ));
    });
    ui.checkbox(
        &mut s.general.snap_to_screen_edges,
        "Snap Sources to screen edges",
    );
    ui.checkbox(
        &mut s.general.snap_to_sources,
        "Snap Sources to other sources",
    );
}

fn render_tab_stream(ui: &mut Ui, st: &mut SettingsModalState, backend: &dyn StudioBackend) {
    ui.heading("Stream");
    let services = backend.services();
    let s = &mut st.draft.stream;

    ui.horizontal(|ui| {
        ui.label("Service:");
        egui::ComboBox::from_id_salt("svc_sel")
            .selected_text(&s.service_name)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(s.service_name == "Custom", "Custom...")
                    .clicked()
                {
                    s.service_name = "Custom".to_string();
                }
                for entry in services.iter().take(40) {
                    if ui
                        .selectable_label(s.service_name == entry.name, &entry.name)
                        .clicked()
                    {
                        s.service_name = entry.name.clone();
                        if let Some(first) = entry.servers.first() {
                            s.server_name = first.name.clone();
                            s.server_url = first.url.clone();
                            s.protocol = if first.url.starts_with("rtmps://") {
                                StreamProtocol::Rtmps
                            } else if first.url.starts_with("srt://") {
                                StreamProtocol::Srt
                            } else {
                                StreamProtocol::Rtmp
                            };
                        }
                    }
                }
            });
    });

    if let Some(svc) = services.iter().find(|e| e.name == s.service_name) {
        ui.horizontal(|ui| {
            ui.label("Server:");
            egui::ComboBox::from_id_salt("srv_sel")
                .selected_text(&s.server_name)
                .show_ui(ui, |ui| {
                    for srv in &svc.servers {
                        if ui
                            .selectable_label(s.server_url == srv.url, &srv.name)
                            .clicked()
                        {
                            s.server_name = srv.name.clone();
                            s.server_url = srv.url.clone();
                            s.protocol = if srv.url.starts_with("rtmps://") {
                                StreamProtocol::Rtmps
                            } else {
                                StreamProtocol::Rtmp
                            };
                        }
                    }
                });
        });
    }

    egui::Grid::new("stream_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Protocol:");
            egui::ComboBox::from_id_salt("proto_sel")
                .selected_text(s.protocol.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.protocol, StreamProtocol::Rtmp, "RTMP");
                    ui.selectable_value(&mut s.protocol, StreamProtocol::Rtmps, "RTMPS (TLS)");
                    ui.selectable_value(&mut s.protocol, StreamProtocol::Srt, "SRT (Caller)");
                });
            ui.end_row();

            ui.label("Server URL:");
            ui.text_edit_singleline(&mut s.server_url);
            ui.end_row();

            ui.label("Stream Key:");
            ui.add(egui::TextEdit::singleline(&mut s.stream_key).password(true));
            ui.end_row();

            if s.protocol == StreamProtocol::Srt {
                ui.label("SRT Latency (ms):");
                ui.add(egui::DragValue::new(&mut s.srt_latency_ms).range(20..=5000));
                ui.end_row();
                ui.label("SRT Passphrase:");
                ui.add(egui::TextEdit::singleline(&mut s.srt_passphrase).password(true));
                ui.end_row();
            }
        });

    ui.checkbox(&mut s.use_auth, "Use Authentication");
    if s.use_auth {
        ui.horizontal(|ui| {
            ui.label("Username:");
            ui.text_edit_singleline(&mut s.username);
            ui.label("Password:");
            ui.add(egui::TextEdit::singleline(&mut s.password).password(true));
        });
    }

    ui.separator();
    ui.label(RichText::new("Multi-Output Simultaneous Streaming Targets").strong());
    let mut remove_idx = None;
    for (idx, ex) in s.extra_outputs.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut ex.enabled, "");
            ui.label(format!("{} ({})", ex.name, ex.url));
            if ui.small_button("Remove").clicked() {
                remove_idx = Some(idx);
            }
        });
    }
    if let Some(i) = remove_idx {
        s.extra_outputs.remove(i);
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.new_extra_name)
                .hint_text("Name (e.g. YouTube)")
                .desired_width(120.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut st.new_extra_url)
                .hint_text("rtmp(s)://...")
                .desired_width(220.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut st.new_extra_key)
                .hint_text("Stream key")
                .password(true)
                .desired_width(140.0),
        );
        if ui.button("Add Target").clicked() && !st.new_extra_url.trim().is_empty() {
            s.extra_outputs.push(obs_settings::ExtraOutputConfig {
                id: format!("extra-{}", s.extra_outputs.len() + 1),
                name: if st.new_extra_name.trim().is_empty() {
                    "Extra Stream".to_string()
                } else {
                    st.new_extra_name.trim().to_string()
                },
                enabled: true,
                url: st.new_extra_url.trim().to_string(),
                stream_key: st.new_extra_key.trim().to_string(),
                protocol: if st.new_extra_url.starts_with("rtmps://") {
                    StreamProtocol::Rtmps
                } else {
                    StreamProtocol::Rtmp
                },
                bitrate_kbps: 6000,
            });
            st.new_extra_name.clear();
            st.new_extra_key.clear();
        }
    });
}

fn render_tab_output(ui: &mut Ui, s: &mut AppSettings, backend: &dyn StudioBackend) {
    ui.heading("Output");
    let report = backend.encoder_report();
    let available_encoders: Vec<(String, String)> =
        std::iter::once(("auto".to_string(), "Auto (Best Verified HW/SW)".to_string()))
            .chain(
                report
                    .video_encoders
                    .iter()
                    .filter(|e| e.available)
                    .map(|e| (e.id.clone(), e.display_name.clone())),
            )
            .collect();

    ui.horizontal(|ui| {
        ui.label("Output Mode:");
        egui::ComboBox::from_id_salt("out_mode")
            .selected_text(match s.output.mode {
                OutputMode::Simple => "Simple",
                OutputMode::Advanced => "Advanced",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut s.output.mode, OutputMode::Simple, "Simple");
                ui.selectable_value(&mut s.output.mode, OutputMode::Advanced, "Advanced");
            });
        if ui.button("Re-Probe Hardware Encoders").clicked() {
            let _ = backend.reprobe_encoders();
        }
    });

    ui.separator();
    ui.label(RichText::new("Streaming Encoder").strong());
    egui::Grid::new("out_stream_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Video Encoder:");
            let sel_label = available_encoders
                .iter()
                .find(|(id, _)| id == &s.output.stream_video_encoder)
                .map(|(_, d)| d.clone())
                .unwrap_or_else(|| s.output.stream_video_encoder.clone());
            egui::ComboBox::from_id_salt("str_enc")
                .selected_text(sel_label)
                .show_ui(ui, |ui| {
                    for (id, disp) in &available_encoders {
                        ui.selectable_value(&mut s.output.stream_video_encoder, id.clone(), disp);
                    }
                });
            ui.end_row();

            ui.label("Video Bitrate (Kbps):");
            ui.add(
                egui::DragValue::new(&mut s.output.stream_bitrate_kbps)
                    .range(500..=51000)
                    .speed(100),
            );
            ui.end_row();

            ui.label("Audio Bitrate (Kbps):");
            ui.add(
                egui::DragValue::new(&mut s.output.stream_audio_bitrate_kbps)
                    .range(64..=320)
                    .speed(16),
            );
            ui.end_row();

            ui.label("Keyframe Interval (s):");
            ui.add(egui::DragValue::new(&mut s.output.stream_keyint_sec).range(1..=10));
            ui.end_row();

            ui.label("Audio Track (1-6):");
            if s.output.mode == OutputMode::Simple {
                ui.label(format!("Track {}", s.output.stream_audio_track));
            } else {
                ui.add(egui::DragValue::new(&mut s.output.stream_audio_track).range(1..=6));
            }
            ui.end_row();

            if s.output.mode == OutputMode::Advanced {
                ui.label("Rate Control:");
                egui::ComboBox::from_id_salt("str_rc")
                    .selected_text(&s.output.stream_rate_control)
                    .show_ui(ui, |ui| {
                        for rc in ["CBR", "VBR"] {
                            ui.selectable_value(
                                &mut s.output.stream_rate_control,
                                rc.to_string(),
                                rc,
                            );
                        }
                    });
                ui.end_row();
            }
        });

    ui.separator();
    ui.label(RichText::new("Recording").strong());
    egui::Grid::new("out_rec_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Recording Path:");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut s.output.rec_path);
                if ui.button("Browse").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        s.output.rec_path = dir.to_string_lossy().to_string();
                    }
                }
            });
            ui.end_row();

            ui.label("Filename Formatting:");
            ui.text_edit_singleline(&mut s.output.rec_filename_format);
            ui.end_row();

            ui.label("Recording Format:");
            egui::ComboBox::from_id_salt("rec_fmt")
                .selected_text(s.output.rec_format.display_name())
                .show_ui(ui, |ui| {
                    for f in [
                        RecordingFormat::Mkv,
                        RecordingFormat::Mp4,
                        RecordingFormat::FragmentedMp4,
                        RecordingFormat::Mov,
                        RecordingFormat::Flv,
                    ] {
                        ui.selectable_value(&mut s.output.rec_format, f, f.display_name());
                    }
                });
            ui.end_row();

            ui.label("Recording Encoder:");
            let sel_rec = available_encoders
                .iter()
                .find(|(id, _)| id == &s.output.rec_video_encoder)
                .map(|(_, d)| d.clone())
                .unwrap_or_else(|| s.output.rec_video_encoder.clone());
            egui::ComboBox::from_id_salt("rec_enc")
                .selected_text(sel_rec)
                .show_ui(ui, |ui| {
                    for (id, disp) in &available_encoders {
                        ui.selectable_value(&mut s.output.rec_video_encoder, id.clone(), disp);
                    }
                });
            ui.end_row();

            if s.output.mode == OutputMode::Simple {
                ui.label("Quality Preset:");
                egui::ComboBox::from_id_salt("rec_qual")
                    .selected_text(s.output.rec_quality_preset.display_name())
                    .show_ui(ui, |ui| {
                        for q in [
                            RecordingQualityPreset::SameAsStream,
                            RecordingQualityPreset::HighQuality,
                            RecordingQualityPreset::Indistinguishable,
                            RecordingQualityPreset::Lossless,
                        ] {
                            ui.selectable_value(
                                &mut s.output.rec_quality_preset,
                                q,
                                q.display_name(),
                            );
                        }
                    });
                ui.end_row();
            } else {
                ui.label("Audio Encoder:");
                egui::ComboBox::from_id_salt("rec_aenc")
                    .selected_text(if s.output.rec_audio_encoder == "libopus" {
                        "Opus (libopus)"
                    } else {
                        "AAC (aac)"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut s.output.rec_audio_encoder,
                            "aac".to_string(),
                            "AAC (aac)",
                        );
                        ui.selectable_value(
                            &mut s.output.rec_audio_encoder,
                            "libopus".to_string(),
                            "Opus (libopus)",
                        );
                    });
                ui.end_row();

                ui.label("Rate Control:");
                egui::ComboBox::from_id_salt("rec_rc")
                    .selected_text(&s.output.rec_rate_control)
                    .show_ui(ui, |ui| {
                        for rc in ["CQP", "CBR", "VBR"] {
                            ui.selectable_value(&mut s.output.rec_rate_control, rc.to_string(), rc);
                        }
                    });
                ui.end_row();

                if s.output.rec_rate_control == "CQP" {
                    ui.label("CRF / CQ Level:");
                    ui.add(egui::Slider::new(&mut s.output.rec_crf, 0..=40));
                    ui.end_row();
                } else {
                    ui.label("Video Bitrate (Kbps):");
                    ui.add(
                        egui::DragValue::new(&mut s.output.rec_bitrate_kbps)
                            .range(1000..=100000)
                            .speed(250),
                    );
                    ui.end_row();
                }

                ui.label("Encoder Preset:");
                egui::ComboBox::from_id_salt("rec_preset")
                    .selected_text(&s.output.rec_preset)
                    .show_ui(ui, |ui| {
                        for p in ["speed", "balanced", "quality"] {
                            ui.selectable_value(&mut s.output.rec_preset, p.to_string(), p);
                        }
                    });
                ui.end_row();

                ui.label("Keyframe Interval (s):");
                ui.add(egui::DragValue::new(&mut s.output.rec_keyint_sec).range(1..=10));
                ui.end_row();

                ui.label("Audio Bitrate (Kbps):");
                ui.add(
                    egui::DragValue::new(&mut s.output.rec_audio_bitrate_kbps)
                        .range(64..=320)
                        .speed(16),
                );
                ui.end_row();
            }

            ui.label("Audio Tracks:");
            ui.horizontal(|ui| {
                for t in 0..6u8 {
                    let mut checked = (s.output.rec_audio_tracks & (1 << t)) != 0;
                    if ui.checkbox(&mut checked, format!("{}", t + 1)).changed() {
                        if checked {
                            s.output.rec_audio_tracks |= 1 << t;
                        } else {
                            s.output.rec_audio_tracks &= !(1 << t);
                        }
                    }
                }
            });
            ui.end_row();
        });

    ui.checkbox(
        &mut s.output.auto_remux_to_mp4,
        "Automatically remux MKV recordings to MP4",
    );

    ui.separator();
    ui.label(RichText::new("Replay Buffer").strong());
    ui.checkbox(&mut s.output.replay_enabled, "Enable Replay Buffer");
    ui.horizontal(|ui| {
        ui.label("Max Replay Time (sec):");
        ui.add(egui::DragValue::new(&mut s.output.replay_duration_sec).range(5..=600));
        ui.label("Max Memory (MB):");
        ui.add(egui::DragValue::new(&mut s.output.replay_max_memory_mb).range(64..=8192));
    });
}

fn render_tab_audio(ui: &mut Ui, s: &mut AppSettings, backend: &dyn StudioBackend) {
    ui.heading("Audio");
    let inputs = backend.enumerate_audio_inputs();
    let outputs = backend.enumerate_audio_outputs();
    let output_active = backend.has_active_output();

    if output_active {
        ui.colored_label(
            Color32::from_rgb(245, 190, 65),
            "Sample rate & channel layout are locked while recording, streaming, or replay is active.",
        );
    }

    egui::Grid::new("aud_grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Sample Rate:");
            ui.add_enabled_ui(!output_active, |ui| {
                egui::ComboBox::from_id_salt("aud_sr")
                    .selected_text(format!("{} Hz", s.audio.sample_rate))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio.sample_rate, 48000, "48 kHz");
                        ui.selectable_value(&mut s.audio.sample_rate, 44100, "44.1 kHz");
                    });
            });
            ui.end_row();

            ui.label("Channels:");
            ui.add_enabled_ui(!output_active, |ui| {
                egui::ComboBox::from_id_salt("aud_ch")
                    .selected_text(&s.audio.channels)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio.channels, "Stereo".to_string(), "Stereo");
                        ui.selectable_value(&mut s.audio.channels, "Mono".to_string(), "Mono");
                    });
            });
            ui.end_row();

            ui.label("Desktop Audio:");
            device_combo(ui, "desk1", &mut s.audio.desktop_device_1, &outputs);
            ui.end_row();

            ui.label("Desktop Audio 2:");
            device_combo(ui, "desk2", &mut s.audio.desktop_device_2, &outputs);
            ui.end_row();

            ui.label("Mic/Auxiliary Audio:");
            device_combo(ui, "mic1", &mut s.audio.mic_device_1, &inputs);
            ui.end_row();

            ui.label("Mic/Auxiliary Audio 2:");
            device_combo(ui, "mic2", &mut s.audio.mic_device_2, &inputs);
            ui.end_row();

            ui.label("Audio Monitoring Device:");
            device_combo(ui, "mon1", &mut s.audio.monitoring_device, &outputs);
            ui.end_row();
        });
}

fn device_combo(
    ui: &mut Ui,
    id: &str,
    selected: &mut String,
    devices: &[obs_platform_windows::AudioDeviceInfo],
) {
    let label = if selected == "Default" || selected == "Disabled" {
        selected.clone()
    } else {
        devices
            .iter()
            .find(|d| d.id == *selected)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| selected.clone())
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, "Default".to_string(), "Default");
            ui.selectable_value(selected, "Disabled".to_string(), "Disabled");
            for d in devices {
                ui.selectable_value(selected, d.id.clone(), &d.name);
            }
        });
}

fn render_tab_video(ui: &mut Ui, s: &mut AppSettings, backend: &dyn StudioBackend) {
    ui.heading("Video");
    let output_active = backend.has_active_output();
    if output_active {
        ui.colored_label(
            Color32::from_rgb(245, 190, 65),
            "Video resolution and FPS are locked while recording, streaming, or replay is active.",
        );
    }
    let vc = &mut s.video.config;
    ui.add_enabled_ui(!output_active, |ui| {
        egui::Grid::new("vid_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Base (Canvas) Resolution:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut vc.canvas_width)
                            .range(320..=3840)
                            .speed(2),
                    );
                    ui.label("x");
                    ui.add(
                        egui::DragValue::new(&mut vc.canvas_height)
                            .range(240..=2160)
                            .speed(2),
                    );
                    for (lw, lh) in [(1920, 1080), (1920, 1200), (2560, 1440), (1280, 720)] {
                        if ui.small_button(format!("{lw}x{lh}")).clicked() {
                            vc.canvas_width = lw;
                            vc.canvas_height = lh;
                        }
                    }
                });
                ui.end_row();

                ui.label("Output (Scaled) Resolution:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut vc.output_width)
                            .range(320..=3840)
                            .speed(2),
                    );
                    ui.label("x");
                    ui.add(
                        egui::DragValue::new(&mut vc.output_height)
                            .range(240..=2160)
                            .speed(2),
                    );
                    for (lw, lh) in [(1920, 1080), (1280, 720), (852, 480)] {
                        if ui.small_button(format!("{lw}x{lh}")).clicked() {
                            vc.output_width = lw;
                            vc.output_height = lh;
                        }
                    }
                });
                ui.end_row();

                ui.label("Downscale Filter:");
                egui::ComboBox::from_id_salt("scale_flt")
                    .selected_text(vc.scale_filter.display_name())
                    .show_ui(ui, |ui| {
                        for f in [
                            ScaleFilter::Point,
                            ScaleFilter::Bilinear,
                            ScaleFilter::Bicubic,
                            ScaleFilter::Lanczos,
                            ScaleFilter::Area,
                        ] {
                            ui.selectable_value(&mut vc.scale_filter, f, f.display_name());
                        }
                    });
                ui.end_row();

                ui.label("Common FPS Values:");
                egui::ComboBox::from_id_salt("fps_sel")
                    .selected_text(format!("{:.2} FPS", vc.fps_f64()))
                    .show_ui(ui, |ui| {
                        for (num, den, lbl) in [
                            (60, 1, "60 FPS"),
                            (5994, 100, "59.94 FPS"),
                            (48, 1, "48 FPS"),
                            (30, 1, "30 FPS"),
                            (2997, 100, "29.97 FPS"),
                            (24, 1, "24 FPS"),
                        ] {
                            if ui
                                .selectable_label(vc.fps_num == num && vc.fps_den == den, lbl)
                                .clicked()
                            {
                                vc.fps_num = num;
                                vc.fps_den = den;
                            }
                        }
                    });
                ui.end_row();

                ui.label("Color Format:");
                ui.label("NV12 (8-bit, 4:2:0) [GPU Compute]");
                ui.end_row();

                ui.label("Color Space:");
                egui::ComboBox::from_id_salt("vid_cs_sel")
                    .selected_text(vc.color_space.display_name())
                    .show_ui(ui, |ui| {
                        for cs in [
                            ColorSpace::Rec709,
                            ColorSpace::Rec601,
                            ColorSpace::Rec2100Pq,
                            ColorSpace::Rec2100Hlg,
                        ] {
                            ui.selectable_value(&mut vc.color_space, cs, cs.display_name());
                        }
                    });
                ui.end_row();

                ui.label("Color Range:");
                egui::ComboBox::from_id_salt("vid_cr_sel")
                    .selected_text(vc.color_range.display_name())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut vc.color_range,
                            ColorRange::Partial,
                            "Limited (16-235)",
                        );
                        ui.selectable_value(&mut vc.color_range, ColorRange::Full, "Full (0-255)");
                    });
                ui.end_row();
            });
    });
}

fn render_tab_hotkeys(ctx: &egui::Context, ui: &mut Ui, st: &mut SettingsModalState) {
    ui.heading("Hotkeys");
    ui.checkbox(
        &mut st.draft.hotkeys.enabled,
        "Enable Global Windows Hotkeys",
    );
    ui.separator();

    if let Some(idx) = st.recording_hotkey_idx {
        ui.colored_label(
            Color32::YELLOW,
            "Press any key combination (with Ctrl/Alt/Shift) or Escape to cancel...",
        );
        ctx.input(|inp| {
            for ev in &inp.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = ev
                {
                    if *key == egui::Key::Escape {
                        st.recording_hotkey_idx = None;
                    } else if let Some(b) = st.draft.hotkeys.bindings.get_mut(idx) {
                        b.key = format!("{key:?}");
                        b.ctrl = modifiers.ctrl;
                        b.alt = modifiers.alt;
                        b.shift = modifiers.shift;
                        st.recording_hotkey_idx = None;
                    }
                }
            }
        });
    }

    egui::Grid::new("hk_grid")
        .num_columns(3)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for (idx, b) in st.draft.hotkeys.bindings.iter_mut().enumerate() {
                ui.label(&b.action);
                let btn_lbl = if st.recording_hotkey_idx == Some(idx) {
                    "Listening...".to_string()
                } else {
                    b.display_combo()
                };
                if ui.button(btn_lbl).clicked() {
                    st.recording_hotkey_idx = Some(idx);
                }
                if ui.small_button("Clear").clicked() {
                    b.key.clear();
                    b.ctrl = false;
                    b.alt = false;
                    b.shift = false;
                }
                ui.end_row();
            }
        });
}

fn render_tab_a11y(ui: &mut Ui, s: &mut AppSettings) {
    ui.heading("Accessibility");
    ui.checkbox(&mut s.accessibility.high_contrast, "Force High Contrast UI");
    ui.horizontal(|ui| {
        ui.label("UI Scale:");
        ui.add(egui::Slider::new(&mut s.accessibility.ui_scale, 0.8..=1.6));
    });
    ui.horizontal(|ui| {
        ui.label("Selection Color:");
        ui.color_edit_button_srgba_unmultiplied(&mut s.accessibility.selection_color_rgba);
        ui.label("Crop Color:");
        ui.color_edit_button_srgba_unmultiplied(&mut s.accessibility.crop_color_rgba);
    });
}

fn render_tab_advanced(ui: &mut Ui, s: &mut AppSettings) {
    ui.heading("Advanced");
    ui.label(RichText::new("Video Color & HDR").strong());
    egui::Grid::new("adv_col")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Color Space:");
            egui::ComboBox::from_id_salt("cs_sel")
                .selected_text(s.video.config.color_space.display_name())
                .show_ui(ui, |ui| {
                    for cs in [
                        ColorSpace::Rec709,
                        ColorSpace::Rec601,
                        ColorSpace::Rec2100Pq,
                        ColorSpace::Rec2100Hlg,
                    ] {
                        ui.selectable_value(&mut s.video.config.color_space, cs, cs.display_name());
                    }
                });
            ui.end_row();

            ui.label("Color Range:");
            egui::ComboBox::from_id_salt("cr_sel")
                .selected_text(s.video.config.color_range.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut s.video.config.color_range,
                        ColorRange::Partial,
                        "Limited (16-235)",
                    );
                    ui.selectable_value(
                        &mut s.video.config.color_range,
                        ColorRange::Full,
                        "Full (0-255)",
                    );
                });
            ui.end_row();
        });

    ui.separator();
    ui.label(RichText::new("Network & Stream Reconnection").strong());
    ui.checkbox(&mut s.advanced.auto_reconnect, "Enable Automatic Reconnect");
    ui.horizontal(|ui| {
        ui.label("Retry Delay (s):");
        ui.add(egui::DragValue::new(&mut s.advanced.reconnect_delay_sec).range(1..=60));
        ui.label("Max Retries:");
        ui.add(egui::DragValue::new(&mut s.advanced.max_retries).range(0..=100));
    });
    ui.checkbox(
        &mut s.advanced.dynamic_bitrate,
        "Dynamically change bitrate to manage congestion",
    );
    ui.small("Applies AIMD congestion bitrate adaptation on RTMP/RTMPS streams (not direct SRT).");

    ui.separator();
    ui.label(RichText::new("OBS WebSocket 5.x Server").strong());
    ui.checkbox(&mut s.advanced.websocket_enabled, "Enable WebSocket Server");
    ui.horizontal(|ui| {
        ui.label("Host:");
        ui.text_edit_singleline(&mut s.advanced.websocket_host);
        ui.label("Port:");
        ui.add(egui::DragValue::new(&mut s.advanced.websocket_port).range(1024..=65535));
    });
    ui.checkbox(
        &mut s.advanced.websocket_auth_enabled,
        "Enable Authentication",
    );
    if s.advanced.websocket_auth_enabled {
        ui.horizontal(|ui| {
            ui.label("Server Password:");
            ui.add(egui::TextEdit::singleline(&mut s.advanced.websocket_password).password(true));
        });
    }
}

fn render_tab_plugins(ui: &mut Ui, s: &mut AppSettings, backend: &dyn StudioBackend) {
    ui.heading("Plugins (OxideStudio C-ABI v1)");
    if ui.button("Reload Plugins").clicked() {
        backend.reload_plugins();
    }
    ui.separator();
    let plugins = backend.plugins();
    if plugins.is_empty() {
        ui.label("No dynamic plugins found in ./plugins directory.");
    } else {
        for p in plugins {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let mut enabled = !s.plugins.disabled_plugins.contains(&p.id);
                    if ui
                        .checkbox(&mut enabled, RichText::new(&p.name).strong())
                        .changed()
                    {
                        if enabled {
                            s.plugins.disabled_plugins.retain(|x| x != &p.id);
                        } else if !s.plugins.disabled_plugins.contains(&p.id) {
                            s.plugins.disabled_plugins.push(p.id.clone());
                        }
                    }
                    ui.label(format!("v{} by {}", p.version, p.author));
                });
                ui.small(&p.description);
                ui.small(format!("Path: {}", p.path));
                if let Some(err) = p.error {
                    ui.colored_label(Color32::LIGHT_RED, err);
                }
            });
        }
    }
    ui.add_space(6.0);
    ui.small("Note: Upstream C/C++ OBS Studio binary plugins (.dll) are not directly ABI-compatible and are safely skipped if placed in plugins/.");
}

#[derive(Default)]
pub struct AddSourceModalState {
    pub open: bool,
    pub kind: Option<SourceKind>,
    pub create_new: bool,
    pub new_name: String,
    pub selected_existing: Option<SourceId>,
}

pub fn render_add_source_modal(
    ctx: &egui::Context,
    st: &mut AddSourceModalState,
    backend: &dyn StudioBackend,
    toast: &mut crate::ToastQueue,
    props_modal: &mut Option<SourceId>,
) {
    if !st.open {
        return;
    }
    let Some(kind) = st.kind else {
        st.open = false;
        return;
    };
    let mut open = st.open;
    egui::Window::new(format!("Create/Select Source — {}", kind.display_name()))
        .open(&mut open)
        .default_width(400.0)
        .show(ctx, |ui| {
            ui.radio_value(&mut st.create_new, true, "Create new");
            ui.add_enabled_ui(st.create_new, |ui| {
                ui.text_edit_singleline(&mut st.new_name);
            });

            let col = backend.collection();
            let matching_existing: Vec<(SourceId, String)> = col
                .sources
                .values()
                .filter(|s| s.kind == kind)
                .map(|s| (s.id, s.name.clone()))
                .collect();

            if !matching_existing.is_empty() {
                ui.radio_value(&mut st.create_new, false, "Add Existing (Reference Clone)");
                ui.add_enabled_ui(!st.create_new, |ui| {
                    for (id, name) in &matching_existing {
                        ui.selectable_value(&mut st.selected_existing, Some(*id), name);
                    }
                });
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    let active_scene = col.active_scene_id;
                    if st.create_new {
                        let mut props = SourceProperties::default_for(kind);
                        if let SourceProperties::SceneSource { scene_id } = &mut props {
                            if let Some(other) = col.scenes.iter().find(|s| s.id != active_scene) {
                                *scene_id = other.id;
                            }
                        }
                        match backend.add_new_source(active_scene, &st.new_name, kind, props) {
                            Ok((sid, _)) => {
                                st.open = false;
                                *props_modal = Some(sid);
                            }
                            Err(e) => toast.push_error(e),
                        }
                    } else if let Some(existing_id) = st.selected_existing {
                        match backend.add_existing_source(active_scene, existing_id) {
                            Ok(_) => st.open = false,
                            Err(e) => toast.push_error(e),
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    st.open = false;
                }
            });
        });
    if !open {
        st.open = false;
    }
}

pub fn render_source_properties_modal(
    ctx: &egui::Context,
    source_id_opt: &mut Option<SourceId>,
    backend: &dyn StudioBackend,
) {
    let Some(source_id) = *source_id_opt else {
        return;
    };
    let col = backend.collection();
    let Some(src_def) = col.sources.get(&source_id).cloned() else {
        *source_id_opt = None;
        return;
    };

    let mut open = true;
    let mut name = src_def.name.clone();
    let mut props = src_def.properties.clone();

    egui::Window::new(format!("Properties for '{}'", src_def.name))
        .open(&mut open)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Source Name:");
                ui.text_edit_singleline(&mut name);
            });
            ui.separator();

            match &mut props {
                SourceProperties::DisplayCapture {
                    monitor_index,
                    capture_cursor,
                    force_gdi,
                    capture_method,
                    force_sdr,
                } => {
                    if *force_gdi && capture_method == "Auto" {
                        *capture_method = "GDI".to_string();
                    }
                    let mons = backend.enumerate_monitors();
                    let cur_lbl = mons
                        .get(*monitor_index)
                        .map(|m| m.display_label())
                        .unwrap_or_else(|| format!("Display {}", *monitor_index + 1));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("Display")
                            .selected_text(cur_lbl)
                            .show_ui(ui, |ui| {
                                for m in &mons {
                                    ui.selectable_value(monitor_index, m.index, m.display_label());
                                }
                            });
                        if ui.button("Refresh").clicked() {
                            ctx.request_repaint();
                        }
                    });
                    egui::ComboBox::from_label("Capture Method")
                        .selected_text(match capture_method.as_str() {
                            "DXGI" => "DXGI Desktop Duplication",
                            "GDI" => "GDI BitBlt (Compatibility)",
                            _ => "Automatic (DXGI + GDI Fallback)",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                capture_method,
                                "Auto".to_string(),
                                "Automatic (DXGI + GDI Fallback)",
                            );
                            ui.selectable_value(
                                capture_method,
                                "DXGI".to_string(),
                                "DXGI Desktop Duplication",
                            );
                            ui.selectable_value(
                                capture_method,
                                "GDI".to_string(),
                                "GDI BitBlt (Compatibility)",
                            );
                        });
                    *force_gdi = capture_method == "GDI";
                    ui.label("Color Format: RGBA8 (sRGB) -> GPU NV12");
                    ui.checkbox(capture_cursor, "Capture Cursor");
                    ui.checkbox(
                        force_sdr,
                        "Force SDR (Automatic tone-mapping on HDR displays)",
                    );
                    ui.horizontal(|ui| {
                        if let Some((sw, sh)) = backend.source_dimensions(source_id) {
                            ui.small(format!("Active Capture Size: {sw}x{sh}"));
                        }
                        if ui.button("Fit to Canvas").clicked() {
                            fit_source_items_to_canvas(backend, source_id);
                        }
                    });
                }
                SourceProperties::WindowCapture {
                    window_title,
                    process_name,
                    pid,
                    capture_cursor,
                    client_area_only,
                    capture_method,
                    match_priority,
                } => {
                    let wins = backend.enumerate_windows();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("Window")
                            .selected_text(if window_title.is_empty() {
                                "Select Window..."
                            } else {
                                window_title.as_str()
                            })
                            .show_ui(ui, |ui| {
                                for w in &wins {
                                    if ui
                                        .selectable_label(*window_title == w.title, w.display_label())
                                        .clicked()
                                    {
                                        *window_title = w.title.clone();
                                        *process_name = w.process_name.clone();
                                        *pid = w.pid;
                                    }
                                }
                            });
                        if ui.button("Refresh").clicked() {
                            ctx.request_repaint();
                        }
                    });
                    egui::ComboBox::from_label("Capture Method")
                        .selected_text(capture_method.as_str())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                capture_method,
                                "Auto".to_string(),
                                "Automatic (PrintWindow + BitBlt Fallback)",
                            );
                            ui.selectable_value(
                                capture_method,
                                "PrintWindow".to_string(),
                                "Windows PrintWindow (Works when occluded)",
                            );
                            ui.selectable_value(
                                capture_method,
                                "BitBlt".to_string(),
                                "Win32 BitBlt",
                            );
                        });
                    egui::ComboBox::from_label("Window Match Priority")
                        .selected_text(match match_priority.as_str() {
                            "ExactTitle" => "Window title must match",
                            "ExeOnly" => "Match executable name",
                            _ => "Match title, otherwise executable",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                match_priority,
                                "TitleThenExe".to_string(),
                                "Match title, otherwise executable",
                            );
                            ui.selectable_value(
                                match_priority,
                                "ExactTitle".to_string(),
                                "Window title must match",
                            );
                            ui.selectable_value(
                                match_priority,
                                "ExeOnly".to_string(),
                                "Match executable name",
                            );
                        });
                    ui.label("Color Format: RGBA8 (sRGB)");
                    ui.checkbox(capture_cursor, "Capture Cursor");
                    ui.checkbox(client_area_only, "Client Area");
                    ui.horizontal(|ui| {
                        if let Some((sw, sh)) = backend.source_dimensions(source_id) {
                            ui.small(format!("Active Capture Size: {sw}x{sh}"));
                        }
                        if ui.button("Fit to Canvas").clicked() {
                            fit_source_items_to_canvas(backend, source_id);
                        }
                    });
                }
                SourceProperties::GameCapture {
                    mode,
                    window_title,
                    process_name,
                    pid,
                    capture_cursor,
                    force_scaling,
                    capture_method,
                    scale_width,
                    scale_height,
                    force_sdr,
                } => {
                    egui::ComboBox::from_label("Mode")
                        .selected_text(if mode == "SpecificWindow" {
                            "Capture specific window"
                        } else {
                            "Capture any fullscreen application"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                mode,
                                "AnyFullscreen".to_string(),
                                "Capture any fullscreen application",
                            );
                            ui.selectable_value(
                                mode,
                                "SpecificWindow".to_string(),
                                "Capture specific window",
                            );
                        });
                    if mode == "SpecificWindow" {
                        let self_pid = std::process::id();
                        let wins = backend.enumerate_windows();
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_label("Window")
                                .selected_text(if window_title.is_empty() {
                                    "Select Game/Window..."
                                } else {
                                    window_title.as_str()
                                })
                                .show_ui(ui, |ui| {
                                    for w in wins.iter().filter(|w| w.pid != self_pid) {
                                        if ui
                                            .selectable_label(
                                                *window_title == w.title,
                                                w.display_label(),
                                            )
                                            .clicked()
                                        {
                                            *window_title = w.title.clone();
                                            *process_name = w.process_name.clone();
                                            *pid = w.pid;
                                        }
                                    }
                                });
                            if ui.button("Refresh").clicked() {
                                ctx.request_repaint();
                            }
                        });
                    }
                    egui::ComboBox::from_label("Capture Method")
                        .selected_text(match capture_method.as_str() {
                            "DXGI" => "DXGI Desktop Duplication (Best FPS)",
                            "WindowCapture" => "PrintWindow / BitBlt",
                            _ => "Automatic (DXGI Foreground + PrintWindow)",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                capture_method,
                                "Auto".to_string(),
                                "Automatic (DXGI Foreground + PrintWindow)",
                            );
                            ui.selectable_value(
                                capture_method,
                                "DXGI".to_string(),
                                "DXGI Desktop Duplication (Best FPS)",
                            );
                            ui.selectable_value(
                                capture_method,
                                "WindowCapture".to_string(),
                                "PrintWindow / BitBlt",
                            );
                        });
                    ui.label("Color Format: RGBA8 (sRGB) -> GPU NV12");
                    ui.checkbox(capture_cursor, "Capture Cursor");
                    ui.checkbox(force_sdr, "Force SDR (HDR game compatibility)");
                    ui.checkbox(force_scaling, "Force Scaling");
                    if *force_scaling {
                        ui.horizontal(|ui| {
                            ui.label("Scale Resolution:");
                            ui.add(egui::DragValue::new(scale_width).range(160..=3840).speed(2));
                            ui.label("x");
                            ui.add(egui::DragValue::new(scale_height).range(120..=2160).speed(2));
                            for (rw, rh) in [(1920, 1080), (1280, 720), (2560, 1440)] {
                                if ui.small_button(format!("{rw}x{rh}")).clicked() {
                                    *scale_width = rw;
                                    *scale_height = rh;
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        if let Some((sw, sh)) = backend.source_dimensions(source_id) {
                            ui.small(format!("Active Capture Size: {sw}x{sh}"));
                        }
                        if ui.button("Fit to Canvas").clicked() {
                            fit_source_items_to_canvas(backend, source_id);
                        }
                    });
                    ui.small(
                        "Uses monitor-matched non-invasive DXGI + PrintWindow fallback (works with anti-cheat games).",
                    );
                }
                SourceProperties::VideoCaptureDevice {
                    device_id,
                    device_name,
                    width,
                    height,
                    fps,
                } => {
                    let cams = backend.enumerate_video_devices();
                    egui::ComboBox::from_label("Device")
                        .selected_text(device_name.as_str())
                        .show_ui(ui, |ui| {
                            for c in &cams {
                                if ui.selectable_label(*device_id == c.id, &c.name).clicked() {
                                    *device_id = c.id.clone();
                                    *device_name = c.name.clone();
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("Width:");
                        ui.add(egui::DragValue::new(width).range(160..=3840));
                        ui.label("Height:");
                        ui.add(egui::DragValue::new(height).range(120..=2160));
                        ui.label("FPS:");
                        ui.add(egui::DragValue::new(fps).range(5..=60));
                    });
                }
                SourceProperties::AudioInputCapture {
                    device_id,
                    device_name,
                } => {
                    let devs = backend.enumerate_audio_inputs();
                    egui::ComboBox::from_label("Device")
                        .selected_text(device_name.as_str())
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(*device_id == "default", "Default")
                                .clicked()
                            {
                                *device_id = "default".to_string();
                                *device_name = "Default".to_string();
                            }
                            for d in &devs {
                                if ui.selectable_label(*device_id == d.id, &d.name).clicked() {
                                    *device_id = d.id.clone();
                                    *device_name = d.name.clone();
                                }
                            }
                        });
                }
                SourceProperties::AudioOutputCapture {
                    device_id,
                    device_name,
                } => {
                    let devs = backend.enumerate_audio_outputs();
                    egui::ComboBox::from_label("Device")
                        .selected_text(device_name.as_str())
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(*device_id == "default", "Default")
                                .clicked()
                            {
                                *device_id = "default".to_string();
                                *device_name = "Default".to_string();
                            }
                            for d in &devs {
                                if ui.selectable_label(*device_id == d.id, &d.name).clicked() {
                                    *device_id = d.id.clone();
                                    *device_name = d.name.clone();
                                }
                            }
                        });
                }
                SourceProperties::ApplicationAudioCapture {
                    pid,
                    process_name,
                    window_title,
                } => {
                    let wins = backend.enumerate_windows();
                    egui::ComboBox::from_label("Application Window")
                        .selected_text(if window_title.is_empty() {
                            "Select Application...".to_string()
                        } else {
                            format!("[{process_name}] {window_title} (PID {pid})")
                        })
                        .show_ui(ui, |ui| {
                            for w in &wins {
                                if ui
                                    .selectable_label(*pid == w.pid, w.display_label())
                                    .clicked()
                                {
                                    *pid = w.pid;
                                    *process_name = w.process_name.clone();
                                    *window_title = w.title.clone();
                                }
                            }
                        });
                }
                SourceProperties::Image {
                    file_path,
                    unload_when_not_showing,
                } => {
                    ui.horizontal(|ui| {
                        ui.label("Image File:");
                        ui.text_edit_singleline(file_path);
                        if ui.button("Browse").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter(
                                    "Images",
                                    &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tga"],
                                )
                                .pick_file()
                            {
                                *file_path = p.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.checkbox(unload_when_not_showing, "Unload image when not showing");
                }
                SourceProperties::ImageSlideshow {
                    files,
                    slide_time_ms,
                    loop_slides,
                    randomize,
                    paused,
                    ..
                } => {
                    ui.horizontal(|ui| {
                        ui.label("Slide Time (ms):");
                        ui.add(egui::DragValue::new(slide_time_ms).range(300..=60000));
                    });
                    ui.checkbox(loop_slides, "Loop");
                    ui.checkbox(randomize, "Randomize playback");
                    ui.checkbox(paused, "Paused");
                    ui.horizontal(|ui| {
                        if ui.button("Add Files").clicked() {
                            if let Some(picks) = rfd::FileDialog::new()
                                .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "webp"])
                                .pick_files()
                            {
                                for p in picks {
                                    files.push(p.to_string_lossy().to_string());
                                }
                            }
                        }
                        if ui.button("Clear").clicked() {
                            files.clear();
                        }
                        if ui.button("Prev").clicked() {
                            backend.slideshow_prev(source_id);
                        }
                        if ui.button("Next").clicked() {
                            backend.media_next(source_id);
                        }
                    });
                    ui.small(format!("{} slides configured", files.len()));
                }
                SourceProperties::MediaSource {
                    local_file,
                    is_network_stream,
                    loop_playback,
                    speed_percent,
                    paused,
                    ..
                } => {
                    ui.checkbox(is_network_stream, "Network Stream / RTSP / HTTP");
                    ui.horizontal(|ui| {
                        ui.label("Input:");
                        ui.text_edit_singleline(local_file);
                        if !*is_network_stream && ui.button("Browse").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                *local_file = p.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.checkbox(loop_playback, "Loop");
                    ui.checkbox(paused, "Paused");
                    ui.horizontal(|ui| {
                        ui.label("Speed (%):");
                        ui.add(egui::Slider::new(speed_percent, 25..=300));
                        if ui.button("Restart").clicked() {
                            backend.media_restart(source_id);
                        }
                    });
                }
                SourceProperties::PlaylistSource {
                    files,
                    loop_playlist,
                    shuffle,
                    paused,
                } => {
                    ui.checkbox(loop_playlist, "Loop Playlist");
                    ui.checkbox(shuffle, "Shuffle");
                    ui.checkbox(paused, "Paused");
                    ui.horizontal(|ui| {
                        if ui.button("Add Media Files").clicked() {
                            if let Some(picks) = rfd::FileDialog::new().pick_files() {
                                for p in picks {
                                    files.push(p.to_string_lossy().to_string());
                                }
                            }
                        }
                        if ui.button("Next Track").clicked() {
                            backend.media_next(source_id);
                        }
                        if ui.button("Restart").clicked() {
                            backend.media_restart(source_id);
                        }
                        if ui.button("Clear").clicked() {
                            files.clear();
                        }
                    });
                    for f in files.iter().take(6) {
                        ui.small(f);
                    }
                }
                SourceProperties::TextSource {
                    text,
                    font_family,
                    font_size_px,
                    color_rgba,
                    gradient_enabled,
                    gradient_bottom_rgba,
                    outline_enabled,
                    outline_size,
                    outline_color_rgba,
                    drop_shadow,
                    align,
                    read_from_file,
                    file_path,
                } => {
                    let fonts = backend.system_fonts();
                    egui::ComboBox::from_label("Font")
                        .selected_text(font_family.as_str())
                        .show_ui(ui, |ui| {
                            for f in &fonts {
                                ui.selectable_value(font_family, f.clone(), f);
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("Font Size:");
                        ui.add(egui::Slider::new(font_size_px, 12.0..=180.0));
                        ui.label("Color:");
                        ui.color_edit_button_srgba_unmultiplied(color_rgba);
                    });
                    ui.checkbox(read_from_file, "Read from file");
                    if *read_from_file {
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(file_path);
                            if ui.button("Browse").clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_file() {
                                    *file_path = p.to_string_lossy().to_string();
                                }
                            }
                        });
                    } else {
                        ui.label("Text:");
                        ui.text_edit_multiline(text);
                    }
                    ui.horizontal(|ui| {
                        ui.checkbox(gradient_enabled, "Gradient");
                        if *gradient_enabled {
                            ui.color_edit_button_srgba_unmultiplied(gradient_bottom_rgba);
                        }
                        ui.checkbox(outline_enabled, "Outline");
                        if *outline_enabled {
                            ui.add(egui::DragValue::new(outline_size).range(1.0..=8.0));
                            ui.color_edit_button_srgba_unmultiplied(outline_color_rgba);
                        }
                        ui.checkbox(drop_shadow, "Drop Shadow");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Align:");
                        ui.selectable_value(align, "left".to_string(), "Left");
                        ui.selectable_value(align, "center".to_string(), "Center");
                        ui.selectable_value(align, "right".to_string(), "Right");
                    });
                }
                SourceProperties::ColorSource {
                    color_rgba,
                    width,
                    height,
                } => {
                    ui.horizontal(|ui| {
                        ui.label("Color:");
                        ui.color_edit_button_srgba_unmultiplied(color_rgba);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Width:");
                        ui.add(egui::DragValue::new(width).range(8..=3840));
                        ui.label("Height:");
                        ui.add(egui::DragValue::new(height).range(8..=2160));
                    });
                }
                SourceProperties::BrowserSource {
                    url,
                    width,
                    height,
                    fps,
                    custom_css,
                    ..
                } => {
                    ui.horizontal(|ui| {
                        ui.label("URL:");
                        ui.text_edit_singleline(url);
                        if ui.button("Refresh").clicked() {
                            backend.browser_refresh(source_id);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Width:");
                        ui.add(egui::DragValue::new(width).range(64..=3840));
                        ui.label("Height:");
                        ui.add(egui::DragValue::new(height).range(64..=2160));
                        ui.label("FPS:");
                        ui.add(egui::DragValue::new(fps).range(1..=10));
                    });
                    ui.label("Custom CSS:");
                    ui.text_edit_multiline(custom_css);
                }
                SourceProperties::SceneSource { scene_id } => {
                    egui::ComboBox::from_label("Target Scene")
                        .selected_text(
                            col.scene_by_id(*scene_id)
                                .map(|s| s.name.as_str())
                                .unwrap_or("Select Scene"),
                        )
                        .show_ui(ui, |ui| {
                            for sc in &col.scenes {
                                if sc.id != col.active_scene_id {
                                    ui.selectable_value(scene_id, sc.id, &sc.name);
                                }
                            }
                        });
                }
                SourceProperties::GroupSource | SourceProperties::PluginSource { .. } => {
                    ui.label("Configured via Scene / Plugin panel.");
                }
            }

            ui.separator();
            if ui.button("Close").clicked() {
                *source_id_opt = None;
            }
        });

    backend.with_collection_mut(&mut |c| {
        if let Some(s) = c.sources.get_mut(&source_id) {
            s.name = name.clone();
            s.properties = props.clone();
        }
    });

    if !open {
        *source_id_opt = None;
    }
}

fn fit_items_recursive(
    items: &mut [obs_scenes::SceneItem],
    source_id: SourceId,
    src_size: [f32; 2],
    canvas: [f32; 2],
) {
    for it in items {
        if it.source_id == source_id {
            SceneCollection::fit_item_to_canvas(&mut it.transform, src_size, canvas);
        }
        if let Some(children) = &mut it.group_children {
            fit_items_recursive(children, source_id, src_size, canvas);
        }
    }
}

fn fit_source_items_to_canvas(backend: &dyn StudioBackend, source_id: SourceId) {
    let (cw, ch) = backend.canvas_size();
    let canvas = [cw as f32, ch as f32];
    let src_size = backend
        .source_dimensions(source_id)
        .map(|(w, h)| [w as f32, h as f32])
        .unwrap_or(canvas);
    backend.with_collection_mut(&mut |c| {
        for sc in &mut c.scenes {
            fit_items_recursive(&mut sc.items, source_id, src_size, canvas);
        }
    });
}

pub fn render_filters_modal(
    ctx: &egui::Context,
    source_id_opt: &mut Option<SourceId>,
    backend: &dyn StudioBackend,
) {
    let Some(source_id) = *source_id_opt else {
        return;
    };
    let col = backend.collection();
    let Some(mut src) = col.sources.get(&source_id).cloned() else {
        *source_id_opt = None;
        return;
    };

    let mut open = true;
    egui::Window::new(format!("Filters for '{}'", src.name))
        .open(&mut open)
        .default_size([700.0, 480.0])
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                // Left: Video / Effect Filters
                cols[0].heading("Effect (Video) Filters");
                cols[0].horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("add_vflt")
                        .selected_text("+ Add Video Filter")
                        .show_ui(ui, |ui| {
                            for (name, kind) in [
                                ("Chroma Key", VideoFilterKind::default_chroma_key()),
                                ("Color Key", VideoFilterKind::default_color_key()),
                                ("Luma Key", VideoFilterKind::default_luma_key()),
                                ("Color Correction", VideoFilterKind::default_color_correction()),
                                ("Crop/Pad", VideoFilterKind::default_crop_pad()),
                                ("Scaling/Aspect Ratio", VideoFilterKind::default_scaling_aspect()),
                                ("Scroll", VideoFilterKind::default_scroll()),
                                ("Sharpen", VideoFilterKind::default_sharpen()),
                                ("Image Mask/Blend", VideoFilterKind::default_image_mask()),
                                ("Render Delay", VideoFilterKind::default_render_delay()),
                                ("Custom WGSL Shader", VideoFilterKind::default_custom_wgsl()),
                            ] {
                                if ui.button(name).clicked() {
                                    src.video_filters.push(VideoFilterInstance::new(name, kind));
                                    ui.close_menu();
                                }
                            }
                            for (pid, fid, fname) in backend.available_plugin_filters() {
                                if ui.button(&fname).clicked() {
                                    src.video_filters.push(VideoFilterInstance::new(
                                        fname,
                                        VideoFilterKind::PluginVideoFilter {
                                            plugin_id: pid,
                                            filter_id: fid,
                                            settings: serde_json::json!({ "strength": 0.55, "radius": 0.75 }),
                                        },
                                    ));
                                    ui.close_menu();
                                }
                            }
                        });
                });

                egui::ScrollArea::vertical().id_salt("vflt_scroll").max_height(360.0).show(&mut cols[0], |ui| {
                    let mut remove_idx = None;
                    for (idx, flt) in src.video_filters.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut flt.enabled, RichText::new(&flt.name).strong());
                                if ui.small_button("Remove").clicked() {
                                    remove_idx = Some(idx);
                                }
                            });
                            render_video_filter_editor(ui, &mut flt.kind);
                        });
                    }
                    if let Some(i) = remove_idx {
                        src.video_filters.remove(i);
                    }
                });

                // Right: Audio Filters
                cols[1].heading("Audio Filters");
                cols[1].horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("add_aflt")
                        .selected_text("+ Add Audio Filter")
                        .show_ui(ui, |ui| {
                            for (name, kind) in [
                                ("Gain", AudioFilterKind::default_gain()),
                                ("Noise Gate", AudioFilterKind::default_noise_gate()),
                                ("Compressor", AudioFilterKind::default_compressor()),
                                ("Limiter", AudioFilterKind::default_limiter()),
                                ("Expander", AudioFilterKind::default_expander()),
                                ("Noise Suppression", AudioFilterKind::default_noise_suppression()),
                                ("3-Band Equalizer", AudioFilterKind::default_three_band_eq()),
                                ("Invert Polarity", AudioFilterKind::InvertPolarity),
                            ] {
                                if ui.button(name).clicked() {
                                    src.audio_filters.push(AudioFilterInstance::new(name, kind));
                                    ui.close_menu();
                                }
                            }
                        });
                });

                egui::ScrollArea::vertical().id_salt("aflt_scroll").max_height(360.0).show(&mut cols[1], |ui| {
                    let mut remove_idx = None;
                    for (idx, flt) in src.audio_filters.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut flt.enabled, RichText::new(&flt.name).strong());
                                if ui.small_button("Remove").clicked() {
                                    remove_idx = Some(idx);
                                }
                            });
                            render_audio_filter_editor(ui, &mut flt.kind);
                        });
                    }
                    if let Some(i) = remove_idx {
                        src.audio_filters.remove(i);
                    }
                });
            });
            ui.separator();
            if ui.button("Close").clicked() {
                *source_id_opt = None;
            }
        });

    backend.with_collection_mut(&mut |c| {
        if let Some(dst) = c.sources.get_mut(&source_id) {
            dst.video_filters = src.video_filters.clone();
            dst.audio_filters = src.audio_filters.clone();
        }
    });
    if !open {
        *source_id_opt = None;
    }
}

fn render_video_filter_editor(ui: &mut Ui, kind: &mut VideoFilterKind) {
    match kind {
        VideoFilterKind::ChromaKey {
            preset,
            custom_color_rgba,
            similarity,
            smoothness,
            key_color_spill_reduction,
            opacity,
            contrast,
            brightness,
            gamma,
        } => {
            ui.horizontal(|ui| {
                ui.label("Key Color:");
                for p in [
                    ChromaKeyPreset::Green,
                    ChromaKeyPreset::Blue,
                    ChromaKeyPreset::Magenta,
                    ChromaKeyPreset::Custom,
                ] {
                    ui.selectable_value(preset, p, p.display_name());
                }
                if *preset == ChromaKeyPreset::Custom {
                    ui.color_edit_button_srgba_unmultiplied(custom_color_rgba);
                }
            });
            ui.add(egui::Slider::new(similarity, 0.01..=1.0).text("Similarity"));
            ui.add(egui::Slider::new(smoothness, 0.001..=1.0).text("Smoothness"));
            ui.add(
                egui::Slider::new(key_color_spill_reduction, 0.001..=1.0).text("Spill Reduction"),
            );
            ui.add(egui::Slider::new(opacity, 0.0..=1.0).text("Opacity"));
            ui.add(egui::Slider::new(contrast, -1.0..=1.0).text("Contrast"));
            ui.add(egui::Slider::new(brightness, -1.0..=1.0).text("Brightness"));
            ui.add(egui::Slider::new(gamma, -1.0..=1.0).text("Gamma"));
        }
        VideoFilterKind::ColorKey {
            key_color_rgba,
            similarity,
            smoothness,
            opacity,
            ..
        } => {
            ui.horizontal(|ui| {
                ui.label("Color:");
                ui.color_edit_button_srgba_unmultiplied(key_color_rgba);
            });
            ui.add(egui::Slider::new(similarity, 0.01..=1.0).text("Similarity"));
            ui.add(egui::Slider::new(smoothness, 0.001..=1.0).text("Smoothness"));
            ui.add(egui::Slider::new(opacity, 0.0..=1.0).text("Opacity"));
        }
        VideoFilterKind::LumaKey {
            luma_max,
            luma_max_smooth,
            luma_min,
            luma_min_smooth,
        } => {
            ui.add(egui::Slider::new(luma_max, 0.0..=1.0).text("Luma Max"));
            ui.add(egui::Slider::new(luma_max_smooth, 0.0..=1.0).text("Luma Max Smooth"));
            ui.add(egui::Slider::new(luma_min, 0.0..=1.0).text("Luma Min"));
            ui.add(egui::Slider::new(luma_min_smooth, 0.0..=1.0).text("Luma Min Smooth"));
        }
        VideoFilterKind::ColorCorrection {
            gamma,
            contrast,
            brightness,
            saturation,
            hue_shift_deg,
            opacity,
            color_multiply_rgba,
            ..
        } => {
            ui.add(egui::Slider::new(gamma, -1.0..=2.0).text("Gamma"));
            ui.add(egui::Slider::new(contrast, -1.0..=2.0).text("Contrast"));
            ui.add(egui::Slider::new(brightness, -1.0..=1.0).text("Brightness"));
            ui.add(egui::Slider::new(saturation, -1.0..=2.0).text("Saturation"));
            ui.add(egui::Slider::new(hue_shift_deg, -180.0..=180.0).text("Hue Shift"));
            ui.add(egui::Slider::new(opacity, 0.0..=1.0).text("Opacity"));
            ui.horizontal(|ui| {
                ui.label("Color Multiply:");
                ui.color_edit_button_srgba_unmultiplied(color_multiply_rgba);
            });
        }
        VideoFilterKind::CropPad {
            left,
            top,
            right,
            bottom,
            ..
        } => {
            ui.horizontal(|ui| {
                ui.label("L:");
                ui.add(egui::DragValue::new(left).range(0..=1920));
                ui.label("T:");
                ui.add(egui::DragValue::new(top).range(0..=1080));
                ui.label("R:");
                ui.add(egui::DragValue::new(right).range(0..=1920));
                ui.label("B:");
                ui.add(egui::DragValue::new(bottom).range(0..=1080));
            });
        }
        VideoFilterKind::ScalingAspect {
            target_width,
            target_height,
            ..
        } => {
            ui.horizontal(|ui| {
                ui.label("Size:");
                ui.add(egui::DragValue::new(target_width).range(16..=3840));
                ui.label("x");
                ui.add(egui::DragValue::new(target_height).range(16..=2160));
            });
        }
        VideoFilterKind::Scroll {
            speed_x,
            speed_y,
            loop_scroll,
            ..
        } => {
            ui.add(egui::Slider::new(speed_x, -500.0..=500.0).text("Horizontal Speed"));
            ui.add(egui::Slider::new(speed_y, -500.0..=500.0).text("Vertical Speed"));
            ui.checkbox(loop_scroll, "Loop");
        }
        VideoFilterKind::Sharpen { sharpness } => {
            ui.add(egui::Slider::new(sharpness, 0.0..=1.0).text("Sharpness"));
        }
        VideoFilterKind::ImageMaskBlend {
            mask_type,
            image_path,
            color_rgba,
            opacity,
            ..
        } => {
            egui::ComboBox::from_id_salt("mask_type")
                .selected_text(mask_type.display_name())
                .show_ui(ui, |ui| {
                    for mt in [
                        MaskBlendType::AlphaMaskColorChannel,
                        MaskBlendType::AlphaMaskAlphaChannel,
                        MaskBlendType::BlendMultiply,
                        MaskBlendType::BlendAddition,
                        MaskBlendType::BlendSubtraction,
                    ] {
                        ui.selectable_value(mask_type, mt, mt.display_name());
                    }
                });
            ui.horizontal(|ui| {
                ui.text_edit_singleline(image_path);
                if ui.button("Mask File").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                        *image_path = p.to_string_lossy().to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Color:");
                ui.color_edit_button_srgba_unmultiplied(color_rgba);
                ui.add(egui::Slider::new(opacity, 0.0..=1.0).text("Opacity"));
            });
        }
        VideoFilterKind::RenderDelay { delay_ms } => {
            ui.add(egui::Slider::new(delay_ms, 0..=2000).text("Delay (ms)"));
        }
        VideoFilterKind::CustomWgslShader {
            wgsl_source,
            param_a,
            param_b,
            ..
        } => {
            ui.add(egui::Slider::new(param_a, 0.0..=2.0).text("Param A"));
            ui.add(egui::Slider::new(param_b, 0.0..=2.0).text("Param B"));
            ui.add(
                egui::TextEdit::multiline(wgsl_source)
                    .code_editor()
                    .desired_rows(5),
            );
        }
        VideoFilterKind::PluginVideoFilter { settings, .. } => {
            let mut strength = settings
                .get("strength")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.55) as f32;
            let mut radius = settings
                .get("radius")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.75) as f32;
            if ui
                .add(egui::Slider::new(&mut strength, 0.0..=1.0).text("Strength"))
                .changed()
            {
                settings["strength"] = serde_json::json!(strength);
            }
            if ui
                .add(egui::Slider::new(&mut radius, 0.2..=1.5).text("Radius"))
                .changed()
            {
                settings["radius"] = serde_json::json!(radius);
            }
        }
    }
}

fn render_audio_filter_editor(ui: &mut Ui, kind: &mut AudioFilterKind) {
    match kind {
        AudioFilterKind::Gain { gain_db } => {
            ui.add(egui::Slider::new(gain_db, -30.0..=30.0).text("Gain (dB)"));
        }
        AudioFilterKind::NoiseGate {
            close_threshold_db,
            open_threshold_db,
            attack_ms,
            hold_ms,
            release_ms,
        } => {
            ui.add(egui::Slider::new(close_threshold_db, -96.0..=0.0).text("Close Threshold (dB)"));
            ui.add(egui::Slider::new(open_threshold_db, -96.0..=0.0).text("Open Threshold (dB)"));
            ui.add(egui::Slider::new(attack_ms, 1.0..=500.0).text("Attack (ms)"));
            ui.add(egui::Slider::new(hold_ms, 0.0..=1000.0).text("Hold (ms)"));
            ui.add(egui::Slider::new(release_ms, 1.0..=1000.0).text("Release (ms)"));
        }
        AudioFilterKind::Compressor {
            ratio,
            threshold_db,
            attack_ms,
            release_ms,
            output_gain_db,
        } => {
            ui.add(egui::Slider::new(ratio, 1.0..=32.0).text("Ratio"));
            ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("Threshold (dB)"));
            ui.add(egui::Slider::new(attack_ms, 1.0..=500.0).text("Attack (ms)"));
            ui.add(egui::Slider::new(release_ms, 1.0..=1000.0).text("Release (ms)"));
            ui.add(egui::Slider::new(output_gain_db, -32.0..=32.0).text("Output Gain (dB)"));
        }
        AudioFilterKind::Limiter {
            threshold_db,
            release_ms,
        } => {
            ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("Threshold (dB)"));
            ui.add(egui::Slider::new(release_ms, 1.0..=1000.0).text("Release (ms)"));
        }
        AudioFilterKind::Expander {
            preset,
            ratio,
            threshold_db,
            attack_ms,
            release_ms,
            output_gain_db,
        } => {
            ui.horizontal(|ui| {
                ui.selectable_value(preset, ExpanderPreset::Expander, "Expander");
                ui.selectable_value(preset, ExpanderPreset::Gate, "Gate");
            });
            ui.add(egui::Slider::new(ratio, 1.0..=20.0).text("Ratio"));
            ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("Threshold (dB)"));
            ui.add(egui::Slider::new(attack_ms, 1.0..=500.0).text("Attack (ms)"));
            ui.add(egui::Slider::new(release_ms, 1.0..=1000.0).text("Release (ms)"));
            ui.add(egui::Slider::new(output_gain_db, -32.0..=32.0).text("Output Gain (dB)"));
        }
        AudioFilterKind::NoiseSuppression {
            method,
            suppression_level_db,
        } => {
            egui::ComboBox::from_id_salt("ns_method")
                .selected_text(method.display_name())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        method,
                        NoiseSuppressionMethod::AdaptiveSpectral,
                        "RNNoise-style Adaptive",
                    );
                    ui.selectable_value(
                        method,
                        NoiseSuppressionMethod::FastNoiseGate,
                        "Speex-style Low CPU",
                    );
                });
            ui.add(
                egui::Slider::new(suppression_level_db, -60.0..=0.0).text("Suppression Level (dB)"),
            );
        }
        AudioFilterKind::ThreeBandEq {
            low_db,
            mid_db,
            high_db,
        } => {
            ui.add(egui::Slider::new(low_db, -20.0..=20.0).text("Low (dB)"));
            ui.add(egui::Slider::new(mid_db, -20.0..=20.0).text("Mid (dB)"));
            ui.add(egui::Slider::new(high_db, -20.0..=20.0).text("High (dB)"));
        }
        AudioFilterKind::InvertPolarity => {
            ui.label("Inverts audio waveform polarity (180 deg phase shift).");
        }
    }
}

pub fn render_advanced_audio_modal(
    ctx: &egui::Context,
    open: &mut bool,
    backend: &dyn StudioBackend,
) {
    if !*open {
        return;
    }
    let mut win_open = *open;
    let col = backend.collection();
    let mut audio_updates: Vec<(SourceId, obs_scenes::SourceAudioSettings)> = col
        .sources
        .values()
        .filter(|s| s.kind.has_audio())
        .map(|s| (s.id, s.audio.clone()))
        .collect();

    egui::Window::new("Advanced Audio Properties")
        .open(&mut win_open)
        .default_width(780.0)
        .show(ctx, |ui| {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                egui::Grid::new("adv_aud_grid")
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Name").strong());
                        ui.label(RichText::new("Volume").strong());
                        ui.label(RichText::new("Mono").strong());
                        ui.label(RichText::new("Balance").strong());
                        ui.label(RichText::new("Sync Offset (ms)").strong());
                        ui.label(RichText::new("Audio Monitoring").strong());
                        ui.label(RichText::new("Tracks (1-6)").strong());
                        ui.end_row();

                        for (id, aud) in audio_updates.iter_mut() {
                            let name = col
                                .sources
                                .get(id)
                                .map(|s| s.name.as_str())
                                .unwrap_or("Source");
                            ui.label(name);
                            let mut pct = aud.volume_mul * 100.0;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut pct)
                                        .range(0.0..=200.0)
                                        .suffix("%"),
                                )
                                .changed()
                            {
                                aud.volume_mul = pct / 100.0;
                            }
                            ui.checkbox(&mut aud.mono_downmix, "");
                            ui.add(
                                egui::Slider::new(&mut aud.balance, -1.0..=1.0).show_value(false),
                            );
                            ui.add(
                                egui::DragValue::new(&mut aud.sync_offset_ms)
                                    .range(0..=2000)
                                    .suffix(" ms"),
                            );
                            egui::ComboBox::from_id_salt(format!("mon_{id}"))
                                .selected_text(aud.monitoring.display_name())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut aud.monitoring,
                                        AudioMonitoringType::None,
                                        "Monitor Off",
                                    );
                                    ui.selectable_value(
                                        &mut aud.monitoring,
                                        AudioMonitoringType::MonitorOnly,
                                        "Monitor Only (mute output)",
                                    );
                                    ui.selectable_value(
                                        &mut aud.monitoring,
                                        AudioMonitoringType::MonitorAndOutput,
                                        "Monitor and Output",
                                    );
                                });
                            ui.horizontal(|ui| {
                                for t in 0..6u8 {
                                    let mut on = (aud.track_mask & (1 << t)) != 0;
                                    if ui.checkbox(&mut on, format!("{}", t + 1)).changed() {
                                        if on {
                                            aud.track_mask |= 1 << t;
                                        } else {
                                            aud.track_mask &= !(1 << t);
                                        }
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
            ui.separator();
            if ui.button("Close").clicked() {
                *open = false;
            }
        });

    backend.with_collection_mut(&mut |c| {
        for (id, aud) in &audio_updates {
            if let Some(s) = c.sources.get_mut(id) {
                s.audio = aud.clone();
            }
        }
    });
    if !win_open {
        *open = false;
    }
}

pub fn render_transition_config_modal(
    ctx: &egui::Context,
    tr_id_opt: &mut Option<TransitionId>,
    backend: &dyn StudioBackend,
) {
    let Some(tr_id) = *tr_id_opt else {
        return;
    };
    let col = backend.collection();
    let Some(mut tr) = col.transitions.iter().find(|t| t.id == tr_id).cloned() else {
        *tr_id_opt = None;
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Properties for '{}'", tr.name))
        .open(&mut open)
        .default_width(380.0)
        .show(ctx, |ui| {
            match &mut tr.kind {
                TransitionKind::Cut => {
                    ui.label("Cut transition switches immediately without parameters.");
                }
                TransitionKind::Fade => {
                    ui.label("Smooth crossfade transition between scenes.");
                }
                TransitionKind::Swipe {
                    direction,
                    swipe_in,
                } => {
                    egui::ComboBox::from_label("Direction")
                        .selected_text(direction.display_name())
                        .show_ui(ui, |ui| {
                            for d in [
                                TransitionDirection::Left,
                                TransitionDirection::Right,
                                TransitionDirection::Up,
                                TransitionDirection::Down,
                            ] {
                                ui.selectable_value(direction, d, d.display_name());
                            }
                        });
                    ui.checkbox(swipe_in, "Swipe In");
                }
                TransitionKind::Slide { direction } => {
                    egui::ComboBox::from_label("Direction")
                        .selected_text(direction.display_name())
                        .show_ui(ui, |ui| {
                            for d in [
                                TransitionDirection::Left,
                                TransitionDirection::Right,
                                TransitionDirection::Up,
                                TransitionDirection::Down,
                            ] {
                                ui.selectable_value(direction, d, d.display_name());
                            }
                        });
                }
                TransitionKind::FadeToColor {
                    color_rgba,
                    switch_point,
                } => {
                    ui.horizontal(|ui| {
                        ui.label("Color:");
                        ui.color_edit_button_srgba_unmultiplied(color_rgba);
                    });
                    ui.add(egui::Slider::new(switch_point, 0.1..=0.9).text("Color Switch Point"));
                }
                TransitionKind::LumaWipe {
                    wipe_name,
                    softness,
                    invert,
                } => {
                    let wipes = [
                        "linear-h.png",
                        "linear-v.png",
                        "barndoor-h.png",
                        "barndoor-v.png",
                        "blinds-h.png",
                        "clock.png",
                        "iris.png",
                        "spiral.png",
                        "checkerboard-small.png",
                        "cloud.png",
                        "curtain.png",
                    ];
                    egui::ComboBox::from_label("Image Pattern")
                        .selected_text(wipe_name.as_str())
                        .show_ui(ui, |ui| {
                            for w in wipes {
                                ui.selectable_value(wipe_name, w.to_string(), w);
                            }
                        });
                    ui.add(egui::Slider::new(softness, 0.01..=0.8).text("Softness"));
                    ui.checkbox(invert, "Invert");
                }
                TransitionKind::Stinger {
                    video_path,
                    transition_point_ms,
                } => {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(video_path);
                        if ui.button("Browse").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                *video_path = p.to_string_lossy().to_string();
                            }
                        }
                    });
                    ui.add(egui::DragValue::new(transition_point_ms).range(0..=10000));
                }
            }
            ui.separator();
            if ui.button("OK").clicked() {
                *tr_id_opt = None;
            }
        });
    backend.with_collection_mut(&mut |c| {
        if let Some(dst) = c.transitions.iter_mut().find(|t| t.id == tr_id) {
            dst.kind = tr.kind.clone();
        }
    });
    if !open {
        *tr_id_opt = None;
    }
}

#[derive(Default)]
pub struct RemuxModalState {
    pub open: bool,
    pub src_path: String,
    pub dst_path: String,
    pub result_msg: Option<String>,
}

pub fn render_remux_modal(
    ctx: &egui::Context,
    st: &mut RemuxModalState,
    backend: &dyn StudioBackend,
) {
    if !st.open {
        return;
    }
    let mut open = st.open;
    egui::Window::new("Remux Recordings")
        .open(&mut open)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.label("Losslessly repackage MKV/FLV/MOV recordings into fast-start MP4:");
            ui.horizontal(|ui| {
                ui.label("Source:");
                ui.text_edit_singleline(&mut st.src_path);
                if ui.button("Browse").clicked() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Recordings", &["mkv", "flv", "mov"])
                        .pick_file()
                    {
                        st.dst_path = p.with_extension("mp4").to_string_lossy().to_string();
                        st.src_path = p.to_string_lossy().to_string();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Target MP4:");
                ui.text_edit_singleline(&mut st.dst_path);
            });
            if let Some(msg) = &st.result_msg {
                ui.label(msg);
            }
            ui.horizontal(|ui| {
                if ui.button("Remux").clicked() && !st.src_path.trim().is_empty() {
                    let dst_opt = if st.dst_path.trim().is_empty() {
                        None
                    } else {
                        Some(std::path::Path::new(&st.dst_path))
                    };
                    match backend.remux_recording(std::path::Path::new(&st.src_path), dst_opt) {
                        Ok(out) => {
                            st.result_msg = Some(format!("Remux completed: {}", out.display()));
                        }
                        Err(e) => {
                            st.result_msg = Some(format!("Error: {e}"));
                        }
                    }
                }
                if ui.button("Close").clicked() {
                    st.open = false;
                }
            });
        });
    if !open {
        st.open = false;
    }
}

pub struct AutoConfigModalState {
    pub open: bool,
    pub optimize_for_streaming: bool,
    pub target_fps: u32,
}

impl Default for AutoConfigModalState {
    fn default() -> Self {
        Self {
            open: false,
            optimize_for_streaming: true,
            target_fps: 60,
        }
    }
}

pub fn render_autoconfig_modal(
    ctx: &egui::Context,
    st: &mut AutoConfigModalState,
    backend: &dyn StudioBackend,
    toast: &mut crate::ToastQueue,
) {
    if !st.open {
        return;
    }
    let mut open = st.open;
    egui::Window::new("Auto-Configuration Wizard")
        .open(&mut open)
        .default_width(440.0)
        .show(ctx, |ui| {
            ui.label("Automatically configure OxideStudio based on your GPU, display, and probed encoders.");
            ui.radio_value(&mut st.optimize_for_streaming, true, "Optimize for streaming, recording is secondary");
            ui.radio_value(&mut st.optimize_for_streaming, false, "Optimize just for recording, I will not be streaming");
            ui.horizontal(|ui| {
                ui.label("Target FPS:");
                ui.selectable_value(&mut st.target_fps, 60, "60 FPS");
                ui.selectable_value(&mut st.target_fps, 30, "30 FPS");
            });

            let mons = backend.enumerate_monitors();
            let rep = backend.reprobe_encoders();
            let primary = mons.first();
            let (bw, bh) = primary.map(|m| (m.width, m.height)).unwrap_or((1920, 1080));
            let best_enc = obs_encoder::resolve_best_video_encoder("auto", &rep);

            ui.separator();
            ui.label(format!("Detected Display: {bw}x{bh}"));
            ui.label(format!("Best Verified Encoder: {best_enc}"));
            ui.label(format!(
                "Recommended Bitrate: {} Kbps",
                if st.optimize_for_streaming { 6000 } else { 20000 }
            ));

            ui.horizontal(|ui| {
                if ui.button("Apply Settings").clicked() {
                    let mut cfg = backend.settings();
                    cfg.video.config.canvas_width = (bw / 2) * 2;
                    cfg.video.config.canvas_height = (bh / 2) * 2;
                    cfg.video.config.output_width = if st.optimize_for_streaming { 1920.min((bw / 2) * 2) } else { (bw / 2) * 2 };
                    cfg.video.config.output_height = if st.optimize_for_streaming { 1080.min((bh / 2) * 2) } else { (bh / 2) * 2 };
                    cfg.video.config.fps_num = st.target_fps;
                    cfg.video.config.fps_den = 1;
                    cfg.output.stream_video_encoder = best_enc.clone();
                    cfg.output.rec_video_encoder = best_enc;
                    cfg.output.stream_bitrate_kbps = 6000;
                    cfg.output.rec_bitrate_kbps = 20000;
                    match backend.apply_settings(cfg) {
                        Ok(()) => {
                            toast.push_info("Auto-Configuration applied.");
                            st.open = false;
                        }
                        Err(e) => toast.push_error(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    st.open = false;
                }
            });
        });
    if !open {
        st.open = false;
    }
}

pub fn render_about_modal(ctx: &egui::Context, open: &mut bool, backend: &dyn StudioBackend) {
    if !*open {
        return;
    }
    let mut win_open = *open;
    let rep = backend.encoder_report();
    egui::Window::new("About OxideStudio (OBS-Rust)")
        .open(&mut win_open)
        .default_width(540.0)
        .show(ctx, |ui| {
            ui.heading(format!("OxideStudio v{}", env!("CARGO_PKG_VERSION")));
            ui.label("Independent Rust rebuild of OBS Studio architecture — GPL-2.0 License.");
            ui.small("Not affiliated with or endorsed by the OBS Project.");
            ui.separator();
            ui.label(RichText::new("Verified Encoders on This Machine:").strong());
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for e in &rep.video_encoders {
                        let color = if e.available {
                            Color32::LIGHT_GREEN
                        } else {
                            Color32::GRAY
                        };
                        let tag = if e.available { "READY" } else { "UNAVAILABLE" };
                        ui.horizontal(|ui| {
                            ui.colored_label(color, format!("[{tag}] {}", e.display_name));
                            if !e.available {
                                ui.small(&e.reason);
                            }
                        });
                    }
                });
            ui.separator();
            if ui.button("Close").clicked() {
                *open = false;
            }
        });
    if !win_open {
        *open = false;
    }
}
