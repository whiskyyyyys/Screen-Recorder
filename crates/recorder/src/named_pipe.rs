use obs_core::{ObsError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{HRESULT, HSTRING};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, WAIT_EVENT,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, WriteFile, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_OUTBOUND,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const PIPE_IO_TIMEOUT_MS: u32 = 15_000;

/// Thread-safe cancellation token that unblocks pending overlapped named-pipe I/O immediately.
pub struct PipeCancelToken {
    event: HANDLE,
    cancelled: AtomicBool,
}

unsafe impl Send for PipeCancelToken {}
unsafe impl Sync for PipeCancelToken {}

impl PipeCancelToken {
    pub fn new() -> Result<Self> {
        let event = unsafe {
            CreateEventW(None, true, false, None).map_err(|e| {
                ObsError::Recorder(format!("CreateEventW failed for PipeCancelToken: {e}"))
            })?
        };
        Ok(Self {
            event,
            cancelled: AtomicBool::new(false),
        })
    }

    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            unsafe {
                let _ = SetEvent(self.event);
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for PipeCancelToken {
    fn drop(&mut self) {
        unsafe {
            if !self.event.is_invalid() {
                let _ = CloseHandle(self.event);
                self.event = INVALID_HANDLE_VALUE;
            }
        }
    }
}

pub struct WindowsNamedPipe {
    handle: HANDLE,
    pub path: String,
    connected: bool,
    cancel: Arc<PipeCancelToken>,
}

unsafe impl Send for WindowsNamedPipe {}

impl WindowsNamedPipe {
    pub fn create(name: &str) -> Result<Self> {
        let cancel = Arc::new(PipeCancelToken::new()?);
        let full_path = format!(r"\\.\pipe\{name}");
        let wide = HSTRING::from(&full_path);
        unsafe {
            let handle = CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                1024 * 512,
                1024 * 512,
                0,
                None,
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(ObsError::Recorder(format!(
                    "CreateNamedPipeW failed for {full_path}: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(Self {
                handle,
                path: full_path,
                connected: false,
                cancel,
            })
        }
    }

    pub fn cancel_token(&self) -> Arc<PipeCancelToken> {
        Arc::clone(&self.cancel)
    }

    pub fn connect(&mut self) -> Result<()> {
        if self.connected {
            return Ok(());
        }
        if self.cancel.is_cancelled() {
            return Err(ObsError::Recorder(format!(
                "Pipe connect cancelled before start on {}",
                self.path
            )));
        }

        unsafe {
            let io_event = CreateEventW(None, true, false, None)
                .map_err(|e| ObsError::Recorder(format!("CreateEventW connect: {e}")))?;
            let _ev_guard = HandleGuard(io_event);
            let mut overlapped = OVERLAPPED {
                hEvent: io_event,
                ..Default::default()
            };

            match ConnectNamedPipe(self.handle, Some(&mut overlapped)) {
                Ok(()) => {}
                Err(e) => {
                    let pipe_connected = HRESULT::from_win32(ERROR_PIPE_CONNECTED.0);
                    let io_pending = HRESULT::from_win32(ERROR_IO_PENDING.0);
                    if e.code() == pipe_connected {
                        self.connected = true;
                        return Ok(());
                    } else if e.code() == io_pending {
                        wait_overlapped_or_cancel(
                            self.handle,
                            &mut overlapped,
                            io_event,
                            &self.cancel,
                            PIPE_IO_TIMEOUT_MS,
                            &self.path,
                            "connect",
                        )?;
                    } else {
                        return Err(ObsError::Recorder(format!(
                            "ConnectNamedPipe failed on {}: {e}",
                            self.path
                        )));
                    }
                }
            }
        }
        self.connected = true;
        Ok(())
    }

    pub fn write_all(&mut self, mut data: &[u8]) -> Result<()> {
        if !self.connected {
            self.connect()?;
        }
        unsafe {
            while !data.is_empty() {
                if self.cancel.is_cancelled() {
                    return Err(ObsError::Recorder(format!(
                        "Pipe write cancelled on {}",
                        self.path
                    )));
                }
                let io_event = CreateEventW(None, true, false, None)
                    .map_err(|e| ObsError::Recorder(format!("CreateEventW write: {e}")))?;
                let _ev_guard = HandleGuard(io_event);
                let mut overlapped = OVERLAPPED {
                    hEvent: io_event,
                    ..Default::default()
                };

                let mut written: u32 = 0;
                let res = WriteFile(
                    self.handle,
                    Some(data),
                    Some(&mut written),
                    Some(&mut overlapped),
                );
                match res {
                    Ok(()) => {
                        let _ = GetOverlappedResult(self.handle, &overlapped, &mut written, false);
                    }
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
                        written = wait_overlapped_or_cancel(
                            self.handle,
                            &mut overlapped,
                            io_event,
                            &self.cancel,
                            PIPE_IO_TIMEOUT_MS,
                            &self.path,
                            "write",
                        )?;
                    }
                    Err(e) => {
                        return Err(ObsError::Recorder(format!(
                            "Pipe WriteFile error on {}: {e}",
                            self.path
                        )));
                    }
                }

                if written == 0 {
                    return Err(ObsError::Recorder("Pipe WriteFile wrote 0 bytes".into()));
                }
                data = &data[written as usize..];
            }
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.cancel.cancel();
        unsafe {
            if self.handle != INVALID_HANDLE_VALUE {
                if self.connected {
                    let _ = FlushFileBuffers(self.handle);
                    let _ = DisconnectNamedPipe(self.handle);
                }
                let _ = CloseHandle(self.handle);
                self.handle = INVALID_HANDLE_VALUE;
                self.connected = false;
            }
        }
    }
}

impl Drop for WindowsNamedPipe {
    fn drop(&mut self) {
        self.close();
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

unsafe fn wait_overlapped_or_cancel(
    handle: HANDLE,
    overlapped: &mut OVERLAPPED,
    io_event: HANDLE,
    cancel: &PipeCancelToken,
    timeout_ms: u32,
    path: &str,
    op: &str,
) -> Result<u32> {
    let handles = [io_event, cancel.event];
    let wait_res = WaitForMultipleObjects(&handles, false, timeout_ms);
    let mut transferred = 0u32;

    if wait_res == WAIT_OBJECT_0 {
        GetOverlappedResult(handle, overlapped, &mut transferred, true).map_err(|e| {
            ObsError::Recorder(format!("GetOverlappedResult {op} failed on {path}: {e}"))
        })?;
        return Ok(transferred);
    }

    // Cancel pending I/O and drain overlapped completion BEFORE returning so stack OVERLAPPED stays valid.
    let _ = CancelIoEx(handle, Some(overlapped));
    let _ = GetOverlappedResult(handle, overlapped, &mut transferred, true);

    if wait_res == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
        Err(ObsError::Recorder(format!(
            "Named pipe {op} cancelled on {path}"
        )))
    } else if wait_res == WAIT_TIMEOUT {
        Err(ObsError::Recorder(format!(
            "Named pipe {op} timed out after {timeout_ms}ms on {path}"
        )))
    } else {
        Err(ObsError::Recorder(format!(
            "WaitForMultipleObjects {op} failed (0x{:08X}) on {path}",
            wait_res.0
        )))
    }
}
