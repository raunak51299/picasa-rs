//! JPEG EXIF and ICC metadata helpers.
//!
//! Rotation is a session-only view transform; this module only reads
//! orientation tags and copies EXIF/ICC segments when saving.

use std::io::Cursor;

use anyhow::{Context, Result, bail};
#[cfg(test)]
use exif::experimental::Writer;
#[cfg(test)]
use exif::{Field, Value};
use exif::{In, Reader, Tag};

/// Read the primary orientation from a JPEG or TIFF container.
pub fn read_orientation(data: &[u8]) -> Result<u16> {
    let reader = Reader::new();
    let exif = reader
        .read_from_container(&mut Cursor::new(data))
        .context("could not read EXIF data")?;
    Ok(exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|value| value as u16)
        .filter(|value| (1..=8).contains(value))
        .unwrap_or(1))
}

/// Replace (or add) the EXIF APP1 segment of a JPEG, preserving all other
/// segments and the compressed scan data byte for byte.
#[cfg(test)]
pub fn rewrite_jpeg_orientation(data: &[u8], orientation: u16) -> Result<Vec<u8>> {
    let orientation = orientation.clamp(1, 8);
    let has_exif = exif_tiff_range(data).is_some();

    // Preferred path: overwrite the two orientation bytes in place so every
    // other EXIF byte (maker notes, thumbnail, GPS) stays untouched.
    if has_exif {
        let mut patched = data.to_vec();
        if patch_orientation_in_place(&mut patched, orientation) {
            return Ok(patched);
        }
    }

    // The EXIF block has no orientation tag to patch. Re-encoding it is only
    // safe when it can be parsed; otherwise refuse instead of wiping metadata.
    let fields = if has_exif {
        read_fields(data).context("the EXIF data could not be parsed; refusing to rewrite it")?
    } else {
        Vec::new()
    };

    let mut fields: Vec<Field> = fields
        .into_iter()
        .filter(|field| field.ifd_num == In::PRIMARY && field.tag != Tag::Orientation)
        .collect();
    fields.push(Field {
        tag: Tag::Orientation,
        ifd_num: In::PRIMARY,
        value: Value::Short(vec![orientation.clamp(1, 8)]),
    });

    let mut writer = Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut tiff = Cursor::new(Vec::new());
    writer
        .write(&mut tiff, false)
        .context("could not encode EXIF data")?;

    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff.into_inner());
    if app1.len() + 2 > u16::MAX as usize {
        bail!("EXIF data is too large for a JPEG segment");
    }

    insert_exif_segment(data, &app1)
}

