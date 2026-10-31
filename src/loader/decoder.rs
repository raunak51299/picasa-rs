//! Multi-threaded image and EXIF reader.
//!
//! Full-size images keep their raw pixel orientation and are rotated on the GPU
//! with a UV matrix. Thumbnails are oriented on the CPU so egui can draw them
//! directly.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

const THUMBNAIL_MAX_EDGE: u32 = 128;

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "webp", "tif", "tiff", "bmp", "gif",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
        .unwrap_or(false)
}

#[derive(Debug)]
pub struct FullImage {
    pub path: PathBuf,
    pub index: usize,
    pub rgba: Vec<u8>,
    /// Dimensions of the decoded texture after any GPU-limit downscale.
    pub width: u32,
    pub height: u32,
    /// EXIF orientation, 1..=8.
    pub orientation: u8,
}

impl FullImage {
    /// Dimensions after applying EXIF orientation (before view rotation).
    pub fn display_size(&self) -> (u32, u32) {
        if self.orientation >= 5 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// UV matrix rows and offset for the photo shader.
    pub fn uv_transform(&self) -> ([f32; 2], [f32; 2], [f32; 2]) {
        match self.orientation {
            2 => ([-1.0, 0.0], [0.0, 1.0], [1.0, 0.0]),
            3 => ([-1.0, 0.0], [0.0, -1.0], [1.0, 1.0]),
            4 => ([1.0, 0.0], [0.0, -1.0], [0.0, 1.0]),
            5 => ([0.0, 1.0], [1.0, 0.0], [0.0, 0.0]),
            6 => ([0.0, 1.0], [-1.0, 0.0], [0.0, 1.0]),
            7 => ([0.0, -1.0], [-1.0, 0.0], [1.0, 1.0]),
            8 => ([0.0, -1.0], [1.0, 0.0], [1.0, 0.0]),
            _ => ([1.0, 0.0], [0.0, 1.0], [0.0, 0.0]),
        }
    }
}

#[derive(Debug)]
pub struct Thumbnail {
    pub path: PathBuf,
    pub index: usize,
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn decode_full(path: &Path, index: usize, max_texture_dim: u32) -> Result<FullImage> {
    let mut rgba = decode_rgba(path)?;
    let orientation = read_orientation(path).unwrap_or(1);

    let limit = max_texture_dim.max(1);
    if rgba.width() > limit || rgba.height() > limit {
        let scale = (limit as f32 / rgba.width().max(rgba.height()) as f32).min(1.0);
        let width = ((rgba.width() as f32 * scale).round() as u32).max(1);
        let height = ((rgba.height() as f32 * scale).round() as u32).max(1);
        log::info!(
            "downscaling {} from {}x{} to {}x{} for the GPU texture limit",
            path.display(),
            rgba.width(),
            rgba.height(),
            width,
            height
        );
        rgba = image::imageops::resize(&rgba, width, height, FilterType::Lanczos3);
    }

    Ok(FullImage {
        path: path.to_path_buf(),
        index,
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        orientation,
    })
}

pub fn decode_thumbnail(path: &Path, index: usize) -> Result<Thumbnail> {
    let rgba = decode_rgba(path)?;
    let orientation = read_orientation(path).unwrap_or(1);
    let oriented = orient_rgba(&rgba, orientation);

    let scale =
        (THUMBNAIL_MAX_EDGE as f32 / oriented.width().max(oriented.height()) as f32).min(1.0);
    let width = ((oriented.width() as f32 * scale).round() as u32).max(1);
    let height = ((oriented.height() as f32 * scale).round() as u32).max(1);
    let thumb = if scale < 1.0 {
        image::imageops::thumbnail(&oriented, width, height)
    } else {
        oriented
    };

    Ok(Thumbnail {
        path: path.to_path_buf(),
        index,
        width: thumb.width(),
        height: thumb.height(),
        rgba: thumb.into_raw(),
    })
}

fn decode_rgba(path: &Path) -> Result<RgbaImage> {
    let reader = image::ImageReader::open(path)
        .with_context(|| format!("could not open {}", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("could not identify the format of {}", path.display()))?;

    let dynamic: DynamicImage = reader
        .decode()
        .with_context(|| format!("could not decode {}", path.display()))?;

    Ok(dynamic.to_rgba8())
}

fn read_orientation(path: &Path) -> Option<u8> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let orientation = field.value.get_uint(0)?;
    if (1..=8).contains(&orientation) {
        Some(orientation as u8)
    } else {
        None
    }
}

/// Rotate/flip an RGBA image according to EXIF orientation.
pub fn orient_rgba(source: &RgbaImage, orientation: u8) -> RgbaImage {
    let width = source.width();
    let height = source.height();

    match orientation {
        2 => image::imageops::flip_horizontal(source),
        3 => image::imageops::rotate180(source),
        4 => image::imageops::flip_vertical(source),
        5..=8 => {
            let mut destination = RgbaImage::new(height, width);
            for y in 0..height {
                for x in 0..width {
                    let (dx, dy) = match orientation {
                        // transpose
                        5 => (y, x),
                        // rotate 90 clockwise
                        6 => (height - 1 - y, x),
                        // transverse
                        7 => (height - 1 - y, width - 1 - x),
                        // rotate 270 clockwise (90 counter-clockwise)
                        _ => (y, width - 1 - x),
                    };
                    destination.put_pixel(dx, dy, *source.get_pixel(x, y));
                }
            }
            destination
        }
        _ => source.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::orient_rgba;
    use image::{Rgba, RgbaImage};

    fn sample() -> RgbaImage {
        // 2x3 image with distinct pixel values.
        let mut image = RgbaImage::new(2, 3);
        for y in 0..3 {
            for x in 0..2 {
                image.put_pixel(x, y, Rgba([x as u8, y as u8, 0, 255]));
            }
        }
        image
    }

    #[test]
    fn orientation_six_rotates_clockwise() {
        let source = sample();
        let rotated = orient_rgba(&source, 6);
        assert_eq!(rotated.dimensions(), (3, 2));
        // Source (0,0) lands at top-right of the rotated image.
        assert_eq!(rotated.get_pixel(2, 0), &Rgba([0, 0, 0, 255]));
        // Source (1,2) lands at bottom-left.
        assert_eq!(rotated.get_pixel(0, 1), &Rgba([1, 2, 0, 255]));
    }

    #[test]
    fn orientation_five_is_transpose() {
        let source = sample();
        let rotated = orient_rgba(&source, 5);
        assert_eq!(rotated.dimensions(), (3, 2));
        assert_eq!(rotated.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
        // Transpose maps source (0,1) to destination (1,0).
        assert_eq!(rotated.get_pixel(1, 0), &Rgba([0, 1, 0, 255]));
    }

    #[test]
    fn orientation_mirrors_and_180() {
        let source = sample();

        let mirrored = orient_rgba(&source, 2);
        assert_eq!(mirrored.dimensions(), (2, 3));
        assert_eq!(mirrored.get_pixel(0, 0), &Rgba([1, 0, 0, 255]));

        let upside_down = orient_rgba(&source, 3);
        assert_eq!(upside_down.dimensions(), (2, 3));
        assert_eq!(upside_down.get_pixel(0, 0), &Rgba([1, 2, 0, 255]));

        let flipped = orient_rgba(&source, 4);
        assert_eq!(flipped.dimensions(), (2, 3));
        assert_eq!(flipped.get_pixel(0, 0), &Rgba([0, 2, 0, 255]));
    }

    #[test]
    fn orientation_seven_and_eight() {
        let source = sample();

        let transverse = orient_rgba(&source, 7);
        assert_eq!(transverse.dimensions(), (3, 2));
        assert_eq!(transverse.get_pixel(0, 0), &Rgba([1, 2, 0, 255]));
        assert_eq!(transverse.get_pixel(2, 1), &Rgba([0, 0, 0, 255]));

        let counter_clockwise = orient_rgba(&source, 8);
        assert_eq!(counter_clockwise.dimensions(), (3, 2));
        assert_eq!(counter_clockwise.get_pixel(0, 1), &Rgba([0, 0, 0, 255]));
        assert_eq!(counter_clockwise.get_pixel(2, 0), &Rgba([1, 2, 0, 255]));
    }
}
