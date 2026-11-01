//! Write adjusted pixels back to disk, either in place or as a copy.
//!
//! JPEG files keep their original EXIF APP1 segment (including the orientation
//! tag) and only the compressed pixels are re-encoded. Formats without EXIF
//! support bake the orientation into the pixels because the metadata is
//! dropped by the encoder.

use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::adjust::{Adjustments, apply_to_rgba};
use crate::loader::decoder;
use crate::rotate;

const JPEG_QUALITY: u8 = 92;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveMode {
    /// Replace the original file.
    Overwrite,
    /// Write a sibling named `<stem> (fixed).<ext>`.
    Copy,
}

#[derive(Debug)]
pub struct SavedFile {
    pub path: PathBuf,
    pub is_copy: bool,
}

/// Apply `adjustments` to `path` and save the result.
pub fn save_adjusted(path: &Path, adjustments: Adjustments, mode: SaveMode) -> Result<SavedFile> {
    let format = format_for(path)
        .with_context(|| format!("saving color fixes is not supported for {}", path.display()))?;
    if format == ImageFormat::Gif {
        bail!("saving color fixes for GIF files is not supported yet");
    }
    if adjustments.is_neutral() {
        bail!("there are no adjustments to save");
    }

    let original = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let decoded = image::load_from_memory_with_format(&original, format)
        .with_context(|| format!("could not decode {}", path.display()))?;
    let mut rgba = decoded.to_rgba8();

    let encoded = if format == ImageFormat::Jpeg {
        // JPEG pixels stay in their raw orientation; the EXIF orientation tag
        // is copied below and every decoder applies it on load.
        apply_to_rgba(rgba.as_mut(), adjustments);
        let encoded = encode_jpeg(&rgba)?;
        match rotate::exif_app1(&original) {
            Some(app1) => rotate::insert_exif_segment(&encoded, &app1)?,
            None => encoded,
        }
    } else {
        // The encoder drops metadata, so bake the orientation into the pixels.
        let orientation = rotate::read_orientation(&original).unwrap_or(1) as u8;
        if orientation != 1 {
            rgba = decoder::orient_rgba(&rgba, orientation);
        }
        apply_to_rgba(rgba.as_mut(), adjustments);
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut encoded, format)
            .with_context(|| format!("could not encode {}", path.display()))?;
        encoded.into_inner()
    };

    let (destination, is_copy) = match mode {
        SaveMode::Overwrite => (path.to_path_buf(), false),
        SaveMode::Copy => (unique_copy_path(path), true),
    };

    write_atomically(&destination, &encoded)?;
    Ok(SavedFile {
        path: destination,
        is_copy,
    })
}

/// `<stem> (fixed).<ext>`, then `<stem> (fixed 2).<ext>` and so on. The first
/// name is returned when `attempt` is 1.
pub fn copy_name(path: &Path, attempt: usize) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let suffix = if attempt <= 1 {
        "(fixed)".to_string()
    } else {
        format!("(fixed {attempt})")
    };
    let name = if extension.is_empty() {
        format!("{stem} {suffix}")
    } else {
        format!("{stem} {suffix}.{extension}")
    };
    path.with_file_name(name)
}

fn unique_copy_path(path: &Path) -> PathBuf {
    for attempt in 1..=10_000 {
        let candidate = copy_name(path, attempt);
        if !candidate.exists() {
            return candidate;
        }
    }
    // Extremely unlikely; the caller will report the write error.
    copy_name(path, 10_001)
}

fn format_for(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" | "jfif" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::WebP),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "bmp" => Some(ImageFormat::Bmp),
        "gif" => Some(ImageFormat::Gif),
        _ => None,
    }
}

fn encode_jpeg(rgba: &RgbaImage) -> Result<Vec<u8>> {
    let rgb = DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
    let mut encoded = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .encode_image(&DynamicImage::ImageRgb8(rgb))
        .context("could not encode JPEG data")?;
    Ok(encoded.into_inner())
}

