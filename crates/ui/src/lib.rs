pub mod backend;
pub mod modals;
pub mod theme;

pub use backend::StudioBackend;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_dock::{DockArea, DockState, NodeIndex, Style as DockStyle, TabViewer};
use modals::{
    render_about_modal, render_add_source_modal, render_advanced_audio_modal,
    render_autoconfig_modal, render_filters_modal, render_remux_modal, render_settings_modal,
    render_source_properties_modal, render_transition_config_modal, AddSourceModalState,
    AutoConfigModalState, RemuxModalState, SettingsModalState, SettingsTab,
};
use obs_audio::{cubic_fader_to_mul, mul_to_cubic_fader, MeterReading};
use obs_core::{
    RecordState, ReplayBufferState, SceneId, SceneItemId, SourceId, StreamState, TransitionId,
};
use obs_scenes::{SceneCollection, SceneItem, SourceKind};
use obs_settings::ThemeMode;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DockTab {
    Canvas,
    Multiview,
    Scenes,
    Sources,
    AudioMixer,
    Transitions,
    Controls,
    Stats,
}

impl DockTab {
    fn title(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas Preview",
            Self::Multiview => "Multiview",
            Self::Scenes => "Scenes",
            Self::Sources => "Sources",
            Self::AudioMixer => "Audio Mixer",
            Self::Transitions => "Scene Transitions",
            Self::Controls => "Controls",
            Self::Stats => "Stats",
        }
    }
}

fn default_obs_dock_layout() -> DockState<DockTab> {
    let mut state = DockState::new(vec![DockTab::Canvas, DockTab::Multiview]);
    let surface = state.main_surface_mut();
    let [center, bottom] = surface.split_below(NodeIndex::root(), 0.72, vec![DockTab::Scenes]);
    let [_scenes, sources_node] = surface.split_right(bottom, 0.20, vec![DockTab::Sources]);
    let [_sources, mixer_node] = surface.split_right(sources_node, 0.25, vec![DockTab::AudioMixer]);
    let [_mixer, tr_node] = surface.split_right(mixer_node, 0.50, vec![DockTab::Transitions]);
    let [_tr, _ctrl] = surface.split_right(tr_node, 0.45, vec![DockTab::Controls, DockTab::Stats]);
    let _ = center;
    state
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    ResizeBottomRight,
    ResizeTopLeft,
    ResizeTopRight,
    ResizeBottomLeft,
    Rotate,
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
}

struct CanvasDragState {
    item_id: SceneItemId,
    mode: DragMode,
    start_mouse: Pos2,
    orig_pos: [f32; 2],
    orig_scale: [f32; 2],
    orig_rot: f32,
    orig_crop: obs_scenes::CropRect,
}

pub struct ObsDesktopApp {
    backend: Arc<dyn StudioBackend>,
    dock_state: DockState<DockTab>,
    render_state: Option<egui_wgpu::RenderState>,

    prog_tex_id: Option<egui::TextureId>,
    prog_tex_size: (u32, u32),
    prev_tex_id: Option<egui::TextureId>,
    prev_tex_size: (u32, u32),
    mv_tex_id: Option<egui::TextureId>,

    selected_items: Vec<SceneItemId>,
    drag_state: Option<CanvasDragState>,
    multiview_popout_open: bool,

    settings_modal: SettingsModalState,
    add_source_modal: AddSourceModalState,
    props_modal_source: Option<SourceId>,
    filters_modal_source: Option<SourceId>,
    adv_audio_open: bool,
    transition_cfg_modal: Option<TransitionId>,
    remux_modal: RemuxModalState,
    autoconfig_modal: AutoConfigModalState,
    about_open: bool,

    inline_prompt: Option<InlinePromptKind>,
    inline_prompt_text: String,
    status_toast: Option<String>,
    last_theme: Option<(ThemeMode, f32)>,
}

enum InlinePromptKind {
    CreateScene,
    RenameScene(SceneId),
    CreateProfile,
    RenameProfile,
    DuplicateProfile,
    CreateCollection,
}

impl ObsDesktopApp {
    pub fn new(cc: &eframe::CreationContext<'_>, backend: Arc<dyn StudioBackend>) -> Self {
        let render_state = cc.wgpu_render_state.clone();
        if let Some(rs) = &render_state {
            let info = rs.adapter.get_info();
            backend.attach_wgpu(
                Arc::new(rs.device.clone()),
                Arc::new(rs.queue.clone()),
                info.name,
                format!("{:?}", info.backend),
            );
        }

        let root_state = backend.root_state();
        let dock_state = root_state
            .dock_layout_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<DockState<DockTab>>(j).ok())
            .unwrap_or_else(default_obs_dock_layout);

        let settings = backend.settings();
        let mode = if settings.accessibility.high_contrast {
            ThemeMode::HighContrast
        } else {
            settings.general.theme
        };
        theme::apply_theme(&cc.egui_ctx, mode, settings.accessibility.ui_scale);