/// Return the raw EXIF APP1 payload (starting with `Exif\0\0`) from a JPEG.
/// Copying the segment byte for byte keeps maker notes and other fields that
/// this crate does not understand.
pub fn exif_app1(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut index = 2;
    while index + 1 < data.len() {
        if data[index] != 0xFF {
            return None;
        }
        let marker = data[index + 1];
        if marker == 0xDA {
            return None;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        if index + 3 >= data.len() {
            return None;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        if length < 2 || index + 2 + length > data.len() {
            return None;
        }
        let segment = &data[index..index + 2 + length];
        if marker == 0xE1 && segment.len() >= 10 && &segment[4..10] == b"Exif\0\0" {
            return Some(segment[4..].to_vec());
        }
        index += 2 + length;
    }
    None
}

/// Byte range of the TIFF structure inside the first EXIF APP1 segment.
#[cfg(test)]
fn exif_tiff_range(data: &[u8]) -> Option<std::ops::Range<usize>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 3 < data.len() {
        if data[index] != 0xFF {
            return None;
        }
        let marker = data[index + 1];
        if marker == 0xDA {
            return None;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        if length < 2 || index + 2 + length > data.len() {
            return None;
        }
        if marker == 0xE1 && length >= 8 && &data[index + 4..index + 10] == b"Exif\0\0" {
            return Some(index + 10..index + 2 + length);
        }
        index += 2 + length;
    }
    None
}

/// Overwrite the primary-IFD orientation value. Returns false when the tag is
/// absent or the structure is not a well-formed single SHORT value.
#[cfg(test)]
fn patch_orientation_in_place(data: &mut [u8], orientation: u16) -> bool {
    let Some(range) = exif_tiff_range(data) else {
        return false;
    };
    let tiff = &mut data[range];
    if tiff.len() < 8 {
        return false;
    }
    let big_endian = match &tiff[..2] {
        b"MM" => true,
        b"II" => false,
        _ => return false,
    };
    let read_u16 = |bytes: &[u8]| {
        let pair = [bytes[0], bytes[1]];
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    };
    let read_u32 = |bytes: &[u8]| {
        let quad = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if big_endian {
            u32::from_be_bytes(quad)
        } else {
            u32::from_le_bytes(quad)
        }
    };

    let ifd = read_u32(&tiff[4..8]) as usize;
    if ifd.checked_add(2).is_none_or(|end| end > tiff.len()) {
        return false;
    }
    let count = read_u16(&tiff[ifd..ifd + 2]) as usize;
    for entry in 0..count {
        let start = ifd + 2 + entry * 12;
        if start + 12 > tiff.len() {
            return false;
        }
        if read_u16(&tiff[start..start + 2]) != 0x0112 {
            continue;
        }
        let kind = read_u16(&tiff[start + 2..start + 4]);
        let values = read_u32(&tiff[start + 4..start + 8]);
        if kind != 3 || values != 1 {
            return false;
        }
        let encoded = if big_endian {
            orientation.to_be_bytes()
        } else {
            orientation.to_le_bytes()
        };
        tiff[start + 8..start + 10].copy_from_slice(&encoded);
        return true;
    }
    false
}

/// Raw ICC profile APP2 segments (marker and length included) from a JPEG.
pub fn icc_segments(data: &[u8]) -> Vec<Vec<u8>> {
    let mut segments = Vec::new();
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return segments;
    }
    let mut index = 2;
    while index + 3 < data.len() {
        if data[index] != 0xFF {
            break;
        }
        let marker = data[index + 1];
        if marker == 0xDA {
            break;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        if length < 2 || index + 2 + length > data.len() {
            break;
        }
        let segment = &data[index..index + 2 + length];
        if marker == 0xE2 && segment.len() >= 16 && &segment[4..16] == b"ICC_PROFILE\0" {
            segments.push(segment.to_vec());
        }
        index += 2 + length;
    }
    segments
}

/// Insert whole marker segments directly after the SOI marker.
pub fn insert_segments_after_soi(data: &[u8], segments: &[Vec<u8>]) -> Result<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        bail!("not a JPEG file");
    }
    let mut output = Vec::with_capacity(data.len() + segments.iter().map(Vec::len).sum::<usize>());
    output.extend_from_slice(&data[..2]);
    for segment in segments {
        output.extend_from_slice(segment);
    }
    output.extend_from_slice(&data[2..]);
    Ok(output)
}

#[cfg(test)]
fn read_fields(data: &[u8]) -> Result<Vec<Field>> {
    let reader = Reader::new();
    let exif = reader
        .read_from_container(&mut Cursor::new(data))
        .context("could not read EXIF data")?;
    Ok(exif.fields().cloned().collect())
}

