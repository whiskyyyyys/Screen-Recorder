use obs_core::{ObsError, Result};
use obs_video::{bgra_to_rgba_vec, VideoFrame};
use serde::{Deserialize, Serialize};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaSource, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video, MFStartup,
    MFVideoFormat_RGB32, MFSTARTUP_NOSOCKET, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDeviceInfo {
    pub id: String,
    pub name: String,
}

pub fn enumerate_video_devices() -> Vec<VideoDeviceInfo> {
    let mut list = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
            return list;
        }
        let mut attrs = None;
        if MFCreateAttributes(&mut attrs, 1).is_err() {
            return list;
        }
        let Some(attrs) = attrs else {
            return list;
        };
        if attrs
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .is_err()
        {
            return list;
        }
        let mut devs: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;
        if MFEnumDeviceSources(&attrs, &mut devs, &mut count).is_ok() && !devs.is_null() {
            let slice = std::slice::from_raw_parts(devs, count as usize);
            for dev in slice.iter().flatten() {
                let name = get_activate_string(dev, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                    .unwrap_or_else(|| "Camera".to_string());
                let id = get_activate_string(
                    dev,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                )
                .unwrap_or_else(|| name.clone());
                list.push(VideoDeviceInfo { id, name });
            }
            CoTaskMemFree(Some(devs as _));
        }
    }
    list
}

unsafe fn get_activate_string(act: &IMFActivate, key: &windows::core::GUID) -> Option<String> {
    let mut ptr = windows::core::PWSTR::null();
    let mut len = 0u32;
    if act.GetAllocatedString(key, &mut ptr, &mut len).is_ok() && !ptr.is_null() {
        let slice = std::slice::from_raw_parts(ptr.0, len as usize);
        let s = String::from_utf16_lossy(slice);
        CoTaskMemFree(Some(ptr.0 as _));
        Some(s)
    } else {
        None
    }
}

pub struct WebcamCapturer {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    seq: u64,
}

unsafe impl Send for WebcamCapturer {}

impl WebcamCapturer {
    pub fn open(device_id: &str, req_width: u32, req_height: u32, req_fps: u32) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
                .map_err(|e| ObsError::Capture(format!("MFStartup: {e}")))?;

            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)
                .map_err(|e| ObsError::Capture(format!("MFCreateAttributes: {e}")))?;
            let attrs = attrs.ok_or_else(|| ObsError::Capture("Null MF attributes".into()))?;
            attrs
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(|e| ObsError::Capture(format!("SetGUID: {e}")))?;

            let mut devs: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count: u32 = 0;
            MFEnumDeviceSources(&attrs, &mut devs, &mut count)
                .map_err(|e| ObsError::Capture(format!("MFEnumDeviceSources: {e}")))?;
            if devs.is_null() || count == 0 {
                return Err(ObsError::Capture(
                    "No video capture devices connected".to_string(),
                ));
            }

            let slice = std::slice::from_raw_parts(devs, count as usize);
            let mut chosen: Option<IMFActivate> = None;
            for dev in slice.iter().flatten() {
                let id = get_activate_string(
                    dev,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                )
                .unwrap_or_default();
                let name = get_activate_string(dev, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                    .unwrap_or_default();
                if device_id == "default"
                    || device_id.is_empty()
                    || id == device_id
                    || name == device_id
                {
                    chosen = Some(dev.clone());
                    break;
                }
            }
            if chosen.is_none() {
                chosen = slice.iter().flatten().next().cloned();
            }
            let activate = chosen
                .ok_or_else(|| ObsError::Capture("Requested video device not found".to_string()))?;
            let media_source: IMFMediaSource = activate
                .ActivateObject()
                .map_err(|e| ObsError::Capture(format!("ActivateObject webcam: {e}")))?;
            CoTaskMemFree(Some(devs as _));

            let mut reader_attrs = None;
            MFCreateAttributes(&mut reader_attrs, 1)
                .map_err(|e| ObsError::Capture(format!("Reader attrs: {e}")))?;
            if let Some(ra) = &reader_attrs {
                let _ = ra.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
            }

            let reader = MFCreateSourceReaderFromMediaSource(&media_source, reader_attrs.as_ref())
                .map_err(|e| ObsError::Capture(format!("MFCreateSourceReader: {e}")))?;

            let mt = MFCreateMediaType()
                .map_err(|e| ObsError::Capture(format!("MFCreateMediaType: {e}")))?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| ObsError::Capture(format!("SetGUID major: {e}")))?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| ObsError::Capture(format!("SetGUID RGB32: {e}")))?;
            if req_width > 0 && req_height > 0 {
                let packed_size = ((req_width as u64) << 32) | (req_height as u64);
                let _ = mt.SetUINT64(&MF_MT_FRAME_SIZE, packed_size);
            }
            if req_fps > 0 {
                let packed_fps = ((req_fps as u64) << 32) | 1u64;
                let _ = mt.SetUINT64(&MF_MT_FRAME_RATE, packed_fps);
            }

            let stream_idx = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            if reader.SetCurrentMediaType(stream_idx, None, &mt).is_err() {
                // Retry with minimal RGB32 request without explicit size/fps override
                let mt_min = MFCreateMediaType()
                    .map_err(|e| ObsError::Capture(format!("MFCreateMediaType min: {e}")))?;
                let _ = mt_min.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
                let _ = mt_min.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32);
                reader
                    .SetCurrentMediaType(stream_idx, None, &mt_min)
                    .map_err(|e| ObsError::Capture(format!("SetCurrentMediaType RGB32: {e}")))?;
            }

            let actual_mt = reader
                .GetCurrentMediaType(stream_idx)
                .map_err(|e| ObsError::Capture(format!("GetCurrentMediaType: {e}")))?;
            let packed = actual_mt
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| ObsError::Capture(format!("GetUINT64 frame size: {e}")))?;
            let width = (packed >> 32) as u32;
            let height = (packed & 0xFFFF_FFFF) as u32;

            Ok(Self {
                reader,
                width: width.max(16),
                height: height.max(16),
                seq: 0,
            })
        }
    }

    pub fn read_frame(&mut self, pts_us: u64) -> Result<Option<VideoFrame>> {
        unsafe {
            let stream_idx = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let mut actual_idx = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            self.reader
                .ReadSample(
                    stream_idx,
                    0,
                    Some(&mut actual_idx),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| ObsError::Capture(format!("ReadSample: {e}")))?;

            let Some(sample) = sample else {
                return Ok(None);
            };
            let buf = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| ObsError::Capture(format!("ConvertToContiguousBuffer: {e}")))?;
            let mut ptr_data: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buf.Lock(&mut ptr_data, Some(&mut max_len), Some(&mut cur_len))
                .map_err(|e| ObsError::Capture(format!("Lock buffer: {e}")))?;

            let expected = (self.width as usize) * (self.height as usize) * 4;
            let frame = if !ptr_data.is_null() && (cur_len as usize) >= expected {
                let slice = std::slice::from_raw_parts(ptr_data, expected);
                let mut rgba = bgra_to_rgba_vec(slice, self.width, self.height, self.width * 4);
                for px in rgba.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                self.seq += 1;
                Some(VideoFrame::new_rgba(
                    self.width,
                    self.height,
                    rgba,
                    pts_us,
                    self.seq,
                ))
            } else {
                None
            };
            let _ = buf.Unlock();
            Ok(frame)
        }
    }
}
