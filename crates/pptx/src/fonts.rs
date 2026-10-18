//! Embedded fonts (`p:embeddedFontLst`, `ppt/fonts/*`): turn the stored part into a plain
//! TrueType/OpenType file a browser or font loader can use.
//!
//! Three storage forms exist in the wild:
//! - a plain sfnt (TrueType/OpenType) file;
//! - an obfuscated font (ECMA-376 Part 2, §15.2.13 / the font-obfuscation algorithm used by
//!   `.odttf`): the first 32 bytes are XORed with a 16-byte key taken from the GUID in the part
//!   name, read right to left;
//! - Embedded OpenType (EOT, W3C submission 2008) — the form PowerPoint itself writes as
//!   `.fntdata`. EOT (optionally XOR-0x50 encrypted) is unwrapped, and MicroType Express
//!   compressed EOT is decompressed (`crate::mtx`).

/// Why an embedded font part could not be turned into a plain font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSkip {
    /// EOT with MicroType Express compression that failed to decode.
    MtxCompressed,
    /// Not a recognisable font.
    Unknown,
}

/// Is `b` a plain sfnt file? Returns its media type.
pub fn sfnt_type(b: &[u8]) -> Option<&'static str> {
    match b.get(..4)? {
        [0, 1, 0, 0] | b"true" | b"ttcf" => Some("font/ttf"),
        b"OTTO" => Some("font/otf"),
        _ => None,
    }
}

/// The 16-byte obfuscation key from a GUID such as `{6C4A1E2F-...}` found in `part`'s file name.
pub fn guid_key(part: &str) -> Option<[u8; 16]> {
    let stem = part.rsplit('/').next().unwrap_or(part);
    let stem = stem.split('.').next().unwrap_or(stem);
    let hex: Vec<u8> = stem.bytes().filter(|c| *c != b'{' && *c != b'}' && *c != b'-').collect();
    if hex.len() != 32 || !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let nib = |c: u8| -> u8 { (c as char).to_digit(16).and_then(|d| u8::try_from(d).ok()).unwrap_or(0) };
    let mut key = [0u8; 16];
    // Byte i of the key is hex pair (15 - i): the GUID string read right to left.
    for (i, k) in key.iter_mut().enumerate() {
        let j = (15 - i) * 2;
        let (hi, lo) = (hex.get(j).copied().unwrap_or(b'0'), hex.get(j + 1).copied().unwrap_or(b'0'));
        *k = (nib(hi) << 4) | nib(lo);
    }
    Some(key)
}

/// XOR the first 32 bytes of `data` with `key` (the operation is its own inverse).
pub fn xor_obfuscation(data: &mut [u8], key: &[u8; 16]) {
    for (i, b) in data.iter_mut().take(32).enumerate() {
        *b ^= key.get(i % 16).copied().unwrap_or(0);
    }
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

/// Unwrap an EOT container. `Ok(None)` when `b` is not EOT.
fn eot(b: &[u8]) -> Result<Option<Vec<u8>>, FontSkip> {
    let (Some(size), Some(data_size), Some(version), Some(flags)) = (u32le(b, 0), u32le(b, 4), u32le(b, 8), u32le(b, 12)) else {
        return Ok(None);
    };
    if !matches!(version, 0x0001_0000 | 0x0002_0001 | 0x0002_0002) || usize::try_from(size).ok() != Some(b.len()) {
        return Ok(None);
    }
    let data_size = usize::try_from(data_size).map_err(|_| FontSkip::Unknown)?;
    let start = b.len().checked_sub(data_size).ok_or(FontSkip::Unknown)?;
    let mut font = b.get(start..).ok_or(FontSkip::Unknown)?.to_vec();
    if flags & 0x1000_0000 != 0 {
        font.iter_mut().for_each(|x| *x ^= 0x50);
    }
    if flags & 0x4 != 0 {
        return crate::mtx::decode(&font).map(Some).map_err(|_| FontSkip::MtxCompressed);
    }
    Ok(Some(font))
}

/// Decode an embedded font part (`part` is its package name, `data` its bytes) into a plain
/// font and its media type.
pub fn decode(part: &str, data: &[u8]) -> Result<(Vec<u8>, &'static str), FontSkip> {
    if let Some(t) = sfnt_type(data) {
        return Ok((data.to_vec(), t));
    }
    if let Some(key) = guid_key(part) {
        let mut d = data.to_vec();
        xor_obfuscation(&mut d, &key);
        if let Some(t) = sfnt_type(&d) {
            return Ok((d, t));
        }
    }
    if let Some(font) = eot(data)?
        && let Some(t) = sfnt_type(&font)
    {
        return Ok((font, t));
    }
    Err(FontSkip::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic sfnt: header + zero tables, padded past 32 bytes.
    fn fake_ttf() -> Vec<u8> {
        let mut v = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        v.extend((0u8..60).map(|i| i.wrapping_mul(7)));
        v
    }

    #[test]
    fn obfuscated_font_round_trips() {
        let part = "ppt/fonts/{6C4A1E2F-0B3D-4A5C-9E8F-112233445566}.odttf";
        let key = guid_key(part).unwrap();
        // The key is the GUID's bytes read right to left.
        assert_eq!(key[0], 0x66);
        assert_eq!(key[15], 0x6C);
        let plain = fake_ttf();
        let mut obf = plain.clone();
        xor_obfuscation(&mut obf, &key);
        assert_ne!(obf, plain);
        assert_eq!(obf[32..], plain[32..]);
        let (out, ty) = decode(part, &obf).unwrap();
        assert_eq!(out, plain);
        assert_eq!(ty, "font/ttf");
    }

    #[test]
    fn eot_forms() {
        let font = fake_ttf();
        let eot_of = |flags: u32, payload: &[u8]| {
            let mut v = vec![0u8; 16];
            v.extend([0u8; 20]);
            v.extend_from_slice(payload);
            let n = u32::try_from(v.len()).unwrap();
            v[0..4].copy_from_slice(&n.to_le_bytes());
            v[4..8].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            v[8..12].copy_from_slice(&0x0002_0002u32.to_le_bytes());
            v[12..16].copy_from_slice(&flags.to_le_bytes());
            v
        };
        assert_eq!(decode("ppt/fonts/font1.fntdata", &eot_of(0, &font)).unwrap().0, font);
        let xored: Vec<u8> = font.iter().map(|b| b ^ 0x50).collect();
        assert_eq!(decode("ppt/fonts/font1.fntdata", &eot_of(0x1000_0000, &xored)).unwrap().0, font);
        assert_eq!(decode("ppt/fonts/font1.fntdata", &eot_of(4, &font)), Err(FontSkip::MtxCompressed));
        assert_eq!(decode("x", b"junk"), Err(FontSkip::Unknown));
        assert_eq!(decode("x", &[]), Err(FontSkip::Unknown));
    }
}
