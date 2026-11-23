use egui::{Color32, CornerRadius, Stroke, Style, Visuals};
use obs_settings::ThemeMode;

pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode, ui_scale: f32) {
    ctx.set_pixels_per_point(ui_scale.clamp(0.75, 2.0));

    let mut style = Style::default();
    let mut visuals = match mode {
        ThemeMode::Dark => obs_yami_dark_visuals(),
        ThemeMode::Light => obs_crisp_light_visuals(),
        ThemeMode::HighContrast => obs_high_contrast_visuals(),
    };

    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(6);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
    visuals.widgets.active.corner_radius = CornerRadius::same(4);
    visuals.widgets.open.corner_radius = CornerRadius::same(4);

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(7.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
}

fn obs_yami_dark_visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.window_fill = Color32::from_rgb(29, 32, 39);
    v.panel_fill = Color32::from_rgb(24, 26, 32);
    v.faint_bg_color = Color32::from_rgb(33, 36, 44);
    v.extreme_bg_color = Color32::from_rgb(17, 19, 24);

    v.widgets.noninteractive.bg_fill = Color32::from_rgb(35, 38, 47);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(52, 57, 70));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(220, 224, 232));

    v.widgets.inactive.bg_fill = Color32::from_rgb(46, 51, 63);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(42, 46, 56);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(64, 70, 86));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(230, 234, 240));

    v.widgets.hovered.bg_fill = Color32::from_rgb(58, 65, 80);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(55, 61, 75);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(94, 146, 235));

    v.widgets.active.bg_fill = Color32::from_rgb(46, 107, 230);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(110, 165, 255));
    v.selection.bg_fill = Color32::from_rgb(44, 104, 224);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(130, 180, 255));
    v
}

fn obs_crisp_light_visuals() -> Visuals {
    let mut v = Visuals::light();
    v.window_fill = Color32::from_rgb(245, 246, 249);
    v.panel_fill = Color32::from_rgb(237, 239, 244);
    v.extreme_bg_color = Color32::from_rgb(255, 255, 255);
    v.selection.bg_fill = Color32::from_rgb(38, 100, 220);
    v
}

fn obs_high_contrast_visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.window_fill = Color32::BLACK;
    v.panel_fill = Color32::BLACK;
    v.extreme_bg_color = Color32::BLACK;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.inactive.bg_fill = Color32::from_rgb(20, 20, 20);
    v.widgets.inactive.bg_stroke = Stroke::new(1.5_f32, Color32::YELLOW);
    v.widgets.inactive.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0, 80, 160);
    v.selection.bg_fill = Color32::from_rgb(255, 210, 0);
    v.selection.stroke = Stroke::new(1.5_f32, Color32::BLACK);
    v
}
