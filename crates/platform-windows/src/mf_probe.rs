use serde::{Deserialize, Serialize};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, MFTEnumEx, MFT_FRIENDLY_NAME_Attribute, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG,
    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SYNCMFT,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfEncoderInfo {
    pub friendly_name: String,
    pub hardware: bool,
}

/// Queries Windows Media Foundation natively via `MFTEnumEx` for installed video encoder MFTs.
pub fn probe_mf_video_encoders() -> Vec<MfEncoderInfo> {
    let mut out = Vec::new();
    unsafe {
        let com_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;
        let flags = MFT_ENUM_FLAG(
            MFT_ENUM_FLAG_HARDWARE.0
                | MFT_ENUM_FLAG_SYNCMFT.0
                | MFT_ENUM_FLAG_ASYNCMFT.0
                | MFT_ENUM_FLAG_LOCALMFT.0,
        );

        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            None,
            &mut list,
            &mut count,
        )
        .is_ok()
            && !list.is_null()
        {
            let slice = std::slice::from_raw_parts(list, count as usize);
            for item in slice.iter().flatten() {
                let mut name_ptr = windows::core::PWSTR::null();
                let mut name_len = 0u32;
                if item
                    .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name_ptr, &mut name_len)
                    .is_ok()
                    && !name_ptr.is_null()
                {
                    let wide = std::slice::from_raw_parts(name_ptr.0, name_len as usize);
                    let name = String::from_utf16_lossy(wide);
                    CoTaskMemFree(Some(name_ptr.0 as _));
                    let lower = name.to_ascii_lowercase();
                    let hardware = lower.contains("amd")
                        || lower.contains("nvidia")
                        || lower.contains("intel")
                        || lower.contains("qsv")
                        || lower.contains("nvenc")
                        || lower.contains("amf")
                        || lower.contains("hardware");
                    out.push(MfEncoderInfo {
                        friendly_name: name,
                        hardware,
                    });
                }
            }
            CoTaskMemFree(Some(list as _));
        }
        if com_ok {
            CoUninitialize();
        }
    }
    out
}
