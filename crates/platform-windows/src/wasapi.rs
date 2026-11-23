use obs_core::{ObsError, Result};
use serde::{Deserialize, Serialize};
use std::mem::size_of;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use windows::core::{implement, Interface, GUID, HSTRING, PCWSTR, PROPVARIANT};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
    STGM_READ,
};
use windows::Win32::System::Variant::VT_BLOB;

const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioEndpointKind {
    Input,
    Output,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: AudioEndpointKind,
}

pub fn enumerate_audio_devices(kind: AudioEndpointKind) -> Vec<AudioDeviceInfo> {
    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(enumerator): windows::core::Result<IMMDeviceEnumerator> =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return out;
        };
        let flow = match kind {
            AudioEndpointKind::Input => eCapture,
            AudioEndpointKind::Output => eRender,
        };
        let Ok(collection) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let count = collection.GetCount().unwrap_or(0);
        for i in 0..count {
            if let Ok(dev) = collection.Item(i) {
                if let Some(info) = device_to_info(&dev, kind) {
                    out.push(info);
                }
            }
        }
    }
    out
}

unsafe fn device_to_info(dev: &IMMDevice, kind: AudioEndpointKind) -> Option<AudioDeviceInfo> {
    let id_pwstr = dev.GetId().ok()?;
    let id = id_pwstr.to_string().unwrap_or_default();
    CoTaskMemFree(Some(id_pwstr.0 as _));

    let name = if let Ok(store) = dev.OpenPropertyStore(STGM_READ) {
        if let Ok(mut pv) = store.GetValue(&PKEY_Device_FriendlyName) {
            let s = pv.to_string();
            let _ = PropVariantClear(&mut pv);
            if s.is_empty() {
                "Audio Device".to_string()
            } else {
                s
            }
        } else {
            "Audio Device".to_string()
        }
    } else {
        "Audio Device".to_string()
    };

    Some(AudioDeviceInfo { id, name, kind })
}

pub struct WasapiCaptureStream {
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    is_float: bool,
}

unsafe impl Send for WasapiCaptureStream {}

