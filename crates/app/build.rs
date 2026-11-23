use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/obs-rust.ico");
    let ico_path = Path::new("../../assets/icons/obs-rust.ico");
    if !ico_path.exists() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("app_icon.rc");
    let res_obj = out_dir.join("app_icon_res.obj");
    let abs_ico = fs::canonicalize(ico_path).unwrap_or_else(|_| ico_path.to_path_buf());
    let rc_ico = abs_ico
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    let _ = fs::write(&rc_path, format!("1 ICON \"{rc_ico}\"\n"));

    // Use native windres on GNU toolchains if installed in PATH,
    // otherwise emit a pure-Rust x64 COFF .rsrc object so the Windows EXE icon always embeds cleanly on MSVC too!
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        if let Ok(status) = Command::new("windres")
            .args([
                rc_path.to_str().unwrap(),
                "-O",
                "coff",
                "-o",
                res_obj.to_str().unwrap(),
            ])
            .status()
        {
            if status.success() && res_obj.exists() {
                println!("cargo:rustc-link-arg-bin=obs-rust={}", res_obj.display());
                return;
            }
        }
    }

    if let Ok(ico_bytes) = fs::read(ico_path) {
        if let Some(coff) = build_icon_coff_x64(&ico_bytes) {
            if fs::write(&res_obj, coff).is_ok() {
                println!("cargo:rustc-link-arg-bin=obs-rust={}", res_obj.display());
            }
        }
    }
}

