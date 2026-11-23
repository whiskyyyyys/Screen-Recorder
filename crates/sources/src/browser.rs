use obs_core::{Clock, SourceId};
use obs_video::VideoFrame;
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn find_edge_browser() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSpec {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub custom_css: String,
}

pub struct BrowserWorker {
    pub spec: BrowserSpec,
    running: Arc<AtomicBool>,
    refresh_req: Arc<AtomicBool>,
    pub latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    pub last_error: Arc<Mutex<Option<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl BrowserWorker {
    pub fn start(source_id: SourceId, spec: BrowserSpec, clock: Clock) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let refresh_req = Arc::new(AtomicBool::new(true));
        let latest_frame = Arc::new(RwLock::new(None));
        let last_error = Arc::new(Mutex::new(None));

        let r = Arc::clone(&running);
        let rr = Arc::clone(&refresh_req);
        let lf = Arc::clone(&latest_frame);
        let le = Arc::clone(&last_error);
        let spec_c = spec.clone();

        let handle = thread::Builder::new()
            .name(format!("obs-browser-{}", &source_id.to_string()[..8]))
            .spawn(move || browser_loop(spec_c, r, rr, lf, le, clock))
            .ok();

        Self {
            spec,
            running,
            refresh_req,
            latest_frame,
            last_error,
            handle,
        }
    }

    pub fn trigger_refresh(&self) {
        self.refresh_req.store(true, Ordering::Relaxed);
    }
}

impl Drop for BrowserWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn browser_loop(
    spec: BrowserSpec,
    running: Arc<AtomicBool>,
    refresh_req: Arc<AtomicBool>,
    latest_frame: Arc<RwLock<Option<VideoFrame>>>,
    last_error: Arc<Mutex<Option<String>>>,
    clock: Clock,
) {
    let Some(browser_exe) = find_edge_browser() else {
        *last_error.lock() =
            Some("Microsoft Edge / Chromium executable not found for Browser Source".to_string());
        return;
    };

    let tmp_dir = std::env::temp_dir().join("oxidestudio-browser");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let shot_path = tmp_dir.join(format!("shot-{}.png", Uuid::new_v4()));
    let w = spec.width.clamp(64, 3840);
    let h = spec.height.clamp(64, 2160);
    let refresh_interval = Duration::from_millis((1000 / spec.fps.clamp(1, 5)).max(1500) as u64);
    let mut seq = 0u64;

    while running.load(Ordering::Relaxed) {
        let _ = refresh_req.swap(false, Ordering::Relaxed);
        let mut cmd = Command::new(&browser_exe);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-sandbox",
                "--hide-scrollbars",
                "--virtual-time-budget=1500",
                &format!("--window-size={w},{h}"),
                &format!("--screenshot={}", shot_path.display()),
                &spec.url,
            ])
            .output();

        match out {
            Ok(res) if res.status.success() && shot_path.exists() => {
                if let Ok(img) = image::open(&shot_path) {
                    let rgba = img.to_rgba8();
                    let (iw, ih) = rgba.dimensions();
                    seq += 1;
                    *latest_frame.write() = Some(VideoFrame::new_rgba(
                        iw,
                        ih,
                        rgba.into_raw(),
                        clock.now_us(),
                        seq,
                    ));
                    *last_error.lock() = None;
                }
            }
            Ok(res) => {
                *last_error.lock() = Some(format!(
                    "Browser capture exited with code {:?}",
                    res.status.code()
                ));
            }
            Err(e) => {
                *last_error.lock() = Some(format!("Failed launching browser: {e}"));
            }
        }

        let start = std::time::Instant::now();
        while start.elapsed() < refresh_interval
            && running.load(Ordering::Relaxed)
            && !refresh_req.load(Ordering::Relaxed)
        {
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = std::fs::remove_file(shot_path);
}
