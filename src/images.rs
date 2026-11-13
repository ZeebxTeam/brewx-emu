//! Finds PNG and BMP headers inside arbitrary bytes and reads their dimensions.
//!
//! Resolution detection uses it without depending on the `.bar` layout: a game's splash image
//! usually has the size of the screen.

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// `(width, height)` of every PNG or BMP image found in `data`.
pub fn sniff_dimensions(data: &[u8]) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    for i in 0..data.len() {
        let rest = &data[i..];
        if rest.starts_with(PNG_SIGNATURE) {
            found.extend(png_dimensions(rest));
        } else if rest.starts_with(b"BM") {
            found.extend(bmp_dimensions(rest));
        }
    }
    found
}

fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // Signature, chunk length and "IHDR", then width and height in big-endian.
    if data.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(data.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(data.get(20..24)?.try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

fn bmp_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // "BM" shows up by chance in any binary: it only counts if the DIB header makes sense.
    let le_u32 = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let le_u16 = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let dib_size = le_u32(14)?;
    let (w, h, planes, bpp) = match dib_size {
        12 => (
            u32::from(le_u16(18)?),
            u32::from(le_u16(20)?),
            le_u16(22)?,
            le_u16(24)?,
        ),
        40 | 52 | 56 | 108 | 124 => {
            let w = le_u32(18)? as i32;
            // A negative height is a top-down bitmap; the size is the same.
            let h = (le_u32(22)? as i32).unsigned_abs();
            if w <= 0 {
                return None;
            }
            (w as u32, h, le_u16(26)?, le_u16(28)?)
        }
        _ => return None,
    };
    let valid = planes == 1 && matches!(bpp, 1 | 2 | 4 | 8 | 16 | 24 | 32) && h > 0;
    valid.then_some((w, h))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend(13u32.to_be_bytes());
        v.extend(b"IHDR");
        v.extend(w.to_be_bytes());
        v.extend(h.to_be_bytes());
        v.extend([8, 2, 0, 0, 0]);
        v
    }

    pub fn bmp_header(w: i32, h: i32) -> Vec<u8> {
        let mut v = b"BM".to_vec();
        v.extend([0u8; 12]);
        v.extend(40u32.to_le_bytes());
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v
    }

    #[test]
    fn finds_png_and_bmp_inside_noise() {
        let mut blob = vec![0xAA; 30];
        blob.extend(png_header(176, 220));
        blob.extend(b"BM garbage that is not a bitmap");
        blob.extend(bmp_header(128, -160));
        assert_eq!(sniff_dimensions(&blob), vec![(176, 220), (128, 160)]);
    }

    #[test]
    fn rejects_bad_bmp() {
        let mut bad = bmp_header(128, 160);
        bad[26] = 3; // planes != 1
        assert!(sniff_dimensions(&bad).is_empty());
    }
}