impl WasapiCaptureStream {
    pub fn open_endpoint(device_id: &str, kind: AudioEndpointKind) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| ObsError::Audio(format!("MMDeviceEnumerator: {e}")))?;

            let flow = match kind {
                AudioEndpointKind::Input => eCapture,
                AudioEndpointKind::Output => eRender,
            };

            let dev: IMMDevice =
                if device_id.eq_ignore_ascii_case("default") || device_id.is_empty() {
                    enumerator
                        .GetDefaultAudioEndpoint(flow, eConsole)
                        .map_err(|e| ObsError::Audio(format!("GetDefaultAudioEndpoint: {e}")))?
                } else {
                    let wide = HSTRING::from(device_id);
                    enumerator
                        .GetDevice(PCWSTR(wide.as_ptr()))
                        .or_else(|_| enumerator.GetDefaultAudioEndpoint(flow, eConsole))
                        .map_err(|e| ObsError::Audio(format!("GetDevice: {e}")))?
                };

            let audio_client: IAudioClient = dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| ObsError::Audio(format!("Activate IAudioClient: {e}")))?;

            let mix_fmt_ptr = audio_client
                .GetMixFormat()
                .map_err(|e| ObsError::Audio(format!("GetMixFormat: {e}")))?;
            let (sample_rate, channels, bits_per_sample, is_float) = parse_wave_format(mix_fmt_ptr);

            let flags = match kind {
                AudioEndpointKind::Input => 0,
                AudioEndpointKind::Output => AUDCLNT_STREAMFLAGS_LOOPBACK,
            };

            audio_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    300_000, // 30ms buffer in 100ns units
                    0,
                    mix_fmt_ptr,
                    None,
                )
                .map_err(|e| ObsError::Audio(format!("IAudioClient::Initialize: {e}")))?;
            CoTaskMemFree(Some(mix_fmt_ptr as _));

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .map_err(|e| ObsError::Audio(format!("GetService IAudioCaptureClient: {e}")))?;
            audio_client
                .Start()
                .map_err(|e| ObsError::Audio(format!("IAudioClient::Start: {e}")))?;

            Ok(Self {
                audio_client,
                capture_client,
                sample_rate,
                channels,
                bits_per_sample,
                is_float,
            })
        }
    }

    /// Windows 10 2004+ / Windows 11 per-process audio loopback capture (`VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK`).
    pub fn open_process_loopback(pid: u32, sample_rate: u32) -> Result<Self> {
        if pid == 0 {
            return Err(ObsError::Audio(
                "Select a running application PID for Application Audio Capture".to_string(),
            ));
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };

            let mut prop = PROPVARIANT::default();
            // Safety: construct VT_BLOB PROPVARIANT manually
            let raw_ptr = &mut prop as *mut PROPVARIANT as *mut RawPropVariantBlob;
            (*raw_ptr).vt = VT_BLOB.0;
            (*raw_ptr).blob = BLOB {
                cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: &mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8,
            };

            let signal = Arc::new((Mutex::new(false), Condvar::new()));
            let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler {
                signal: Arc::clone(&signal),
            }
            .into();

            let op = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&prop as *const PROPVARIANT),
                &handler,
            )
            .map_err(|e| ObsError::Audio(format!("ActivateAudioInterfaceAsync: {e}")))?;

            {
                let (lock, cvar) = &*signal;
                let mut done = lock.lock().unwrap();
                while !*done {
                    let res = cvar
                        .wait_timeout(done, std::time::Duration::from_secs(2))
                        .unwrap();
                    done = res.0;
                    if res.1.timed_out() {
                        return Err(ObsError::Audio(
                            "Process loopback activation timed out".to_string(),
                        ));
                    }
                }
            }

            let mut hr = windows::core::HRESULT(0);
            let mut iface = None;
            op.GetActivateResult(&mut hr, &mut iface)
                .map_err(|e| ObsError::Audio(format!("GetActivateResult: {e}")))?;
            hr.ok()
                .map_err(|e| ObsError::Audio(format!("Process loopback activation HR: {e}")))?;

            let unk =
                iface.ok_or_else(|| ObsError::Audio("Null process loopback IUnknown".into()))?;
            let audio_client: IAudioClient = unk
                .cast()
                .map_err(|e| ObsError::Audio(format!("Cast IAudioClient: {e}")))?;

            let sr = sample_rate.max(44100);
            let wfx = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
                nChannels: 2,
                nSamplesPerSec: sr,
                nAvgBytesPerSec: sr * 8,
                nBlockAlign: 8,
                wBitsPerSample: 32,
                cbSize: 0,
            };

            audio_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    300_000,
                    0,
                    &wfx,
                    None,
                )
                .map_err(|e| ObsError::Audio(format!("Process loopback Initialize: {e}")))?;

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .map_err(|e| ObsError::Audio(format!("Process loopback GetService: {e}")))?;
            audio_client
                .Start()
                .map_err(|e| ObsError::Audio(format!("Process loopback Start: {e}")))?;

            Ok(Self {
                audio_client,
                capture_client,
                sample_rate: sr,
                channels: 2,
                bits_per_sample: 32,
                is_float: true,
            })
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Reads all queued packets and converts them into interleaved stereo `f32` samples.
    pub fn read_stereo_f32(&self) -> Result<Vec<f32>> {
        let mut out = Vec::new();
        unsafe {
            loop {
                let next_frames = self
                    .capture_client
                    .GetNextPacketSize()
                    .map_err(|e| ObsError::Audio(format!("GetNextPacketSize: {e}")))?;
                if next_frames == 0 {
                    break;
                }
                let mut data_ptr: *mut u8 = ptr::null_mut();
                let mut num_frames = 0u32;
                let mut flags = 0u32;
                self.capture_client
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                    .map_err(|e| ObsError::Audio(format!("GetBuffer: {e}")))?;

                const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
                if (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || data_ptr.is_null() {
                    out.resize(out.len() + (num_frames as usize) * 2, 0.0);
                } else {
                    append_converted_stereo(
                        &mut out,
                        data_ptr,
                        num_frames as usize,
                        self.channels as usize,
                        self.bits_per_sample,
                        self.is_float,
                    );
                }
                let _ = self.capture_client.ReleaseBuffer(num_frames);
            }
        }
        Ok(out)
    }
}

impl Drop for WasapiCaptureStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.audio_client.Stop();
        }
    }
}

pub struct WasapiRenderStream {
    audio_client: IAudioClient,
    render_client: IAudioRenderClient,
    buffer_frames: u32,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    is_float: bool,
}

unsafe impl Send for WasapiRenderStream {}