        Self {
            backend,
            dock_state,
            render_state,
            prog_tex_id: None,
            prog_tex_size: (0, 0),
            prev_tex_id: None,
            prev_tex_size: (0, 0),
            mv_tex_id: None,
            selected_items: Vec::new(),
            drag_state: None,
            multiview_popout_open: false,
            settings_modal: SettingsModalState::default(),
            add_source_modal: AddSourceModalState::default(),
            props_modal_source: None,
            filters_modal_source: None,
            adv_audio_open: false,
            transition_cfg_modal: None,
            remux_modal: RemuxModalState::default(),
            autoconfig_modal: AutoConfigModalState::default(),
            about_open: false,
            inline_prompt: None,
            inline_prompt_text: String::new(),
            status_toast: None,
            last_theme: Some((mode, settings.accessibility.ui_scale)),
        }
    }

    fn sync_gpu_textures(&mut self) {
        let Some(rs) = &self.render_state else {
            return;
        };
        let canvas_size = self.backend.canvas_size();
        if let Some(pv) = self.backend.program_texture_view() {
            let mut renderer = rs.renderer.write();
            if let Some(id) = self.prog_tex_id {
                if self.prog_tex_size != canvas_size {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &rs.device,
                        &pv,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    self.prog_tex_size = canvas_size;
                }
            } else {
                let id =
                    renderer.register_native_texture(&rs.device, &pv, wgpu::FilterMode::Linear);
                self.prog_tex_id = Some(id);
                self.prog_tex_size = canvas_size;
            }
        }
        if let Some(prv) = self.backend.preview_texture_view() {
            let mut renderer = rs.renderer.write();
            if let Some(id) = self.prev_tex_id {
                if self.prev_tex_size != canvas_size {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &rs.device,
                        &prv,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                    self.prev_tex_size = canvas_size;
                }
            } else {
                let id =
                    renderer.register_native_texture(&rs.device, &prv, wgpu::FilterMode::Linear);
                self.prev_tex_id = Some(id);
                self.prev_tex_size = canvas_size;
            }
        }
        if self.mv_tex_id.is_none() {
            if let Some(mv) = self.backend.multiview_texture_view() {
                let id = rs.renderer.write().register_native_texture(
                    &rs.device,
                    &mv,
                    wgpu::FilterMode::Linear,
                );
                self.mv_tex_id = Some(id);
            }
        }
    }

    fn is_multiview_visible(&self) -> bool {
        if self.multiview_popout_open {
            return true;
        }
        for (_surf, node) in self.dock_state.iter_all_nodes() {
            if let Some(tabs) = node.tabs() {
                if tabs.contains(&DockTab::Multiview) {
                    // Check if Multiview is active tab in its node
                    if let egui_dock::Node::Leaf {
                        tabs: t, active, ..
                    } = node
                    {
                        if t.get(active.0) == Some(&DockTab::Multiview) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn render_top_menu(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("obs_menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Show Recordings").clicked() {
                        let path = self.backend.settings().output.rec_path;
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                        ui.close_menu();
                    }
                    if ui.button("Remux Recordings...").clicked() {
                        self.remux_modal.open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Settings").clicked() {
                        self.settings_modal.draft = self.backend.settings();
                        self.settings_modal.open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        ui.close_menu();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if ui.button("Advanced Audio Properties").clicked() {
                        self.adv_audio_open = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Fit Selected Source to Screen").clicked() {
                        self.transform_selected(TransformOp::Fit);
                        ui.close_menu();
                    }
                    if ui.button("Stretch Selected Source to Screen").clicked() {
                        self.transform_selected(TransformOp::Stretch);
                        ui.close_menu();
                    }
                    if ui.button("Reset Selected Transform").clicked() {
                        self.transform_selected(TransformOp::Reset);
                        ui.close_menu();
                    }
                });

                ui.menu_button("View", |ui| {
                    let mut studio = self.backend.studio_mode();
                    if ui.checkbox(&mut studio, "Studio Mode").changed() {
                        self.backend.set_studio_mode(studio);
                    }
                    if ui.button("Open Multiview (Window)").clicked() {
                        self.multiview_popout_open = !self.multiview_popout_open;
                        ui.close_menu();
                    }
                    if ui.button("Reset UI Layout").clicked() {
                        self.dock_state = default_obs_dock_layout();
                        self.backend.update_dock_layout(None);
                        ui.close_menu();
                    }
                    if ui.button("Save Current UI Layout").clicked() {
                        if let Ok(j) = serde_json::to_string(&self.dock_state) {
                            self.backend.update_dock_layout(Some(j));
                            self.status_toast = Some("Workspace dock layout saved.".to_string());
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Stats").clicked() {
                        if self.dock_state.find_tab(&DockTab::Stats).is_none() {
                            self.dock_state.push_to_first_leaf(DockTab::Stats);
                        }
                        ui.close_menu();
                    }
                });

                ui.menu_button("Profile", |ui| {
                    let active = self.backend.active_profile();
                    for p in self.backend.list_profiles() {
                        if ui.selectable_label(p == active, &p).clicked() {
                            let _ = self.backend.switch_profile(&p);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("New...").clicked() {
                        self.inline_prompt = Some(InlinePromptKind::CreateProfile);
                        self.inline_prompt_text.clear();
                        ui.close_menu();
                    }
                    if ui.button("Duplicate...").clicked() {
                        self.inline_prompt = Some(InlinePromptKind::DuplicateProfile);
                        self.inline_prompt_text = format!("{active} Copy");
                        ui.close_menu();
                    }
                    if ui.button("Rename...").clicked() {
                        self.inline_prompt = Some(InlinePromptKind::RenameProfile);
                        self.inline_prompt_text = active.clone();
                        ui.close_menu();
                    }
                    if ui.button("Delete Current").clicked() {
                        if let Err(e) = self.backend.delete_profile(&active) {
                            self.status_toast = Some(e);
                        }
                        ui.close_menu();
                    }
                });

                ui.menu_button("Scene Collection", |ui| {
                    let active = self.backend.active_collection_name();
                    for c in self.backend.list_collections() {
                        if ui.selectable_label(c == active, &c).clicked() {
                            let _ = self.backend.switch_collection(&c);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("New...").clicked() {
                        self.inline_prompt = Some(InlinePromptKind::CreateCollection);
                        self.inline_prompt_text.clear();
                        ui.close_menu();
                    }
                    if ui.button("Import JSON...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                        {
                            if let Err(e) = self.backend.import_collection(&p) {
                                self.status_toast = Some(e);
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Export JSON...").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_file_name(format!("{active}.json"))
                            .add_filter("JSON", &["json"])
                            .save_file()
                        {
                            if let Err(e) = self.backend.export_collection(&p) {
                                self.status_toast = Some(e);
                            } else {
                                self.status_toast = Some(format!("Exported to {}", p.display()));
                            }
                        }
                        ui.close_menu();
                    }
                });

                ui.menu_button("Tools", |ui| {
                    if ui.button("Auto-Configuration Wizard...").clicked() {
                        self.autoconfig_modal.open = true;
                        ui.close_menu();
                    }
                    if ui.button("WebSocket Server Settings...").clicked() {
                        self.settings_modal.draft = self.backend.settings();
                        self.settings_modal.tab = SettingsTab::Advanced;
                        self.settings_modal.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Plugin Manager...").clicked() {
                        self.settings_modal.draft = self.backend.settings();
                        self.settings_modal.tab = SettingsTab::Plugins;
                        self.settings_modal.open = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("About OxideStudio & Encoders").clicked() {
                        self.about_open = true;
                        ui.close_menu();
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(toast) = &self.status_toast {
                        ui.colored_label(Color32::from_rgb(130, 195, 255), toast);
                    }
                });
            });
        });
    }

    fn render_status_bar(&self, ctx: &egui::Context) {
        let stats = self.backend.stats();
        egui::TopBottomPanel::bottom("obs_status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // LIVE status
                let (s_col, s_txt) = match stats.streaming.state {
                    StreamState::Stopped => (Color32::GRAY, "LIVE: Off".to_string()),
                    StreamState::Connecting => (Color32::YELLOW, "LIVE: Connecting...".to_string()),
                    StreamState::Reconnecting => (Color32::YELLOW, format!("LIVE: Reconnecting (#{})", stats.streaming.reconnect_attempts)),
                    StreamState::Streaming => (
                        Color32::from_rgb(76, 217, 100),
                        format!("LIVE: {} | {:.0} kb/s", fmt_ms(stats.streaming.duration_ms), stats.streaming.bitrate_kbps),
                    ),
                    StreamState::Stopping => (Color32::YELLOW, "LIVE: Stopping...".to_string()),
                };
                ui.colored_label(s_col, s_txt);
                ui.separator();

                // REC status
                let (r_col, r_txt) = match stats.recording.state {
                    RecordState::Stopped => (Color32::GRAY, "REC: Off".to_string()),
                    RecordState::Starting => (Color32::YELLOW, "REC: Starting...".to_string()),
                    RecordState::Recording => (
                        Color32::from_rgb(240, 82, 82),
                        format!("REC: {} | {:.1} MB", fmt_ms(stats.recording.duration_ms), stats.recording.bytes_written as f64 / 1_048_576.0),
                    ),
                    RecordState::Paused => (
                        Color32::from_rgb(245, 190, 50),
                        format!("REC PAUSED: {}", fmt_ms(stats.recording.duration_ms)),
                    ),
                    RecordState::Stopping => (Color32::YELLOW, "REC: Finalizing...".to_string()),
                };
                ui.colored_label(r_col, r_txt);
                ui.separator();

                // REPLAY status
                if stats.replay.state != ReplayBufferState::Stopped {
                    ui.colored_label(
                        Color32::from_rgb(100, 190, 255),
                        format!("REPLAY: {:.1}s ({:.1} MB)", stats.replay.buffered_seconds, stats.replay.buffered_bytes as f64 / 1_048_576.0),
                    );
                    ui.separator();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!(
                        "{:.1} / {:.0} fps  |  Render: {:.2} ms  |  Dropped: {} ({:.1}%)  |  WS Clients: {}",
                        stats.active_fps,
                        stats.target_fps,
                        stats.render_time_ms,
                        stats.streaming.dropped_frames,
                        stats.streaming.dropped_percent,
                        stats.websocket_clients,
                    ));
                });
            });
        });
    }

    fn transform_selected(&mut self, op: TransformOp) {
        let Some(&item_id) = self.selected_items.first() else {
            return;
        };
        let (cw, ch) = self.backend.canvas_size();
        let canvas = [cw as f32, ch as f32];
        let col = self.backend.collection();
        let Some(item) = col.find_item(col.active_scene_id, item_id) else {
            return;
        };
        let src_size = self
            .backend
            .source_dimensions(item.source_id)
            .map(|(w, h)| [w as f32, h as f32])
            .unwrap_or(canvas);

        self.backend.with_collection_mut(&mut |c| {
            let sid = c.active_scene_id;
            if let Some(it) = c.find_item_mut(sid, item_id) {
                match op {
                    TransformOp::Fit => {
                        SceneCollection::fit_item_to_canvas(&mut it.transform, src_size, canvas)
                    }
                    TransformOp::Stretch => {
                        SceneCollection::stretch_item_to_canvas(&mut it.transform, src_size, canvas)
                    }
                    TransformOp::Center => {
                        let w = src_size[0] * it.transform.scale[0];
                        let h = src_size[1] * it.transform.scale[1];
                        it.transform.pos = [(canvas[0] - w) * 0.5, (canvas[1] - h) * 0.5];
                    }
                    TransformOp::Rotate90 => {
                        it.transform.rotation_deg = (it.transform.rotation_deg + 90.0) % 360.0;
                    }
                    TransformOp::Reset => {
                        it.transform = Default::default();
                    }
                }
            }
        });
    }

    fn render_inline_prompt(&mut self, ctx: &egui::Context) {
        let Some(kind) = &self.inline_prompt else {
            return;
        };
        let title = match kind {
            InlinePromptKind::CreateScene => "Add Scene",
            InlinePromptKind::RenameScene(_) => "Rename Scene",
            InlinePromptKind::CreateProfile => "New Profile",
            InlinePromptKind::RenameProfile => "Rename Profile",
            InlinePromptKind::DuplicateProfile => "Duplicate Profile",
            InlinePromptKind::CreateCollection => "New Scene Collection",
        };
        let mut keep_open = true;
        let mut submit = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut keep_open)
            .show(ctx, |ui| {
                ui.label("Name:");
                let resp = ui.text_edit_singleline(&mut self.inline_prompt_text);
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.inline_prompt = None;
                    }
                });
            });

        if submit {
            let txt = self.inline_prompt_text.trim().to_string();
            if let Some(k) = self.inline_prompt.take() {
                let res = match k {
                    InlinePromptKind::CreateScene => self.backend.create_scene(&txt).map(|_| ()),
                    InlinePromptKind::RenameScene(id) => self.backend.rename_scene(id, &txt),
                    InlinePromptKind::CreateProfile => self.backend.create_profile(&txt),
                    InlinePromptKind::RenameProfile => self.backend.rename_profile(&txt),
                    InlinePromptKind::DuplicateProfile => self.backend.duplicate_profile(&txt),
                    InlinePromptKind::CreateCollection => self.backend.create_collection(&txt),
                };
                if let Err(e) = res {
                    self.status_toast = Some(e);
                }
            }
        } else if !keep_open {
            self.inline_prompt = None;
        }
    }
}

