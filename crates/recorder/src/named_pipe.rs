use obs_core::{ObsError, Result};
use std::ptr;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, WriteFile, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};

pub struct WindowsNamedPipe {
    handle: HANDLE,
    pub path: String,
    connected: bool,
}

unsafe impl Send for WindowsNamedPipe {}

impl WindowsNamedPipe {
    pub fn create(name: &str) -> Result<Self> {
        let full_path = format!(r"\\.\pipe\{name}");
        let wide = HSTRING::from(&full_path);
        unsafe {
            let handle = CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_OUTBOUND,
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
            })
        }
    }

    pub fn connect(&mut self) -> Result<()> {
        if self.connected {
            return Ok(());
        }
        unsafe {
            // ConnectNamedPipe returns Ok or ERROR_PIPE_CONNECTED (Wait: handle result properly)
            if let Err(e) = ConnectNamedPipe(self.handle, None) {
                const ERROR_PIPE_CONNECTED_CODE: i32 = -2147024361; // HRESULT_FROM_WIN32(535)
                if e.code().0 != ERROR_PIPE_CONNECTED_CODE {
                    return Err(ObsError::Recorder(format!(
                        "ConnectNamedPipe failed on {}: {e}",
                        self.path
                    )));
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
                let mut written: u32 = 0;
                WriteFile(self.handle, Some(data), Some(&mut written), None).map_err(|e| {
                    ObsError::Recorder(format!("Pipe WriteFile error on {}: {e}", self.path))
                })?;
                if written == 0 {
                    return Err(ObsError::Recorder("Pipe WriteFile wrote 0 bytes".into()));
                }
                data = &data[written as usize..];
            }
        }
        Ok(())
    }

    pub fn close(&mut self) {
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

#[allow(dead_code)]
fn _ptr_null_check() -> *mut std::ffi::c_void {
    ptr::null_mut()
}
