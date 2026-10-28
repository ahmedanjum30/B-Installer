//! Adds the icon, the version, the manifest and the payload to the copied stub. Resources rather than data
//! appended to the file, so the setup can still be signed afterwards.

use std::path::Path;

use windows::{
    Win32::System::LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW},
    core::{HSTRING, PCWSTR},
};

const RT_ICON: PCWSTR = PCWSTR(3 as _);
const RT_RCDATA: PCWSTR = PCWSTR(10 as _);
const RT_GROUP_ICON: PCWSTR = PCWSTR(14 as _);
const RT_VERSION: PCWSTR = PCWSTR(16 as _);
/// How many images assets\bi.ico has (and so the stub).
const STUB_ICONS: usize = 8;

pub fn write(exe: &Path, app: &format::App, icon: Option<&[u8]>, manifest: &[u8], payload: &[u8]) -> Result<(), String> {
    let icons = icon.map(parse_ico).transpose()?;
    unsafe {
        let h = BeginUpdateResourceW(&HSTRING::from(exe), false).map_err(|e| format!("{}: {e}", exe.display()))?;
        let update = |kind: PCWSTR, name: PCWSTR, data: &[u8]| {
            UpdateResourceW(h, kind, name, 0, Some(data.as_ptr().cast()), data.len() as u32).map_err(|e| e.to_string())
        };
        let result = (|| {
            if let Some((images, group)) = &icons {
                // The app's images replace the stub's own icon (B-Installer's), and any of its
                // images beyond the app's count go, so none are left over.
                for (i, image) in images.iter().enumerate() {
                    update(RT_ICON, PCWSTR((i + 1) as *const u16), image)?;
                }
                for id in images.len() + 1..=STUB_ICONS {
                    let _ = UpdateResourceW(h, RT_ICON, PCWSTR(id as *const u16), 0, None, 0);
                }
                update(RT_GROUP_ICON, PCWSTR(1 as _), group)?;
            }
            update(RT_VERSION, PCWSTR(1 as _), &version_info(app))?;
            let manifest_name = HSTRING::from(format::MANIFEST_RESOURCE);
            let payload_name = HSTRING::from(format::PAYLOAD_RESOURCE);
            update(RT_RCDATA, PCWSTR(manifest_name.as_ptr()), manifest)?;
            update(RT_RCDATA, PCWSTR(payload_name.as_ptr()), payload)
        })();
        EndUpdateResourceW(h, result.is_err()).map_err(|e| format!("{}: {e}", exe.display()))?;
        result
    }
}

/// An .ico file as icon images (named 1, 2, ...) and the icon group pointing at them.
fn parse_ico(ico: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<u8>), String> {
    let u16_at = |i: usize| ico.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| ico.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let bad = || "the icon isn't a valid .ico file".to_string();
    if u16_at(2) != Some(1) {
        return Err(bad());
    }
    let count = u16_at(4).ok_or_else(bad)? as usize;
    let mut images = Vec::new();
    let mut group = ico[..6].to_vec();
    for i in 0..count {
        let entry = 6 + i * 16;
        let size = u32_at(entry + 8).ok_or_else(bad)? as usize;
        let offset = u32_at(entry + 12).ok_or_else(bad)? as usize;
        images.push(ico.get(offset..offset + size).ok_or_else(bad)?.to_vec());
        // The group's entries are the file's, with the image's resource name for the offset.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&(i as u16 + 1).to_le_bytes());
    }
    Ok((images, group))
}

/// The version block: what file Properties, Task Manager and the UAC prompt show, such as
/// "B-Rar Setup" rather than the file name.
fn version_info(app: &format::App) -> Vec<u8> {
    let mut parts: Vec<u32> = app.version.split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let ms = (parts[0] << 16) | (parts[1] & 0xFFFF);
    let ls = (parts[2] << 16) | (parts[3] & 0xFFFF);
    // VS_FIXEDFILEINFO: signature, struct version, file and product versions, flags mask, flags,
    // OS (VOS_NT_WINDOWS32), type (VFT_APP), subtype, date.
    let fixed: Vec<u8> = [0xFEEF04BD, 0x10000, ms, ls, ms, ls, 0x3F, 0, 0x40004, 1, 0, 0, 0]
        .iter()
        .flat_map(|v: &u32| v.to_le_bytes())
        .collect();
    let description = format!("{} Setup", app.name);
    let strings = [
        ("CompanyName", app.publisher.as_str()),
        ("FileDescription", description.as_str()),
        ("FileVersion", app.version.as_str()),
        ("ProductName", app.name.as_str()),
        ("ProductVersion", app.version.as_str()),
    ];
    let table = strings
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| node(k, Value::Text(v), &[]))
        .collect::<Vec<_>>();
    let translation = [0x0409u16, 1200].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    node(
        "VS_VERSION_INFO",
        Value::Binary(&fixed),
        &[
            node("StringFileInfo", Value::None, &[node("040904B0", Value::None, &table)]),
            node("VarFileInfo", Value::None, &[node("Translation", Value::Binary(&translation), &[])]),
        ],
    )
}

enum Value<'a> {
    None,
    Text(&'a str),
    Binary(&'a [u8]),
}

/// One block of a version resource: length, value length, type, key, value, then the children,
/// each part aligned to 4 bytes.
fn node(key: &str, value: Value, children: &[Vec<u8>]) -> Vec<u8> {
    let pad = |b: &mut Vec<u8>| b.resize(b.len().next_multiple_of(4), 0);
    let utf16 = |s: &str| s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<_>>();
    let (value_len, kind, bytes) = match value {
        Value::None => (0, 1, Vec::new()),
        Value::Text(s) => (s.encode_utf16().count() + 1, 1, utf16(s)),
        Value::Binary(b) => (b.len(), 0, b.to_vec()),
    };
    let mut b = vec![0, 0];
    b.extend_from_slice(&(value_len as u16).to_le_bytes());
    b.extend_from_slice(&(kind as u16).to_le_bytes());
    b.extend(utf16(key));
    pad(&mut b);
    b.extend(bytes);
    for child in children {
        pad(&mut b);
        b.extend_from_slice(child);
    }
    let len = b.len() as u16;
    b[..2].copy_from_slice(&len.to_le_bytes());
    b
}