#[derive(Clone, Copy)]
enum TransformOp {
    Fit,
    Stretch,
    Center,
    Rotate90,
    Reset,
}

impl eframe::App for ObsDesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Re-apply theme only if changed
        let settings = self.backend.settings();
        let desired_theme = if settings.accessibility.high_contrast {
            ThemeMode::HighContrast
        } else {
            settings.general.theme
        };
        if self.last_theme != Some((desired_theme, settings.accessibility.ui_scale)) {
            theme::apply_theme(ctx, desired_theme, settings.accessibility.ui_scale);
            self.last_theme = Some((desired_theme, settings.accessibility.ui_scale));
        }

        let need_mv = self.is_multiview_visible();
        self.backend.tick_frame(need_mv);
        self.sync_gpu_textures();

        if let Some(notice) = self.backend.recent_notices().into_iter().last() {
            self.status_toast = Some(notice);
        }

        self.render_top_menu(ctx);
        self.render_status_bar(ctx);

        let dock_style = DockStyle::from_egui(ctx.style().as_ref());
        let mut dock_state = std::mem::replace(&mut self.dock_state, DockState::new(vec![]));
        {
            let mut viewer = ObsTabViewer { app: self };
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    DockArea::new(&mut dock_state)
                        .style(dock_style)
                        .show_inside(ui, &mut viewer);
                });
        }
        self.dock_state = dock_state;

        // Multi-monitor pop-out Multiview window
        if self.multiview_popout_open {
            let mut mv_open = true;
            let mv_tex = self.mv_tex_id;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("oxidestudio_multiview_window"),
                egui::ViewportBuilder::default()
                    .with_title("OxideStudio — Multiview")
                    .with_inner_size([1120.0, 640.0]),
                |ctx, _class| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        mv_open = false;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if let Some(tid) = mv_tex {
                            let avail = ui.available_size();
                            let w = avail.x.min(avail.y * (16.0 / 9.0));
                            let h = w * (9.0 / 16.0);
                            ui.centered_and_justified(|ui| {
                                ui.image(egui::load::SizedTexture::new(tid, Vec2::new(w, h)));
                            });
                        } else {
                            ui.centered_and_justified(|ui| {
                                ui.label("Initializing Multiview GPU output...")
                            });
                        }
                    });
                },
            );
            if !mv_open {
                self.multiview_popout_open = false;
            }
        }

        // Modals
        let backend_ref: &dyn StudioBackend = &*self.backend;
        render_settings_modal(
            ctx,
            &mut self.settings_modal,
            backend_ref,
            &mut self.status_toast,
        );
        render_add_source_modal(
            ctx,
            &mut self.add_source_modal,
            backend_ref,
            &mut self.status_toast,
            &mut self.props_modal_source,
        );
        render_source_properties_modal(ctx, &mut self.props_modal_source, backend_ref);
        render_filters_modal(ctx, &mut self.filters_modal_source, backend_ref);
        render_advanced_audio_modal(ctx, &mut self.adv_audio_open, backend_ref);
        render_transition_config_modal(ctx, &mut self.transition_cfg_modal, backend_ref);
        render_remux_modal(ctx, &mut self.remux_modal, backend_ref);
        render_autoconfig_modal(
            ctx,
            &mut self.autoconfig_modal,
            backend_ref,
            &mut self.status_toast,
        );
        render_about_modal(ctx, &mut self.about_open, backend_ref);
        self.render_inline_prompt(ctx);

        ctx.request_repaint();
    }

    fn on_exit(&mut self) {
        if let Ok(j) = serde_json::to_string(&self.dock_state) {
            self.backend.update_dock_layout(Some(j));
        }
        self.backend.shutdown();
    }
}

struct ObsTabViewer<'a> {
    app: &'a mut ObsDesktopApp,
}

impl<'a> TabViewer for ObsTabViewer<'a> {
    type Tab = DockTab;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            DockTab::Canvas => self.ui_canvas(ui),
            DockTab::Multiview => self.ui_multiview(ui),
            DockTab::Scenes => self.ui_scenes(ui),
            DockTab::Sources => self.ui_sources(ui),
            DockTab::AudioMixer => self.ui_audio_mixer(ui),
            DockTab::Transitions => self.ui_transitions(ui),
            DockTab::Controls => self.ui_controls(ui),
            DockTab::Stats => self.ui_stats(ui),
        }
    }
}

