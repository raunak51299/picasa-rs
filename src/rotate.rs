//! Lossless 90-degree rotation for JPEG files.
//!
//! The compressed image data is never touched. The EXIF orientation tag is
//! rewritten, which every decoder (including this viewer) applies on load.
//! Other formats are reported as unsupported rather than re-encoded.

use std::fs::File;
use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use exif::experimental::Writer;
use exif::{Field, In, Reader, Tag, Value};

/// Orientation values 1..=8 after rotating the displayed image clockwise or
/// counter-clockwise by 90 degrees.
pub fn rotate_orientation(orientation: u16, clockwise: bool) -> u16 {
    let orientation = if (1..=8).contains(&orientation) {
        orientation
    } else {
        1
    };
    const CLOCKWISE: [u16; 8] = [6, 7, 8, 5, 2, 3, 4, 1];
    const COUNTER_CLOCKWISE: [u16; 8] = [8, 5, 6, 7, 4, 1, 2, 3];
    let table = if clockwise {
        CLOCKWISE
    } else {
        COUNTER_CLOCKWISE
    };
    table[(orientation - 1) as usize]
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
        })
}

/// Rotate the file's EXIF orientation and return the new orientation value.
pub fn rotate_file(path: &Path, clockwise: bool) -> Result<u16> {
    if !is_supported(path) {
        bail!("persistent rotation is only supported for JPEG files");
    }

    let data = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let current = read_orientation(&data).unwrap_or(1);
    let next = rotate_orientation(current, clockwise);
    let rewritten = rewrite_jpeg_orientation(&data, next)?;

    let temporary = path.with_extension("picasa-rs.tmp");
    {
        let mut file = File::create(&temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(&rewritten)
            .with_context(|| format!("could not write {}", temporary.display()))?;
    }
    std::fs::rename(&temporary, path)
        .with_context(|| format!("could not replace {}", path.display()))?;

    Ok(next)
}

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
pub fn rewrite_jpeg_orientation(data: &[u8], orientation: u16) -> Result<Vec<u8>> {
    let fields = read_fields(data).unwrap_or_default();

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

fn read_fields(data: &[u8]) -> Result<Vec<Field>> {
    let reader = Reader::new();
    let exif = reader
        .read_from_container(&mut Cursor::new(data))
        .context("could not read EXIF data")?;
    Ok(exif.fields().cloned().collect())
}

fn insert_exif_segment(data: &[u8], app1: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        bail!("not a JPEG file");
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

    #[test]
    fn orientation_composition_wraps_both_ways() {
        for orientation in 1..=8 {
            let clockwise = rotate_orientation(orientation, true);
            assert_eq!(rotate_orientation(clockwise, false), orientation);
        }
        assert_eq!(rotate_orientation(1, true), 6);
        assert_eq!(rotate_orientation(6, true), 3);
        assert_eq!(rotate_orientation(3, true), 8);
        assert_eq!(rotate_orientation(8, true), 1);
        assert_eq!(rotate_orientation(0, true), 6);
    }

    fn exif_app1(orientation: u16, description: &str) -> Vec<u8> {
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
        let original = jpeg_with_exif(&exif_app1(1, "keep me"));
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
}
