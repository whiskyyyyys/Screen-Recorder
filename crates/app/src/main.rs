#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use obs_rust::engine::ObsEngine;
use obs_ui::{ObsDesktopApp, StudioBackend};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

const ICON_PNG_BYTES: &[u8] = include_bytes!("../../../assets/icons/obs-rust.png");

fn load_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG_BYTES).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--self-test") {
        let engine = ObsEngine::initialize(true);
        let res = engine.run_self_test();
        engine.shutdown();
        match res {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("SELF-TEST FAILED: {e}");
                std::process::exit(1);
            }
        }
    }

    let engine = ObsEngine::initialize(false);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OxideStudio — OBS Studio Rebuilt in Rust")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([960.0, 620.0]);

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let backend_arc: Arc<dyn obs_ui::StudioBackend> = engine.clone();
    if let Err(e) = eframe::run_native(
        "OxideStudio",
        native_options,
        Box::new(move |cc| Ok(Box::new(ObsDesktopApp::new(cc, backend_arc)))),
    ) {
        eprintln!("Failed launching OxideStudio UI: {e}");
        engine.shutdown();
        std::process::exit(1);
    }
}