impl WasapiRenderStream {
    pub fn open_output(device_id: &str) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| ObsError::Audio(format!("MMDeviceEnumerator: {e}")))?;
            let dev: IMMDevice =
                if device_id.eq_ignore_ascii_case("default") || device_id.is_empty() {
                    enumerator
                        .GetDefaultAudioEndpoint(eRender, eConsole)
                        .map_err(|e| ObsError::Audio(format!("GetDefaultAudioEndpoint: {e}")))?
                } else {
                    let wide = HSTRING::from(device_id);
                    enumerator
                        .GetDevice(PCWSTR(wide.as_ptr()))
                        .or_else(|_| enumerator.GetDefaultAudioEndpoint(eRender, eConsole))
                        .map_err(|e| ObsError::Audio(format!("GetDevice monitor: {e}")))?
                };

            let audio_client: IAudioClient = dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| ObsError::Audio(format!("Activate monitor IAudioClient: {e}")))?;
            let mix_fmt_ptr = audio_client
                .GetMixFormat()
                .map_err(|e| ObsError::Audio(format!("Monitor GetMixFormat: {e}")))?;
            let (sample_rate, channels, bits_per_sample, is_float) = parse_wave_format(mix_fmt_ptr);

            audio_client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 400_000, 0, mix_fmt_ptr, None)
                .map_err(|e| ObsError::Audio(format!("Monitor Initialize: {e}")))?;
            CoTaskMemFree(Some(mix_fmt_ptr as _));

            let buffer_frames = audio_client.GetBufferSize().unwrap_or(1024);
            let render_client: IAudioRenderClient = audio_client
                .GetService()
                .map_err(|e| ObsError::Audio(format!("Monitor GetService: {e}")))?;
            audio_client
                .Start()
                .map_err(|e| ObsError::Audio(format!("Monitor Start: {e}")))?;

            Ok(Self {
                audio_client,
                render_client,
                buffer_frames,
                sample_rate,
                channels,
                bits_per_sample,
                is_float,
            })
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn write_stereo_f32(&self, stereo: &[f32]) {
        if stereo.len() < 2 {
            return;
        }
        unsafe {
            let padding = self.audio_client.GetCurrentPadding().unwrap_or(0);
            let avail = self.buffer_frames.saturating_sub(padding) as usize;
            let in_frames = stereo.len() / 2;
            let frames = avail.min(in_frames);
            if frames == 0 {
                return;
            }
            if let Ok(buf_ptr) = self.render_client.GetBuffer(frames as u32) {
                let ch = self.channels.max(1) as usize;
                if self.is_float && self.bits_per_sample == 32 {
                    let out = std::slice::from_raw_parts_mut(buf_ptr as *mut f32, frames * ch);
                    for i in 0..frames {
                        let l = stereo[i * 2];
                        let r = stereo[i * 2 + 1];
                        if ch == 1 {
                            out[i] = 0.5 * (l + r);
                        } else {
                            out[i * ch] = l;
                            out[i * ch + 1] = r;
                            for c in 2..ch {
                                out[i * ch + c] = 0.0;
                            }
                        }
                    }
                } else if self.bits_per_sample == 16 {
                    let out = std::slice::from_raw_parts_mut(buf_ptr as *mut i16, frames * ch);
                    for i in 0..frames {
                        let l = (stereo[i * 2].clamp(-1.0, 1.0) * 32767.0) as i16;
                        let r = (stereo[i * 2 + 1].clamp(-1.0, 1.0) * 32767.0) as i16;
                        if ch == 1 {
                            out[i] = ((l as i32 + r as i32) / 2) as i16;
                        } else {
                            out[i * ch] = l;
                            out[i * ch + 1] = r;
                        }
                    }
                }
                let _ = self.render_client.ReleaseBuffer(frames as u32, 0);
            }
        }
    }
}

impl Drop for WasapiRenderStream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.audio_client.Stop();
        }
    }
}

unsafe fn parse_wave_format(fmt: *const WAVEFORMATEX) -> (u32, u16, u16, bool) {
    let wfx = &*fmt;
    let sr = wfx.nSamplesPerSec.max(8000);
    let ch = wfx.nChannels.max(1);
    let bits = wfx.wBitsPerSample;
    let is_float = if wfx.wFormatTag == WAVE_FORMAT_IEEE_FLOAT {
        true
    } else if wfx.wFormatTag == WAVE_FORMAT_EXTENSIBLE_TAG {
        let ext = &*(fmt as *const WAVEFORMATEXTENSIBLE);
        let sub = ext.SubFormat;
        sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        false
    };
    (sr, ch, bits, is_float)
}

unsafe fn append_converted_stereo(
    out: &mut Vec<f32>,
    data_ptr: *const u8,
    frames: usize,
    channels: usize,
    bits: u16,
    is_float: bool,
) {
    if channels == 0 || frames == 0 {
        return;
    }
    if is_float && bits == 32 {
        let src = std::slice::from_raw_parts(data_ptr as *const f32, frames * channels);
        for f in 0..frames {
            let l = src[f * channels];
            let r = if channels > 1 {
                src[f * channels + 1]
            } else {
                l
            };
            out.push(l);
            out.push(r);
        }
    } else if bits == 16 {
        let src = std::slice::from_raw_parts(data_ptr as *const i16, frames * channels);
        for f in 0..frames {
            let l = src[f * channels] as f32 / 32768.0;
            let r = if channels > 1 {
                src[f * channels + 1] as f32 / 32768.0
            } else {
                l
            };
            out.push(l);
            out.push(r);
        }
    } else if bits == 32 && !is_float {
        let src = std::slice::from_raw_parts(data_ptr as *const i32, frames * channels);
        for f in 0..frames {
            let l = src[f * channels] as f32 / 2147483648.0;
            let r = if channels > 1 {
                src[f * channels + 1] as f32 / 2147483648.0
            } else {
                l
            };
            out.push(l);
            out.push(r);
        }
    } else {
        out.resize(out.len() + frames * 2, 0.0);
    }
}

#[repr(C)]
struct RawPropVariantBlob {
    vt: u16,
    reserved1: u16,
    reserved2: u16,
    reserved3: u16,
    blob: BLOB,
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    signal: Arc<(Mutex<bool>, Condvar)>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        _activateoperation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let (lock, cvar) = &*self.signal;
        if let Ok(mut done) = lock.lock() {
            *done = true;
            cvar.notify_all();
        }
        Ok(())
    }
}