/// Insert `app1` as the only EXIF APP1 segment of a JPEG, replacing any
/// existing EXIF payload and preserving every other segment.
pub fn insert_exif_segment(data: &[u8], app1: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        bail!("not a JPEG file");
    }
    if app1.len() + 2 > u16::MAX as usize {
        bail!("EXIF data is too large for a JPEG segment");
    }

    let mut output = Vec::with_capacity(data.len() + app1.len() + 4);
    output.extend_from_slice(&data[..2]);
    output.extend_from_slice(&[0xFF, 0xE1]);
    output.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    output.extend_from_slice(app1);

    let mut index = 2;
    while index + 1 < data.len() {
        if data[index] != 0xFF {
            bail!("malformed JPEG marker at byte {index}");
        }
        let marker = data[index + 1];

        if marker == 0xDA {
            output.extend_from_slice(&data[index..]);
            return Ok(output);
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            output.extend_from_slice(&data[index..index + 2]);
            index += 2;
            continue;
        }
        if index + 3 >= data.len() {
            bail!("truncated JPEG segment");
        }

        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        if length < 2 || index + 2 + length > data.len() {
            bail!("invalid JPEG segment length");
        }
        let segment = &data[index..index + 2 + length];
        let is_exif = marker == 0xE1 && segment.len() >= 10 && &segment[4..10] == b"Exif\0\0";
        if !is_exif {
            output.extend_from_slice(segment);
        }
        index += 2 + length;
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_exif_app1(orientation: u16, description: &str) -> Vec<u8> {
        let orientation = Field {
            tag: Tag::Orientation,
            ifd_num: In::PRIMARY,
            value: Value::Short(vec![orientation]),
        };
        let description = Field {
            tag: Tag::ImageDescription,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![description.as_bytes().to_vec()]),
        };
        let mut writer = Writer::new();
        writer.push_field(&orientation);
        writer.push_field(&description);
        let mut tiff = Cursor::new(Vec::new());
        writer.write(&mut tiff, false).unwrap();

        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff.into_inner());
        app1
    }

    fn jpeg_with_exif(app1: &[u8]) -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(app1);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x00, 0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn rewriting_keeps_other_fields_and_replaces_orientation() {
        let original = jpeg_with_exif(&make_exif_app1(1, "keep me"));
        let rewritten = rewrite_jpeg_orientation(&original, 8).unwrap();

        assert_eq!(read_orientation(&rewritten).unwrap(), 8);
        let reader = Reader::new();
        let exif = reader
            .read_from_container(&mut Cursor::new(&rewritten))
            .unwrap();
        let description = exif.get_field(Tag::ImageDescription, In::PRIMARY).unwrap();
        assert!(
            format!("{}", description.value.display_as(Tag::ImageDescription)).contains("keep me")
        );

        // Exactly one Exif APP1 segment, and the scan data is untouched.
        let app1_count = rewritten
            .windows(6)
            .filter(|window| *window == b"Exif\0\0")
            .count();
        assert_eq!(app1_count, 1);
        assert!(rewritten.ends_with(&[0x00, 0xFF, 0xD9]));
    }

    #[test]
    fn rotation_patches_in_place_and_keeps_every_other_byte() {
        let original = jpeg_with_exif(&make_exif_app1(1, "keep me"));
        let rewritten = rewrite_jpeg_orientation(&original, 6).unwrap();
        assert_eq!(rewritten.len(), original.len());
        assert_eq!(read_orientation(&rewritten).unwrap(), 6);
        let changed = original
            .iter()
            .zip(&rewritten)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 1);
    }

    #[test]
    fn unparseable_exif_is_never_wiped() {
        let mut jpeg = jpeg_with_exif(&make_exif_app1(1, "keep me"));
        // Break the TIFF byte-order mark so neither path can understand it.
        jpeg[12] = 0;
        jpeg[13] = 0;
        assert!(rewrite_jpeg_orientation(&jpeg, 6).is_err());
    }

    #[test]
    fn raw_exif_segment_can_be_copied_between_jpegs() {
        let app1 = make_exif_app1(1, "copy me");
        let original = jpeg_with_exif(&app1);
        let extracted = exif_app1(&original).unwrap();
        assert_eq!(extracted, app1);

        let mut encoded = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x00, 0xFF, 0xD9];
        encoded = insert_exif_segment(&encoded, &extracted).unwrap();
        assert_eq!(read_orientation(&encoded).unwrap(), 1);
        assert!(encoded.ends_with(&[0x00, 0xFF, 0xD9]));
    }
}