impl<'a> ObsTabViewer<'a> {
    fn ui_canvas(&mut self, ui: &mut Ui) {
        let studio = self.app.backend.studio_mode();
        let avail = ui.available_size();
        if studio {
            ui.horizontal(|ui| {
                let panel_w = ((avail.x - 95.0) * 0.5).max(160.0);
                let panel_h = (avail.y - 28.0).max(120.0);
                ui.vertical(|ui| {
                    ui.set_width(panel_w);
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(Color32::from_rgb(70, 215, 110), "PREVIEW");
                    });
                    if let Some(prev_id) = self.app.prev_tex_id {
                        self.draw_interactive_canvas(
                            ui,
                            prev_id,
                            Vec2::new(panel_w, panel_h),
                            true,
                        );
                    }
                });

                // Center T-Bar & Transition button
                ui.vertical_centered(|ui| {
                    ui.set_width(80.0);
                    ui.add_space(36.0);
                    if ui.button("Transition").clicked() {
                        self.app.backend.trigger_transition();
                    }
                    ui.add_space(12.0);
                    let mut tbar = self.app.backend.tbar_position();
                    if ui
                        .add(
                            egui::Slider::new(&mut tbar, 0.0..=1.0)
                                .vertical()
                                .show_value(false),
                        )
                        .changed()
                    {
                        self.app.backend.set_tbar_position(tbar);
                    }
                    ui.small("T-Bar");
                });

                ui.vertical(|ui| {
                    ui.set_width(panel_w);
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(Color32::from_rgb(240, 75, 75), "PROGRAM");
                    });
                    if let Some(prog_id) = self.app.prog_tex_id {
                        self.draw_interactive_canvas(
                            ui,
                            prog_id,
                            Vec2::new(panel_w, panel_h),
                            false,
                        );
                    }
                });
            });
        } else if let Some(prog_id) = self.app.prog_tex_id {
            self.draw_interactive_canvas(ui, prog_id, avail, true);
        } else {
            ui.centered_and_justified(|ui| ui.label("Starting GPU Compositor..."));
        }
    }

    fn draw_interactive_canvas(
        &mut self,
        ui: &mut Ui,
        tex_id: egui::TextureId,
        avail: Vec2,
        interactive: bool,
    ) {
        let (cw, ch) = self.app.backend.canvas_size();
        let aspect = (cw as f32) / (ch.max(1) as f32);
        let pad = 12.0;
        let max_w = (avail.x - pad * 2.0).max(64.0);
        let max_h = (avail.y - pad * 2.0).max(64.0);
        let (disp_w, disp_h) = if max_w / max_h > aspect {
            (max_h * aspect, max_h)
        } else {
            (max_w, max_w / aspect)
        };

        let (outer_rect, _) = ui.allocate_exact_size(avail, Sense::hover());
        let canvas_rect = Rect::from_center_size(outer_rect.center(), Vec2::new(disp_w, disp_h));

        ui.painter()
            .rect_filled(outer_rect, 0.0, Color32::from_rgb(14, 15, 19));
        ui.painter().image(
            tex_id,
            canvas_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().rect_stroke(
            canvas_rect,
            0.0,
            Stroke::new(1.0_f32, Color32::from_rgb(60, 66, 82)),
            egui::StrokeKind::Outside,
        );

        if !interactive {
            return;
        }

        let scale_to_ui = disp_w / (cw as f32);
        let col = self.app.backend.collection();
        let target_scene_id = if self.app.backend.studio_mode() {
            col.preview_scene_id.unwrap_or(col.active_scene_id)
        } else {
            col.active_scene_id
        };
        let Some(scene) = col.scene_by_id(target_scene_id) else {
            return;
        };

        let resp = ui.interact(
            canvas_rect,
            ui.id().with(("canvas_hit", tex_id)),
            Sense::click_and_drag(),
        );
        let alt_held = ui.input(|i| i.modifiers.alt);
        let settings = self.app.backend.settings();
        let sel_c = settings.accessibility.selection_color_rgba;
        let sel_color = Color32::from_rgba_unmultiplied(sel_c[0], sel_c[1], sel_c[2], sel_c[3]);
        let crop_c = settings.accessibility.crop_color_rgba;
        let crop_color =
            Color32::from_rgba_unmultiplied(crop_c[0], crop_c[1], crop_c[2], crop_c[3]);

        // Hit-test & draw selection bounding box for selected scene item
        let item_rect_ui = |it: &SceneItem| -> Rect {
            let (sw, sh) = self
                .app
                .backend
                .source_dimensions(it.source_id)
                .unwrap_or((cw, ch));
            let eff_w =
                ((sw as f32) - (it.transform.crop.left + it.transform.crop.right) as f32).max(16.0);
            let eff_h =
                ((sh as f32) - (it.transform.crop.top + it.transform.crop.bottom) as f32).max(16.0);
            let x0 = canvas_rect.min.x + it.transform.pos[0] * scale_to_ui;
            let y0 = canvas_rect.min.y + it.transform.pos[1] * scale_to_ui;
            let w = eff_w * it.transform.scale[0] * scale_to_ui;
            let h = eff_h * it.transform.scale[1] * scale_to_ui;
            Rect::from_min_size(Pos2::new(x0, y0), Vec2::new(w.max(8.0), h.max(8.0)))
        };

        if let Some(&sel_id) = self.app.selected_items.first() {
            if let Some(sel_item) = scene.items.iter().find(|i| i.id == sel_id) {
                let r = item_rect_ui(sel_item);
                let box_col = if alt_held { crop_color } else { sel_color };
                ui.painter().rect_stroke(
                    r,
                    0.0,
                    Stroke::new(2.0_f32, box_col),
                    egui::StrokeKind::Outside,
                );
                let handles = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    Pos2::new(r.center().x, r.top()),
                    Pos2::new(r.center().x, r.bottom()),
                    Pos2::new(r.left(), r.center().y),
                    Pos2::new(r.right(), r.center().y),
                ];
                for hp in handles {
                    ui.painter().rect_filled(
                        Rect::from_center_size(hp, Vec2::splat(8.0)),
                        1.0,
                        box_col,
                    );
                }
                // Rotation handle
                let rot_pos = Pos2::new(r.center().x, r.top() - 18.0);
                ui.painter().line_segment(
                    [Pos2::new(r.center().x, r.top()), rot_pos],
                    Stroke::new(1.5_f32, box_col),
                );
                ui.painter().circle_filled(rot_pos, 5.0, box_col);
            }
        }

        if resp.drag_started() {
            if let Some(mpos) = resp.interact_pointer_pos() {
                let mut started_drag = None;
                if let Some(&sel_id) = self.app.selected_items.first() {
                    if let Some(sel_item) = scene.items.iter().find(|i| i.id == sel_id && !i.locked)
                    {
                        let r = item_rect_ui(sel_item);
                        let rot_pos = Pos2::new(r.center().x, r.top() - 18.0);
                        let mode = if mpos.distance(rot_pos) < 10.0 {
                            Some(DragMode::Rotate)
                        } else if mpos.distance(r.right_bottom()) < 12.0 {
                            Some(DragMode::ResizeBottomRight)
                        } else if mpos.distance(r.left_top()) < 12.0 {
                            Some(DragMode::ResizeTopLeft)
                        } else if mpos.distance(r.right_top()) < 12.0 {
                            Some(DragMode::ResizeTopRight)
                        } else if mpos.distance(r.left_bottom()) < 12.0 {
                            Some(DragMode::ResizeBottomLeft)
                        } else if alt_held && (mpos.x - r.left()).abs() < 10.0 {
                            Some(DragMode::CropLeft)
                        } else if alt_held && (mpos.x - r.right()).abs() < 10.0 {
                            Some(DragMode::CropRight)
                        } else if alt_held && (mpos.y - r.top()).abs() < 10.0 {
                            Some(DragMode::CropTop)
                        } else if alt_held && (mpos.y - r.bottom()).abs() < 10.0 {
                            Some(DragMode::CropBottom)
                        } else if r.contains(mpos) {
                            Some(DragMode::Move)
                        } else {
                            None
                        };
                        if let Some(dm) = mode {
                            started_drag = Some(CanvasDragState {
                                item_id: sel_item.id,
                                mode: dm,
                                start_mouse: mpos,
                                orig_pos: sel_item.transform.pos,
                                orig_scale: sel_item.transform.scale,
                                orig_rot: sel_item.transform.rotation_deg,
                                orig_crop: sel_item.transform.crop,
                            });
                        }
                    }
                }
                if started_drag.is_none() {
                    // Select topmost visible item under cursor
                    for item in scene.items.iter().rev() {
                        if item.visible && item_rect_ui(item).contains(mpos) {
                            self.app.selected_items = vec![item.id];
                            if !item.locked {
                                started_drag = Some(CanvasDragState {
                                    item_id: item.id,
                                    mode: DragMode::Move,
                                    start_mouse: mpos,
                                    orig_pos: item.transform.pos,
                                    orig_scale: item.transform.scale,
                                    orig_rot: item.transform.rotation_deg,
                                    orig_crop: item.transform.crop,
                                });
                            }
                            break;
                        }
                    }
                }
                self.app.drag_state = started_drag;
            }
        } else if resp.clicked() {
            if let Some(mpos) = resp.interact_pointer_pos() {
                self.app.selected_items.clear();
                for item in scene.items.iter().rev() {
                    if item.visible && item_rect_ui(item).contains(mpos) {
                        self.app.selected_items.push(item.id);
                        break;
                    }
                }
            }
        }

        if resp.dragged() {
            if let (Some(ds), Some(mpos)) = (&self.app.drag_state, resp.interact_pointer_pos()) {
                let delta_canvas = (mpos - ds.start_mouse) / scale_to_ui.max(0.01);
                let item_id = ds.item_id;
                let mode = ds.mode;
                let orig_pos = ds.orig_pos;
                let orig_scale = ds.orig_scale;
                let orig_rot = ds.orig_rot;
                let orig_crop = ds.orig_crop;
                let snap_on = settings.general.snap_enabled;
                let snap_dist = settings.general.snap_distance_px;

                let (sw, sh) = scene
                    .items
                    .iter()
                    .find(|i| i.id == item_id)
                    .and_then(|i| self.app.backend.source_dimensions(i.source_id))
                    .unwrap_or((cw, ch));

                self.app.backend.with_collection_mut(&mut |c| {
                    if let Some(it) = c.find_item_mut(target_scene_id, item_id) {
                        match mode {
                            DragMode::Move => {
                                let mut nx = orig_pos[0] + delta_canvas.x;
                                let mut ny = orig_pos[1] + delta_canvas.y;
                                if snap_on {
                                    if nx.abs() < snap_dist {
                                        nx = 0.0;
                                    }
                                    if ny.abs() < snap_dist {
                                        ny = 0.0;
                                    }
                                    let dw = (sw as f32) * orig_scale[0];
                                    let dh = (sh as f32) * orig_scale[1];
                                    if ((nx + dw) - cw as f32).abs() < snap_dist {
                                        nx = cw as f32 - dw;
                                    }
                                    if ((ny + dh) - ch as f32).abs() < snap_dist {
                                        ny = ch as f32 - dh;
                                    }
                                }
                                it.transform.pos = [nx, ny];
                            }
                            DragMode::ResizeBottomRight => {
                                let base_w = (sw as f32 * orig_scale[0]).max(16.0);
                                let base_h = (sh as f32 * orig_scale[1]).max(16.0);
                                let factor = ((base_w + delta_canvas.x) / base_w)
                                    .max((base_h + delta_canvas.y) / base_h)
                                    .max(0.05);
                                it.transform.scale =
                                    [orig_scale[0] * factor, orig_scale[1] * factor];
                            }
                            DragMode::ResizeTopLeft => {
                                let base_w = (sw as f32 * orig_scale[0]).max(16.0);
                                let factor = ((base_w - delta_canvas.x) / base_w).max(0.05);
                                it.transform.scale =
                                    [orig_scale[0] * factor, orig_scale[1] * factor];
                                it.transform.pos = [
                                    orig_pos[0] + base_w * (1.0 - factor),
                                    orig_pos[1] + (sh as f32 * orig_scale[1]) * (1.0 - factor),
                                ];
                            }
                            DragMode::ResizeTopRight | DragMode::ResizeBottomLeft => {
                                let base_w = (sw as f32 * orig_scale[0]).max(16.0);
                                let factor = ((base_w + delta_canvas.x) / base_w).max(0.05);
                                it.transform.scale =
                                    [orig_scale[0] * factor, orig_scale[1] * factor];
                            }
                            DragMode::Rotate => {
                                it.transform.rotation_deg =
                                    (orig_rot + delta_canvas.x * 0.4) % 360.0;
                            }
                            DragMode::CropLeft => {
                                let add = delta_canvas.x.max(-(orig_crop.left as f32)) as i32;
                                it.transform.crop.left =
                                    (orig_crop.left as i32 + add).max(0) as u32;
                            }
                            DragMode::CropRight => {
                                let add = (-delta_canvas.x).max(-(orig_crop.right as f32)) as i32;
                                it.transform.crop.right =
                                    (orig_crop.right as i32 + add).max(0) as u32;
                            }
                            DragMode::CropTop => {
                                let add = delta_canvas.y.max(-(orig_crop.top as f32)) as i32;
                                it.transform.crop.top = (orig_crop.top as i32 + add).max(0) as u32;
                            }
                            DragMode::CropBottom => {
                                let add = (-delta_canvas.y).max(-(orig_crop.bottom as f32)) as i32;
                                it.transform.crop.bottom =
                                    (orig_crop.bottom as i32 + add).max(0) as u32;
                            }
                        }
                    }
                });
            }
        } else if resp.drag_stopped() {
            self.app.drag_state = None;
        }

        resp.context_menu(|ui| {
            if ui.button("Fit to screen").clicked() {
                self.app.transform_selected(TransformOp::Fit);
                ui.close_menu();
            }
            if ui.button("Stretch to screen").clicked() {
                self.app.transform_selected(TransformOp::Stretch);
                ui.close_menu();
            }
            if ui.button("Center to screen").clicked() {
                self.app.transform_selected(TransformOp::Center);
                ui.close_menu();
            }
            if ui.button("Rotate 90° CW").clicked() {
                self.app.transform_selected(TransformOp::Rotate90);
                ui.close_menu();
            }
            if ui.button("Reset Transform").clicked() {
                self.app.transform_selected(TransformOp::Reset);
                ui.close_menu();
            }
            ui.separator();
            if let Some(&sel_id) = self.app.selected_items.first() {
                if let Some(it) = scene.items.iter().find(|i| i.id == sel_id) {
                    let sid = it.source_id;
                    if ui.button("Properties").clicked() {
                        self.app.props_modal_source = Some(sid);
                        ui.close_menu();
                    }
                    if ui.button("Filters").clicked() {
                        self.app.filters_modal_source = Some(sid);
                        ui.close_menu();
                    }
                }
            }
        });
    }

    fn ui_multiview(&mut self, ui: &mut Ui) {
        if let Some(tid) = self.app.mv_tex_id {
            let avail = ui.available_size();
            let w = avail.x.min(avail.y * (16.0 / 9.0)).max(64.0);
            let h = w * (9.0 / 16.0);
            ui.centered_and_justified(|ui| {
                let resp = ui.image(egui::load::SizedTexture::new(tid, Vec2::new(w, h)));
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        // Click bottom 2x4 scene grid to switch directly!
                        let rel = (pos - resp.rect.min) / resp.rect.size();
                        if rel.y >= 0.5 {
                            let col_idx = (rel.x * 4.0).floor().clamp(0.0, 3.0) as usize;
                            let row_idx = ((rel.y - 0.5) * 4.0).floor().clamp(0.0, 1.0) as usize;
                            let idx = row_idx * 4 + col_idx;
                            let col = self.app.backend.collection();
                            if let Some(sc) = col.scenes.get(idx) {
                                if self.app.backend.studio_mode() {
                                    self.app.backend.set_preview_scene(sc.id);
                                } else {
                                    self.app.backend.switch_scene(sc.id);
                                }
                            }
                        }
                    }
                }
            });
        } else {
            ui.centered_and_justified(|ui| ui.label("Multiview initializing..."));
        }
    }

    fn ui_scenes(&mut self, ui: &mut Ui) {
        let col = self.app.backend.collection();
        let studio = self.app.backend.studio_mode();
        let selected_id = if studio {
            col.preview_scene_id.unwrap_or(col.active_scene_id)
        } else {
            col.active_scene_id
        };

        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 32.0)
            .show(ui, |ui| {
                for (idx, sc) in col.scenes.iter().enumerate() {
                    let is_sel = sc.id == selected_id;
                    let is_prog = sc.id == col.active_scene_id;
                    let label = if studio && is_prog {
                        format!("● {} [LIVE]", sc.name)
                    } else {
                        sc.name.clone()
                    };
                    let resp = ui.selectable_label(is_sel, label);
                    if resp.clicked() {
                        if studio {
                            self.app.backend.set_preview_scene(sc.id);
                        } else {
                            self.app.backend.switch_scene(sc.id);
                        }
                        self.app.selected_items.clear();
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Rename").clicked() {
                            self.app.inline_prompt = Some(InlinePromptKind::RenameScene(sc.id));
                            self.app.inline_prompt_text = sc.name.clone();
                            ui.close_menu();
                        }
                        if ui.button("Duplicate").clicked() {
                            let _ = self.app.backend.duplicate_scene(sc.id);
                            ui.close_menu();
                        }
                        if idx > 0 && ui.button("Move Up").clicked() {
                            self.app.backend.move_scene(idx, idx - 1);
                            ui.close_menu();
                        }
                        if idx + 1 < col.scenes.len() && ui.button("Move Down").clicked() {
                            self.app.backend.move_scene(idx, idx + 1);
                            ui.close_menu();
                        }
                        if col.scenes.len() > 1 && ui.button("Remove").clicked() {
                            let _ = self.app.backend.delete_scene(sc.id);
                            ui.close_menu();
                        }
                    });
                }
            });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("+").on_hover_text("Add Scene").clicked() {
                self.app.inline_prompt = Some(InlinePromptKind::CreateScene);
                self.app.inline_prompt_text = format!("Scene {}", col.scenes.len() + 1);
            }
            if ui.button("-").on_hover_text("Remove Scene").clicked() {
                if let Err(e) = self.app.backend.delete_scene(selected_id) {
                    self.app.status_toast = Some(e);
                }
            }
            if ui.button("Dup").on_hover_text("Duplicate Scene").clicked() {
                let _ = self.app.backend.duplicate_scene(selected_id);
            }
            if let Some(idx) = col.scenes.iter().position(|s| s.id == selected_id) {
                if idx > 0 && ui.button("↑").clicked() {
                    self.app.backend.move_scene(idx, idx - 1);
                }
                if idx + 1 < col.scenes.len() && ui.button("↓").clicked() {
                    self.app.backend.move_scene(idx, idx + 1);
                }
            }
        });
    }

    fn ui_sources(&mut self, ui: &mut Ui) {
        let col = self.app.backend.collection();
        let target_scene_id = if self.app.backend.studio_mode() {
            col.preview_scene_id.unwrap_or(col.active_scene_id)
        } else {
            col.active_scene_id
        };
        let errors = self.app.backend.source_errors();

        let mut toggle_vis: Option<SceneItemId> = None;
        let mut toggle_lock: Option<SceneItemId> = None;
        let mut open_props: Option<SourceId> = None;
        let mut open_filters: Option<SourceId> = None;
        let mut ungroup_id: Option<SceneItemId> = None;
        let ctrl_held = ui.input(|i| i.modifiers.ctrl);

        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 32.0)
            .show(ui, |ui| {
                if let Some(scene) = col.scene_by_id(target_scene_id) {
                    // Draw topmost layer first (OBS convention: reverse of render order)
                    for item in scene.items.iter().rev() {
                        self.draw_source_row(
                            ui,
                            &col,
                            item,
                            0,
                            &errors,
                            ctrl_held,
                            &mut toggle_vis,
                            &mut toggle_lock,
                            &mut open_props,
                            &mut open_filters,
                            &mut ungroup_id,
                        );
                    }
                }
            });

        if let Some(id) = toggle_vis {
            self.app.backend.with_collection_mut(&mut |c| {
                if let Some(it) = c.find_item_mut(target_scene_id, id) {
                    it.visible = !it.visible;
                }
            });
        }
        if let Some(id) = toggle_lock {
            self.app.backend.with_collection_mut(&mut |c| {
                if let Some(it) = c.find_item_mut(target_scene_id, id) {
                    it.locked = !it.locked;
                }
            });
        }
        if let Some(sid) = open_props {
            self.app.props_modal_source = Some(sid);
        }
        if let Some(sid) = open_filters {
            self.app.filters_modal_source = Some(sid);
        }
        if let Some(gid) = ungroup_id {
            let _ = self.app.backend.ungroup_item(target_scene_id, gid);
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.menu_button("+", |ui| {
                for &kind in SourceKind::creatable_source_kinds() {
                    if ui.button(kind.display_name()).clicked() {
                        self.app.add_source_modal = AddSourceModalState {
                            open: true,
                            kind: Some(kind),
                            create_new: true,
                            new_name: kind.display_name().to_string(),
                            selected_existing: None,
                        };
                        ui.close_menu();
                    }
                }
            });
            if ui.button("-").on_hover_text("Remove Source").clicked() {
                for id in self.app.selected_items.drain(..) {
                    let _ = self.app.backend.remove_scene_item(target_scene_id, id);
                }
            }
            if ui
                .button("Props")
                .on_hover_text("Source Properties")
                .clicked()
            {
                if let Some(&id) = self.app.selected_items.first() {
                    if let Some(it) = col.find_item(target_scene_id, id) {
                        self.app.props_modal_source = Some(it.source_id);
                    }
                }
            }
            if ui
                .button("Filters")
                .on_hover_text("Source Filters")
                .clicked()
            {
                if let Some(&id) = self.app.selected_items.first() {
                    if let Some(it) = col.find_item(target_scene_id, id) {
                        self.app.filters_modal_source = Some(it.source_id);
                    }
                }
            }
            if !self.app.selected_items.is_empty()
                && ui.button("Grp").on_hover_text("Group Selected").clicked()
            {
                let _ = self.app.backend.group_items(
                    target_scene_id,
                    &self.app.selected_items,
                    "Group",
                );
            }
            if ui.button("↑").on_hover_text("Move Layer Up").clicked() {
                if let Some(&id) = self.app.selected_items.first() {
                    self.app.backend.with_collection_mut(&mut |c| {
                        let _ = c.move_item_step(target_scene_id, id, 1);
                    });
                }
            }
            if ui.button("↓").on_hover_text("Move Layer Down").clicked() {
                if let Some(&id) = self.app.selected_items.first() {
                    self.app.backend.with_collection_mut(&mut |c| {
                        let _ = c.move_item_step(target_scene_id, id, -1);
                    });
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_source_row(
        &mut self,
        ui: &mut Ui,
        col: &SceneCollection,
        item: &SceneItem,
        depth: usize,
        errors: &std::collections::HashMap<SourceId, String>,
        ctrl_held: bool,
        toggle_vis: &mut Option<SceneItemId>,
        toggle_lock: &mut Option<SceneItemId>,
        open_props: &mut Option<SourceId>,
        open_filters: &mut Option<SourceId>,
        ungroup_id: &mut Option<SceneItemId>,
    ) {
        let name = col
            .sources
            .get(&item.source_id)
            .map(|s| s.name.as_str())
            .unwrap_or("Unknown");
        let is_sel = self.app.selected_items.contains(&item.id);
        let has_err = errors.get(&item.source_id);

        ui.horizontal(|ui| {
            if depth > 0 {
                ui.add_space((depth as f32) * 14.0);
            }
            let prefix = if item.is_group() { "[G] " } else { "" };
            let resp = ui.selectable_label(is_sel, format!("{prefix}{name}"));
            if resp.clicked() {
                if ctrl_held {
                    if is_sel {
                        self.app.selected_items.retain(|&x| x != item.id);
                    } else {
                        self.app.selected_items.push(item.id);
                    }
                } else {
                    self.app.selected_items = vec![item.id];
                }
            }
            if resp.double_clicked() {
                *open_props = Some(item.source_id);
            }
            resp.context_menu(|ui| {
                if ui.button("Properties").clicked() {
                    *open_props = Some(item.source_id);
                    ui.close_menu();
                }
                if ui.button("Filters").clicked() {
                    *open_filters = Some(item.source_id);
                    ui.close_menu();
                }
                if item.is_group() && ui.button("Ungroup").clicked() {
                    *ungroup_id = Some(item.id);
                    ui.close_menu();
                }
            });

            if let Some(err) = has_err {
                ui.colored_label(Color32::YELLOW, "!").on_hover_text(err);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let lock_txt = if item.locked { "L" } else { "U" };
                if ui
                    .small_button(lock_txt)
                    .on_hover_text("Toggle Lock")
                    .clicked()
                {
                    *toggle_lock = Some(item.id);
                }
                let vis_txt = if item.visible { "👁" } else { "○" };
                if ui
                    .small_button(vis_txt)
                    .on_hover_text("Toggle Visibility")
                    .clicked()
                {
                    *toggle_vis = Some(item.id);
                }
            });
        });

        if let Some(children) = &item.group_children {
            for child in children.iter().rev() {
                self.draw_source_row(
                    ui,
                    col,
                    child,
                    depth + 1,
                    errors,
                    ctrl_held,
                    toggle_vis,
                    toggle_lock,
                    open_props,
                    open_filters,
                    ungroup_id,
                );
            }
        }
    }

    fn ui_audio_mixer(&mut self, ui: &mut Ui) {
        let col = self.app.backend.collection();
        let meters = self.app.backend.audio_meters();
        let mut updates: Vec<(SourceId, f32, bool)> = Vec::new();
        let mut open_filters_for: Option<SourceId> = None;

        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⚙ Advanced Audio").clicked() {
                    self.app.adv_audio_open = true;
                }
            });
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            for src in col.sources.values().filter(|s| s.kind.has_audio()) {
                let meter = meters.get(&src.id).copied().unwrap_or(MeterReading {
                    peak_l_db: -60.0,
                    peak_r_db: -60.0,
                    rms_l_db: -60.0,
                    rms_r_db: -60.0,
                    hold_l_db: -60.0,
                    hold_r_db: -60.0,
                    clipping: false,
                });

                let mut vol_mul = src.audio.volume_mul;
                let mut muted = src.audio.muted;

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&src.name).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Filters").clicked() {
                                open_filters_for = Some(src.id);
                            }
                            let db = obs_filters::linear_to_db(vol_mul);
                            ui.small(if db <= -60.0 {
                                "-inf dB".to_string()
                            } else {
                                format!("{db:.1} dB")
                            });
                        });
                    });

                    // Live Stereo VU Meter (Green -60..-20, Yellow -20..-9, Red -9..0)
                    draw_vu_meter(ui, meter, muted);

                    ui.horizontal(|ui| {
                        let mut fader_pos = mul_to_cubic_fader(vol_mul);
                        if ui
                            .add(egui::Slider::new(&mut fader_pos, 0.0..=1.2).show_value(false))
                            .changed()
                        {
                            vol_mul = cubic_fader_to_mul(fader_pos);
                        }
                        let mute_lbl = if muted { "Muted" } else { "Mute" };
                        let btn = egui::Button::new(mute_lbl).fill(if muted {
                            Color32::from_rgb(165, 42, 42)
                        } else {
                            Color32::from_rgb(48, 52, 64)
                        });
                        if ui.add(btn).clicked() {
                            muted = !muted;
                        }
                    });
                });

                if (vol_mul - src.audio.volume_mul).abs() > 1e-4 || muted != src.audio.muted {
                    updates.push((src.id, vol_mul, muted));
                }
            }
        });

        if !updates.is_empty() {
            self.app.backend.with_collection_mut(&mut |c| {
                for &(id, vm, m) in &updates {
                    if let Some(s) = c.sources.get_mut(&id) {
                        s.audio.volume_mul = vm;
                        s.audio.muted = m;
                    }
                }
            });
        }
        if let Some(sid) = open_filters_for {
            self.app.filters_modal_source = Some(sid);
        }
    }

    fn ui_transitions(&mut self, ui: &mut Ui) {
        let col = self.app.backend.collection();
        let active_tr = col.active_transition();
        let active_id = active_tr.map(|t| t.id).unwrap_or(col.active_transition_id);
        let active_name = active_tr
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "Fade".to_string());

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("tr_combo")
                .selected_text(&active_name)
                .show_ui(ui, |ui| {
                    for tr in &col.transitions {
                        if ui.selectable_label(tr.id == active_id, &tr.name).clicked() {
                            self.app.backend.set_active_transition(tr.id);
                        }
                    }
                });
            if ui
                .button("⚙")
                .on_hover_text("Configure Transition")
                .clicked()
            {
                self.app.transition_cfg_modal = Some(active_id);
            }
        });

        let mut dur = col.transition_duration_ms;
        ui.horizontal(|ui| {
            ui.label("Duration:");
            if ui
                .add(
                    egui::DragValue::new(&mut dur)
                        .range(0..=5000)
                        .speed(25)
                        .suffix(" ms"),
                )
                .changed()
            {
                self.app.backend.set_transition_duration(dur);
            }
        });
    }

    fn ui_controls(&mut self, ui: &mut Ui) {
        let stats = self.app.backend.stats();
        let streaming = stats.streaming.state != StreamState::Stopped;
        let recording = matches!(
            stats.recording.state,
            RecordState::Recording | RecordState::Paused
        );
        let paused = stats.recording.state == RecordState::Paused;
        let replay_active = stats.replay.state != ReplayBufferState::Stopped;

        let btn_w = ui.available_width().max(120.0);

        let stream_btn = egui::Button::new(if streaming {
            "Stop Streaming"
        } else {
            "Start Streaming"
        })
        .min_size(Vec2::new(btn_w, 28.0))
        .fill(if streaming {
            Color32::from_rgb(38, 125, 220)
        } else {
            Color32::from_rgb(48, 53, 66)
        });
        if ui.add(stream_btn).clicked() {
            let res = if streaming {
                self.app.backend.stop_streaming()
            } else {
                self.app.backend.start_streaming()
            };
            if let Err(e) = res {
                self.app.status_toast = Some(e);
            }
        }

        ui.horizontal(|ui| {
            let rec_w = if recording {
                (btn_w - 42.0).max(80.0)
            } else {
                btn_w
            };
            let rec_btn = egui::Button::new(if recording {
                "Stop Recording"
            } else {
                "Start Recording"
            })
            .min_size(Vec2::new(rec_w, 28.0))
            .fill(if recording {
                Color32::from_rgb(190, 48, 48)
            } else {
                Color32::from_rgb(48, 53, 66)
            });
            if ui.add(rec_btn).clicked() {
                if recording {
                    match self.app.backend.stop_recording() {
                        Ok(p) => {
                            self.app.status_toast = Some(format!("Saved: {}", p.display()));
                        }
                        Err(e) => self.app.status_toast = Some(e),
                    }
                } else if let Err(e) = self.app.backend.start_recording() {
                    self.app.status_toast = Some(e);
                }
            }
            if recording {
                let p_lbl = if paused { "▶" } else { "⏸" };
                if ui
                    .add_sized([36.0, 28.0], egui::Button::new(p_lbl))
                    .on_hover_text(if paused {
                        "Resume Recording"
                    } else {
                        "Pause Recording"
                    })
                    .clicked()
                {
                    if let Err(e) = self.app.backend.pause_or_resume_recording() {
                        self.app.status_toast = Some(e);
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            let rw = if replay_active {
                (btn_w - 42.0).max(80.0)
            } else {
                btn_w
            };
            let rb_btn = egui::Button::new(if replay_active {
                "Stop Replay Buffer"
            } else {
                "Start Replay Buffer"
            })
            .min_size(Vec2::new(rw, 26.0))
            .fill(if replay_active {
                Color32::from_rgb(40, 130, 175)
            } else {
                Color32::from_rgb(48, 53, 66)
            });
            if ui.add(rb_btn).clicked() {
                let res = if replay_active {
                    self.app.backend.stop_replay_buffer()
                } else {
                    self.app.backend.start_replay_buffer()
                };
                if let Err(e) = res {
                    self.app.status_toast = Some(e);
                }
            }
            if replay_active
                && ui
                    .add_sized([36.0, 26.0], egui::Button::new("Save"))
                    .on_hover_text("Save Replay")
                    .clicked()
            {
                match self.app.backend.save_replay_buffer() {
                    Ok(p) => self.app.status_toast = Some(format!("Replay saved: {}", p.display())),
                    Err(e) => self.app.status_toast = Some(e),
                }
            }
        });

        let studio = self.app.backend.studio_mode();
        let st_btn = egui::Button::new("Studio Mode")
            .min_size(Vec2::new(btn_w, 26.0))
            .fill(if studio {
                Color32::from_rgb(46, 107, 230)
            } else {
                Color32::from_rgb(48, 53, 66)
            });
        if ui.add(st_btn).clicked() {
            self.app.backend.set_studio_mode(!studio);
        }

        if ui
            .add_sized([btn_w, 26.0], egui::Button::new("Settings"))
            .clicked()
        {
            self.app.settings_modal.draft = self.app.backend.settings();
            self.app.settings_modal.open = true;
        }
    }

    fn ui_stats(&mut self, ui: &mut Ui) {
        let s = self.app.backend.stats();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("stats_grid")
                .num_columns(2)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    ui.label("FPS:");
                    ui.label(format!("{:.2} / {:.2}", s.active_fps, s.target_fps));
                    ui.end_row();
                    ui.label("GPU Adapter:");
                    ui.label(format!("{} ({})", s.gpu_adapter_name, s.gpu_backend));
                    ui.end_row();
                    ui.label("Avg Render Time:");
                    ui.label(format!("{:.2} ms", s.render_time_ms));
                    ui.end_row();
                    ui.label("Lagged Frames:");
                    ui.label(format!("{} / {}", s.lagged_frames, s.total_rendered_frames));
                    ui.end_row();
                    ui.label("Active Encoder:");
                    ui.label(&s.active_video_encoder);
                    ui.end_row();
                    ui.label("Memory Usage:");
                    ui.label(format!("{:.1} MB", s.memory_usage_mb));
                    ui.end_row();
                    ui.label("Stream Dropped:");
                    ui.label(format!(
                        "{} / {} ({:.1}%)",
                        s.streaming.dropped_frames,
                        s.streaming.total_frames,
                        s.streaming.dropped_percent
                    ));
                    ui.end_row();
                    ui.label("Stream RTT:");
                    ui.label(format!("{:.1} ms", s.streaming.rtt_ms));
                    ui.end_row();
                    ui.label("Recording Size:");
                    ui.label(format!(
                        "{:.2} MB",
                        s.recording.bytes_written as f64 / 1_048_576.0
                    ));
                    ui.end_row();
                    ui.label("WebSocket Clients:");
                    ui.label(format!("{}", s.websocket_clients));
                    ui.end_row();
                });
        });
    }
}

