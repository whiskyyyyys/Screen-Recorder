pub mod display_capture;
pub mod game_capture;
pub mod hotkeys;
pub mod mf_probe;
pub mod monitors;
pub mod wasapi;
pub mod webcam_capture;
pub mod window_capture;

pub use display_capture::DisplayCapturer;
pub use game_capture::GameCapturer;
pub use hotkeys::{HotkeyListener, KeyTriggerEvent};
pub use mf_probe::{probe_mf_video_encoders, MfEncoderInfo};
pub use monitors::{enumerate_monitors, MonitorInfo};
pub use wasapi::{
    enumerate_audio_devices, AudioDeviceInfo, AudioEndpointKind, WasapiCaptureStream,
    WasapiRenderStream,
};
pub use webcam_capture::{enumerate_video_devices, VideoDeviceInfo, WebcamCapturer};
pub use window_capture::{enumerate_windows, WindowCapturer, WindowInfo};

#[doc(hidden)]
pub use window_capture::{spawn_test_capture_window, TestWindowHandle};

/// RAII guard enabling high-resolution Windows multimedia timer ticks (`timeBeginPeriod`).
/// Ensures monotonic real-time CFR pacing loops don't drift from the default 15.6ms timer tick.
pub struct HighResTimerGuard(u32);

impl HighResTimerGuard {
    pub fn new(period_ms: u32) -> Self {
        let p = period_ms.clamp(1, 16);
        unsafe {
            windows::Win32::Media::timeBeginPeriod(p);
        }
        Self(p)
    }
}

impl Drop for HighResTimerGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::Media::timeEndPeriod(self.0);
        }
    }
}
