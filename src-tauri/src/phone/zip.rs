// A zip unpacked: go-ios's release (goios_setup.rs). This is Wakuwaku's
// lhm_setup::zip (src-tauri/src/lhm_setup.rs, "The zip"), copied whole so this
// project builds on its own; merged into Wakuwaku, this file goes and
// goios_setup.rs calls crate::lhm_setup::zip::extract instead.
//
// Copyright (c) 2026 Pengjie Zhou, MIT License
// (https://github.com/zhoupengjie/Wakuwaku/blob/main/LICENSE).
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 5000;
const MAX_FILE: u64 = 256 << 20;
const MAX_ALL: u64 = 512 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    method: u16,
    crc: u32,
    packed: u64,
    pub size: u64,
    header: u64,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

// Its entries, from its central directory (found from the end: the
// end-of-directory record and its comment); no ZIP64 (LHM's is a few
// MB), no encryption, names in UTF-8 or ASCII.
pub fn entries(b: &[u8]) -> Result<Vec<Entry>, String> {
    let from = b.len().saturating_sub(22 + 65535);
    let end = (from..b.len().saturating_sub(21)).rev().find(|&i| u32_at(b, i) == Some(0x0605_4b50)).ok_or("no zip directory")?;
    let count = usize::from(u16_at(b, end + 10).ok_or("cut short")?);
    let at = u32_at(b, end + 16).ok_or("cut short")? as usize;
    if count == 0xFFFF || at == 0xFFFF_FFFF {
        return Err("ZIP64".into());
    }
    if count > MAX_ENTRIES {
        return Err(format!("{count} entries"));
    }
    let mut out = Vec::with_capacity(count);
    let mut p = at;
    let mut all = 0u64;
    for _ in 0..count {
        if u32_at(b, p) != Some(0x0201_4b50) {
            return Err("bad directory entry".into());
        }
        let flags = u16_at(b, p + 8).ok_or("cut short")?;
        let method = u16_at(b, p + 10).ok_or("cut short")?;
        let crc = u32_at(b, p + 16).ok_or("cut short")?;
        let packed = u32_at(b, p + 20).ok_or("cut short")?;
        let size = u32_at(b, p + 24).ok_or("cut short")?;
        let (n, e, c) = (usize::from(u16_at(b, p + 28).ok_or("cut short")?), usize::from(u16_at(b, p + 30).ok_or("cut short")?), usize::from(u16_at(b, p + 32).ok_or("cut short")?));
        let header = u32_at(b, p + 42).ok_or("cut short")?;
        let raw = b.get(p + 46..p + 46 + n).ok_or("cut short")?;
        if flags & 1 != 0 {
            return Err("encrypted".into());
        }
        if [packed, size, header].contains(&0xFFFF_FFFF) {
            return Err("ZIP64".into());
        }
        // Bit 11: UTF-8; otherwise code page 437, taken only when ASCII.
        let name = if flags & 0x800 != 0 || raw.is_ascii() { String::from_utf8(raw.to_vec()).map_err(|_| "a name not in UTF-8")? } else { return Err("a name not in ASCII nor UTF-8".into()) };
        all += u64::from(size);
        if u64::from(size) > MAX_FILE || all > MAX_ALL {
            return Err("too big unpacked".into());
        }
        out.push(Entry { name, method, crc, packed: u64::from(packed), size: u64::from(size), header: u64::from(header) });
        p += 46 + n + e + c;
    }
    Ok(out)
}

// A name as a path within the folder: its parts, none of them empty, "."
// or "..", none with a drive, a stream (:), a letter Windows refuses in
// a name, a device's name (CON, NUL, COM1…), nor ending in a dot or a
// space; None for any other. A folder's (ending in /) has its parts too.
pub fn safe(name: &str) -> Option<Vec<String>> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = name.trim_end_matches('/').split('/').collect();
    let device = |p: &str| {
        let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit())
    };
    let ok = |p: &&str| {
        !p.is_empty() && *p != "." && *p != ".." && !p.ends_with('.') && !p.ends_with(' ') && !device(p) && !p.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
    };
    (parts.iter().all(ok)).then(|| parts.iter().map(|p| p.to_string()).collect())
}

// One entry's bytes, inflated where deflated, checked by its size and CRC.
fn bytes_of(b: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
    let h = e.header as usize;
    if u32_at(b, h) != Some(0x0403_4b50) {
        return Err(format!("{}: no local header", e.name));
    }
    let (n, x) = (usize::from(u16_at(b, h + 26).ok_or("cut short")?), usize::from(u16_at(b, h + 28).ok_or("cut short")?));
    let at = h + 30 + n + x;
    let packed = b.get(at..at + e.packed as usize).ok_or_else(|| format!("{}: cut short", e.name))?;
    let data = match e.method {
        0 => packed.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(e.size as usize);
            flate2::read::DeflateDecoder::new(packed).take(e.size + 1).read_to_end(&mut out).map_err(|err| format!("{}: {err}", e.name))?;
            out
        }
        m => return Err(format!("{}: method {m}", e.name)),
    };
    if data.len() as u64 != e.size {
        return Err(format!("{}: {} bytes, not {}", e.name, data.len(), e.size));
    }
    if crc32fast::hash(&data) != e.crc {
        return Err(format!("{}: CRC", e.name));
    }
    Ok(data)
}

// The whole zip unpacked into `dir` (made), every name safe or none of
// it; one folder that holds everything taken away, so its files land in
// `dir` itself. The files written.
pub fn extract(b: &[u8], dir: &Path) -> Result<usize, String> {
    let entries = entries(b)?;
    let mut named = Vec::with_capacity(entries.len());
    for e in &entries {
        let parts = safe(&e.name).ok_or_else(|| format!("a name not allowed: {:?}", e.name))?;
        named.push((e, parts, e.name.ends_with('/') || e.name.ends_with('\\')));
    }
    // One top folder holding all: dropped.
    let top = named.first().map(|(_, p, _)| p[0].clone());
    let strip = usize::from(top.is_some_and(|top| named.iter().all(|(_, p, folder)| p[0] == top && (p.len() > 1 || *folder))));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut written = 0;
    for (e, parts, folder) in named {
        let rel: PathBuf = parts.iter().skip(strip).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let to = dir.join(&rel);
        if folder {
            std::fs::create_dir_all(&to).map_err(|err| format!("{}: {err}", to.display()))?;
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        std::fs::write(&to, bytes_of(b, e)?).map_err(|err| format!("{}: {err}", to.display()))?;
        written += 1;
    }
    Ok(written)
}