fn draw_vu_meter(ui: &mut Ui, meter: MeterReading, muted: bool) {
    let w = ui.available_width().max(60.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 12.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_rgb(16, 18, 22));

    let draw_bar = |bar_rect: Rect, peak_db: f32, hold_db: f32| {
        let norm = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
        if norm > 0.005 {
            let fill_w = bar_rect.width() * norm;
            let green_end = bar_rect.min.x + bar_rect.width() * (40.0 / 60.0);
            let yellow_end = bar_rect.min.x + bar_rect.width() * (51.0 / 60.0);
            let cur_end = bar_rect.min.x + fill_w;

            let g_col = if muted {
                Color32::from_rgb(75, 80, 90)
            } else {
                Color32::from_rgb(56, 200, 92)
            };
            let y_col = if muted {
                Color32::from_rgb(95, 95, 100)
            } else {
                Color32::from_rgb(235, 196, 48)
            };
            let r_col = if muted {
                Color32::from_rgb(110, 85, 85)
            } else {
                Color32::from_rgb(235, 62, 62)
            };

            let g_r = cur_end.min(green_end);
            if g_r > bar_rect.min.x {
                p.rect_filled(
                    Rect::from_min_max(bar_rect.min, Pos2::new(g_r, bar_rect.max.y)),
                    1.0,
                    g_col,
                );
            }
            if cur_end > green_end {
                let y_r = cur_end.min(yellow_end);
                p.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(green_end, bar_rect.min.y),
                        Pos2::new(y_r, bar_rect.max.y),
                    ),
                    0.0,
                    y_col,
                );
            }
            if cur_end > yellow_end {
                p.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(yellow_end, bar_rect.min.y),
                        Pos2::new(cur_end, bar_rect.max.y),
                    ),
                    0.0,
                    r_col,
                );
            }
        }
        let hold_norm = ((hold_db + 60.0) / 60.0).clamp(0.0, 1.0);
        if hold_norm > 0.01 {
            let hx = bar_rect.min.x + bar_rect.width() * hold_norm;
            p.line_segment(
                [Pos2::new(hx, bar_rect.min.y), Pos2::new(hx, bar_rect.max.y)],
                Stroke::new(1.5_f32, Color32::WHITE),
            );
        }
    };

    let l_rect = Rect::from_min_max(
        rect.min + Vec2::new(1.0, 1.0),
        Pos2::new(rect.max.x - 1.0, rect.min.y + 5.0),
    );
    let r_rect = Rect::from_min_max(
        Pos2::new(rect.min.x + 1.0, rect.min.y + 7.0),
        rect.max - Vec2::new(1.0, 1.0),
    );
    draw_bar(l_rect, meter.peak_l_db, meter.hold_l_db);
    draw_bar(r_rect, meter.peak_r_db, meter.hold_r_db);
}

fn fmt_ms(ms: u64) -> String {
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
