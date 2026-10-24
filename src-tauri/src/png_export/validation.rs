use super::{Result, MAX_PNG};

pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}

pub(super) fn validate_png(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_PNG || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("unknown");
    }
    let mut offset = 8_usize;
    let mut header = false;
    let mut data = false;
    let mut srgb = false;
    while offset + 12 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(size + 12)
            .filter(|end| *end <= bytes.len())
            .ok_or("unknown")?;
        let kind = &bytes[offset + 4..offset + 8];
        if crc32(&bytes[offset + 4..end - 4])
            != u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap())
        {
            return Err("unknown");
        }
        if !header && kind != b"IHDR" {
            return Err("unknown");
        }
        match kind {
            b"IHDR" => {
                if header || size != 13 {
                    return Err("unknown");
                }
                let width = u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap());
                let height =
                    u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap());
                if width == 0 || height == 0 || width > 4000 || height > 4000 {
                    return Err("unknown");
                }
                header = true;
            }
            b"sRGB" => {
                if srgb || data || size != 1 || bytes[offset + 8] > 3 {
                    return Err("unknown");
                }
                srgb = true;
            }
            b"IDAT" => data = true,
            b"iCCP" => return Err("unknown"),
            b"IEND" => {
                return if header && data && srgb && size == 0 && end == bytes.len() {
                    Ok(())
                } else {
                    Err("unknown")
                }
            }
            _ => (),
        }
        offset = end;
    }
    Err("unknown")
}