fn write_atomically(destination: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary_name = destination.file_name().unwrap_or_default().to_os_string();
    temporary_name.push(".picasa-rs.tmp");
    let temporary = destination.with_file_name(temporary_name);

    let write_result = (|| -> Result<()> {
        let mut file = File::create(&temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("could not write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("could not flush {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, destination)
            .with_context(|| format!("could not replace {}", destination.display()))
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("picasa-rs-save-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn sample_jpeg(orientation: u16) -> Vec<u8> {
        let image = RgbaImage::from_pixel(2, 2, Rgba([128, 128, 128, 255]));
        let mut cursor = Cursor::new(Vec::new());
        JpegEncoder::new_with_quality(&mut cursor, 100)
            .encode_image(&DynamicImage::ImageRgba8(image))
            .unwrap();
        rotate::rewrite_jpeg_orientation(&cursor.into_inner(), orientation).unwrap()
    }

    #[test]
    fn copy_names_are_numbered_after_the_first() {
        let source = Path::new(r"C:\Photos\holiday.jpg");
        assert_eq!(
            copy_name(source, 1),
            PathBuf::from(r"C:\Photos\holiday (fixed).jpg")
        );
        assert_eq!(
            copy_name(source, 2),
            PathBuf::from(r"C:\Photos\holiday (fixed 2).jpg")
        );
        assert_eq!(
            copy_name(Path::new("no-extension"), 1),
            PathBuf::from("no-extension (fixed)")
        );
    }

    #[test]
    fn only_known_image_extensions_can_be_saved() {
        assert_eq!(format_for(Path::new("a.JPG")), Some(ImageFormat::Jpeg));
        assert_eq!(format_for(Path::new("a.jfif")), Some(ImageFormat::Jpeg));
        assert_eq!(format_for(Path::new("a.png")), Some(ImageFormat::Png));
        assert_eq!(format_for(Path::new("a.webp")), Some(ImageFormat::WebP));
        assert_eq!(format_for(Path::new("a.tiff")), Some(ImageFormat::Tiff));
        assert_eq!(format_for(Path::new("a.bmp")), Some(ImageFormat::Bmp));
        assert_eq!(format_for(Path::new("a.gif")), Some(ImageFormat::Gif));
        assert_eq!(format_for(Path::new("a.avif")), None);
    }

    #[test]
    fn neutral_adjustments_are_rejected_instead_of_rewriting_the_file() {
        let path = std::env::temp_dir().join("picasa-rs-neutral-save.png");
        let error = save_adjusted(&path, Adjustments::default(), SaveMode::Overwrite).unwrap_err();
        assert!(format!("{error:#}").contains("no adjustments"));
    }

    #[test]
    fn jpeg_overwrite_keeps_orientation_and_applies_pixels() {
        let directory = temp_dir("jpeg");
        let path = directory.join("sample.jpg");
        fs::write(&path, sample_jpeg(6)).unwrap();

        let saved = save_adjusted(&path, Adjustments::lighten(), SaveMode::Overwrite).unwrap();
        assert!(!saved.is_copy);
        assert_eq!(saved.path, path);

        let data = fs::read(&path).unwrap();
        assert_eq!(rotate::read_orientation(&data).unwrap(), 6);
        let decoded = image::load_from_memory_with_format(&data, ImageFormat::Jpeg)
            .unwrap()
            .to_rgba8();
        assert!(
            decoded.get_pixel(0, 0)[0] > 128,
            "expected a lighter pixel, got {:?}",
            decoded.get_pixel(0, 0)
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn png_copy_is_written_beside_the_original_without_touching_it() {
        let directory = temp_dir("png");
        let path = directory.join("sample.png");
        RgbaImage::from_pixel(3, 2, Rgba([64, 64, 64, 255]))
            .save(&path)
            .unwrap();
        let original = fs::read(&path).unwrap();

        let saved = save_adjusted(&path, Adjustments::warm(), SaveMode::Copy).unwrap();
        assert!(saved.is_copy);
        assert_eq!(
            saved.path.file_name().unwrap().to_string_lossy(),
            "sample (fixed).png"
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        let copy = image::open(&saved.path).unwrap().to_rgba8();
        assert_eq!(copy.dimensions(), (3, 2));
        let pixel = copy.get_pixel(0, 0);
        assert!(pixel[0] > pixel[2], "expected a warm pixel, got {pixel:?}");

        // A second copy does not overwrite the first one.
        let again = save_adjusted(&path, Adjustments::cool(), SaveMode::Copy).unwrap();
        assert_eq!(
            again.path.file_name().unwrap().to_string_lossy(),
            "sample (fixed 2).png"
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