/// Builds a minimal x86_64 COFF object containing a valid `.rsrc` section with RT_ICON + RT_GROUP_ICON.
fn build_icon_coff_x64(ico: &[u8]) -> Option<Vec<u8>> {
    if ico.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if count == 0 || ico.len() < 6 + count * 16 {
        return None;
    }

    struct Entry {
        dir_bytes: [u8; 12],
        offset: usize,
        size: usize,
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let base = 6 + i * 16;
        let size = u32::from_le_bytes(ico[base + 8..base + 12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(ico[base + 12..base + 16].try_into().ok()?) as usize;
        if offset + size > ico.len() {
            return None;
        }
        let mut dir_bytes = [0u8; 12];
        dir_bytes.copy_from_slice(&ico[base..base + 12]);
        entries.push(Entry {
            dir_bytes,
            offset,
            size,
        });
    }

    // Build GRPICONDIR blob
    let mut grp = Vec::with_capacity(6 + count * 14);
    grp.extend_from_slice(&ico[0..6]);
    for (i, e) in entries.iter().enumerate() {
        grp.extend_from_slice(&e.dir_bytes);
        grp.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }

    // Layout inside .rsrc section:
    // 0: Root dir (16 + 2*8 = 32 bytes) -> RT_ICON (3), RT_GROUP_ICON (14)
    // 32: Type dir for RT_ICON (16 + count*8)
    //     Type dir for RT_GROUP_ICON (16 + 1*8 = 24)
    //     Lang dirs: (count + 1) * (16 + 8)
    //     Data entries: (count + 1) * 16 bytes (each has 1 IMAGE_REL_AMD64_ADDR32NB relocation at +0!)
    //     Raw payloads (4-byte aligned)
    let root_off = 0usize;
    let type_icon_off = 32usize;
    let type_grp_off = type_icon_off + 16 + count * 8;
    let lang_base_off = type_grp_off + 24;
    let lang_stride = 24usize;
    let data_base_off = lang_base_off + (count + 1) * lang_stride;
    let data_stride = 16usize;
    let payload_base_off = data_base_off + (count + 1) * data_stride;

    let mut rsrc = vec![0u8; payload_base_off];
    let mut payload_offsets = Vec::with_capacity(count + 1);
    for e in &entries {
        while !rsrc.len().is_multiple_of(4) {
            rsrc.push(0);
        }
        payload_offsets.push((rsrc.len(), e.size));
        rsrc.extend_from_slice(&ico[e.offset..e.offset + e.size]);
    }
    while !rsrc.len().is_multiple_of(4) {
        rsrc.push(0);
    }
    payload_offsets.push((rsrc.len(), grp.len()));
    rsrc.extend_from_slice(&grp);
    while !rsrc.len().is_multiple_of(4) {
        rsrc.push(0);
    }

    let write_dir_hdr = |buf: &mut [u8], off: usize, id_entries: u16| {
        buf[off + 14..off + 16].copy_from_slice(&id_entries.to_le_bytes());
    };
    let write_dir_entry = |buf: &mut [u8], off: usize, id: u32, child_off: u32, is_dir: bool| {
        buf[off..off + 4].copy_from_slice(&id.to_le_bytes());
        let val = if is_dir {
            child_off | 0x8000_0000
        } else {
            child_off
        };
        buf[off + 4..off + 8].copy_from_slice(&val.to_le_bytes());
    };

    // Root dir: 2 entries (ID 3 = RT_ICON, ID 14 = RT_GROUP_ICON)
    write_dir_hdr(&mut rsrc, root_off, 2);
    write_dir_entry(&mut rsrc, root_off + 16, 3, type_icon_off as u32, true);
    write_dir_entry(&mut rsrc, root_off + 24, 14, type_grp_off as u32, true);

    // RT_ICON name/id dir
    write_dir_hdr(&mut rsrc, type_icon_off, count as u16);
    for i in 0..count {
        let lang_off = lang_base_off + i * lang_stride;
        write_dir_entry(
            &mut rsrc,
            type_icon_off + 16 + i * 8,
            (i + 1) as u32,
            lang_off as u32,
            true,
        );
        write_dir_hdr(&mut rsrc, lang_off, 1);
        let data_off = data_base_off + i * data_stride;
        write_dir_entry(&mut rsrc, lang_off + 16, 1033, data_off as u32, false);
    }

    // RT_GROUP_ICON name/id dir (ID 1)
    write_dir_hdr(&mut rsrc, type_grp_off, 1);
    let grp_lang_off = lang_base_off + count * lang_stride;
    write_dir_entry(&mut rsrc, type_grp_off + 16, 1, grp_lang_off as u32, true);
    write_dir_hdr(&mut rsrc, grp_lang_off, 1);
    let grp_data_off = data_base_off + count * data_stride;
    write_dir_entry(
        &mut rsrc,
        grp_lang_off + 16,
        1033,
        grp_data_off as u32,
        false,
    );

    // Data entries + relocations (IMAGE_REL_AMD64_ADDR32NB = 3 referencing symbol 0 = .rsrc)
    let mut relocs = Vec::with_capacity((count + 1) * 10);
    for (i, &(p_off, p_len)) in payload_offsets.iter().enumerate() {
        let d_off = data_base_off + i * data_stride;
        rsrc[d_off..d_off + 4].copy_from_slice(&(p_off as u32).to_le_bytes());
        rsrc[d_off + 4..d_off + 8].copy_from_slice(&(p_len as u32).to_le_bytes());
        // Reloc: VirtualAddress (4) + SymbolTableIndex (4) + Type (2)
        relocs.extend_from_slice(&(d_off as u32).to_le_bytes());
        relocs.extend_from_slice(&0u32.to_le_bytes());
        relocs.extend_from_slice(&3u16.to_le_bytes());
    }

    // Assemble x64 COFF header (20 bytes) + 1 section header (40 bytes) + .rsrc data + relocs + 1 symbol (18 bytes) + string table (4 bytes)
    let raw_ptr = 20 + 40;
    let reloc_ptr = raw_ptr + rsrc.len();
    let sym_ptr = reloc_ptr + relocs.len();
    let mut obj = Vec::with_capacity(sym_ptr + 18 + 4);

    // COFF File Header
    obj.extend_from_slice(&0x8664u16.to_le_bytes()); // IMAGE_FILE_MACHINE_AMD64
    obj.extend_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    obj.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
    obj.extend_from_slice(&(sym_ptr as u32).to_le_bytes());
    obj.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSymbols
    obj.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
    obj.extend_from_slice(&0u16.to_le_bytes()); // Characteristics

    // Section Header ".rsrc"
    obj.extend_from_slice(b".rsrc\0\0\0");
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualSize
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress
    obj.extend_from_slice(&(rsrc.len() as u32).to_le_bytes());
    obj.extend_from_slice(&(raw_ptr as u32).to_le_bytes());
    obj.extend_from_slice(&(reloc_ptr as u32).to_le_bytes());
    obj.extend_from_slice(&0u32.to_le_bytes()); // PointerToLinenumbers
    obj.extend_from_slice(&((count + 1) as u16).to_le_bytes()); // NumberOfRelocations
    obj.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    obj.extend_from_slice(&0x4000_0040u32.to_le_bytes()); // CNT_INITIALIZED_DATA | MEM_READ

    obj.extend_from_slice(&rsrc);
    obj.extend_from_slice(&relocs);

    // Symbol 0: ".rsrc" section symbol
    obj.extend_from_slice(b".rsrc\0\0\0");
    obj.extend_from_slice(&0u32.to_le_bytes()); // Value
    obj.extend_from_slice(&1i16.to_le_bytes()); // SectionNumber 1
    obj.extend_from_slice(&0u16.to_le_bytes()); // Type
    obj.push(3); // IMAGE_SYM_CLASS_STATIC
    obj.push(0); // NumberOfAuxSymbols

    // Empty string table (size = 4)
    obj.extend_from_slice(&4u32.to_le_bytes());
    Some(obj)
}
