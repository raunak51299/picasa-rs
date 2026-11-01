//! Multi-threaded image and EXIF reader.
//!
//! Full-size images keep their raw pixel orientation and are rotated on the GPU
//! with a UV matrix. Thumbnails are oriented on the CPU so egui can draw them
//! directly.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};

const THUMBNAIL_MAX_EDGE: u32 = 128;
/// Animated files are decoded into full canvas frames. These caps keep a
/// pathological GIF from exhausting memory; normal photos are far below them.
const MAX_ANIMATION_FRAMES: usize = 600;
const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;

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

/// One fully composed frame of an animated GIF or WebP file.
#[derive(Debug)]
pub struct AnimationFrame {
    pub rgba: Vec<u8>,
    pub delay: Duration,
}

/// All frames of an animated file, already scaled for the GPU texture limit.
#[derive(Debug)]
pub struct Animation {
    pub path: PathBuf,
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub frames: Vec<AnimationFrame>,
}

/// Whether the extension can contain an animation. Static GIF/WebP files also
/// match; they decode to a single frame and are treated as still images.
pub fn supports_animation(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("gif") || extension.eq_ignore_ascii_case("webp")
        })
        .unwrap_or(false)
}

/// GIF delays are stored in centiseconds. Browsers treat very short delays as
/// 100 ms to avoid a slideshow of indistinguishable frames; do the same here.
pub fn animation_delay(delay: Duration) -> Duration {
    if delay < Duration::from_millis(20) {
        Duration::from_millis(100)
    } else {
        delay
    }
}

/// Decode every frame of an animated GIF or WebP. The caller decides whether
/// the frame count is worth playing; a single-frame result means the file is
/// static.
pub fn decode_animation(path: &Path, index: usize, max_texture_dim: u32) -> Result<Animation> {
    let format = image::ImageFormat::from_path(path)
        .with_context(|| format!("could not identify the format of {}", path.display()))?;
    let limit = max_texture_dim.max(1);
    let frames = match format {
        image::ImageFormat::Gif => {
            let file =
                File::open(path).with_context(|| format!("could not open {}", path.display()))?;
            collect_animation_frames(GifDecoder::new(BufReader::new(file))?.into_frames(), limit)?
        }
        image::ImageFormat::WebP => {
            let file =
                File::open(path).with_context(|| format!("could not open {}", path.display()))?;
            collect_animation_frames(WebPDecoder::new(BufReader::new(file))?.into_frames(), limit)?
        }
        _ => bail!("animated playback is only supported for GIF and WebP files"),
    };

    let (width, height) = frames
        .first()
        .map(|frame| (frame.width, frame.height))
        .context("animation has no frames")?;
    Ok(Animation {
        path: path.to_path_buf(),
        index,
        width,
        height,
        frames: frames.into_iter().map(|frame| frame.frame).collect(),
    })
}

struct SizedFrame {
    width: u32,
    height: u32,
    frame: AnimationFrame,
}

fn collect_animation_frames<I>(frames: I, limit: u32) -> Result<Vec<SizedFrame>>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    let mut collected: Vec<SizedFrame> = Vec::new();
    let mut total_bytes = 0usize;
    let mut raw_dimensions: Option<(u32, u32)> = None;
    let mut target_dimensions: Option<(u32, u32)> = None;

    for frame in frames {
        let frame = frame.context("could not decode an animation frame")?;
        let delay = animation_delay(frame.delay().into());
        let rgba = frame.into_buffer();
        let raw = (rgba.width(), rgba.height());

        if let Some(expected) = raw_dimensions {
            if raw != expected {
                bail!(
                    "animation frames have inconsistent dimensions ({}x{} and {}x{})",
                    expected.0,
                    expected.1,
                    raw.0,
                    raw.1
                );
            }
        } else {
            raw_dimensions = Some(raw);
            target_dimensions = Some(fit_dimensions(raw.0, raw.1, limit));
        }

        let target = target_dimensions.expect("dimensions are set with the first frame");
        let rgba = if raw == target {
            rgba
        } else {
            image::imageops::resize(&rgba, target.0, target.1, FilterType::Lanczos3)
        };

        total_bytes = total_bytes
            .checked_add(rgba.len())
            .context("animation is too large to decode")?;
        if collected.len() >= MAX_ANIMATION_FRAMES || total_bytes > MAX_ANIMATION_BYTES {
            bail!(
                "animation is too large to play ({} frames, {} MiB)",
                collected.len() + 1,
                total_bytes / (1024 * 1024)
            );
        }
        collected.push(SizedFrame {
            width: target.0,
            height: target.1,
            frame: AnimationFrame {
                rgba: rgba.into_raw(),
                delay,
            },
        });
    }

    Ok(collected)
}

/// Resize every frame to the exact dimensions of an existing GPU texture.
pub fn resize_animation(mut animation: Animation, width: u32, height: u32) -> Animation {
    if animation.width == width && animation.height == height {
        return animation;
    }
    for frame in &mut animation.frames {
        let image = RgbaImage::from_raw(
            animation.width,
            animation.height,
            std::mem::take(&mut frame.rgba),
        )
        .expect("animation frame dimensions must match its pixel buffer");
        frame.rgba =
            image::imageops::resize(&image, width, height, FilterType::Lanczos3).into_raw();
    }
    animation.width = width;
    animation.height = height;
    animation
}

fn fit_dimensions(width: u32, height: u32, limit: u32) -> (u32, u32) {
    if width <= limit && height <= limit {
        return (width, height);
    }
    let scale = (limit as f32 / width.max(height) as f32).min(1.0);
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}

pub fn decode_full(path: &Path, index: usize, max_texture_dim: u32) -> Result<FullImage> {
    let mut rgba = decode_rgba(path)?;
    let orientation = read_orientation(path).unwrap_or(1);

    let limit = max_texture_dim.max(1);
    if rgba.width() > limit || rgba.height() > limit {
        let (width, height) = fit_dimensions(rgba.width(), rgba.height(), limit);
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

/// Downscale an already-decoded image to the GPU texture limit. This is only
/// used if the loader's early, conservative limit turns out to be larger than
/// the actual device limit.
pub fn downscale_full(full: FullImage, max_texture_dim: u32) -> FullImage {
    let limit = max_texture_dim.max(1);
    if full.width <= limit && full.height <= limit {
        return full;
    }

    let image = RgbaImage::from_raw(full.width, full.height, full.rgba)
        .expect("decoded image dimensions must match its pixel buffer");
    let (width, height) = fit_dimensions(image.width(), image.height(), limit);
    let resized = image::imageops::resize(&image, width, height, FilterType::Lanczos3);

    FullImage {
        path: full.path,
        index: full.index,
        width,
        height,
        rgba: resized.into_raw(),
        orientation: full.orientation,
    }
}

/// Decode an image for the startup preview, scaled to fit inside
/// `max_width` x `max_height` and already oriented.
///
/// This uses a single box-filtered downscale rather than the loader's
/// high-quality path: the result is on screen for well under a second, and a
/// 24 MP image must not spend a second in Lanczos before it can be shown.
#[cfg(windows)]
pub fn decode_preview(path: &Path, max_width: u32, max_height: u32) -> Result<RgbaImage> {
    let rgba = decode_rgba(path)?;
    let orientation = read_orientation(path).unwrap_or(1);
    let (display_width, display_height) = if orientation >= 5 {
        (rgba.height(), rgba.width())
    } else {
        (rgba.width(), rgba.height())
    };

    let scale = (max_width.max(1) as f32 / display_width as f32)
        .min(max_height.max(1) as f32 / display_height as f32)
        .min(1.0);
    let width = ((display_width as f32 * scale).round() as u32).max(1);
    let height = ((display_height as f32 * scale).round() as u32).max(1);

    // Resize in the raw orientation, then rotate the small result. Rotating a
    // full-size 24 MP buffer first would cost more than the downscale itself.
    let (raw_width, raw_height) = if orientation >= 5 {
        (height, width)
    } else {
        (width, height)
    };
    let resized = if raw_width == rgba.width() && raw_height == rgba.height() {
        rgba
    } else {
        image::imageops::thumbnail(&rgba, raw_width, raw_height)
    };

    Ok(if orientation == 1 {
        resized
    } else {
        orient_rgba(&resized, orientation)
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
    use super::{
        animation_delay, decode_animation, fit_dimensions, orient_rgba, resize_animation,
        supports_animation,
    };
    use std::time::Duration;

    use image::{Delay, Frame, Rgba, RgbaImage};

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
    fn gpu_limit_dimensions_preserve_the_aspect_ratio() {
        assert_eq!(fit_dimensions(400, 200, 8192), (400, 200));
        assert_eq!(fit_dimensions(4000, 2000, 1000), (1000, 500));
        assert_eq!(fit_dimensions(2000, 4000, 1000), (500, 1000));
        assert_eq!(fit_dimensions(1, 1, 0), (1, 1));
    }

    #[test]
    fn short_gif_delays_are_clamped() {
        assert_eq!(animation_delay(Duration::ZERO), Duration::from_millis(100));
        assert_eq!(
            animation_delay(Duration::from_millis(10)),
            Duration::from_millis(100)
        );
        assert_eq!(
            animation_delay(Duration::from_millis(20)),
            Duration::from_millis(20)
        );
        assert_eq!(
            animation_delay(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn only_gif_and_webp_are_animation_candidates() {
        assert!(supports_animation(std::path::Path::new("a.GIF")));
        assert!(supports_animation(std::path::Path::new("a.webp")));
        assert!(!supports_animation(std::path::Path::new("a.jpg")));
        assert!(!supports_animation(std::path::Path::new("a.png")));
    }

    #[test]
    fn gif_animations_decode_every_frame_and_delay() {
        let directory =
            std::env::temp_dir().join(format!("picasa-rs-animation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("two.gif");

        let mut file = std::fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut file);
        encoder
            .encode_frames([
                Frame::from_parts(
                    RgbaImage::from_pixel(4, 3, Rgba([255, 0, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ),
                Frame::from_parts(
                    RgbaImage::from_pixel(4, 3, Rgba([0, 255, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(250, 1),
                ),
            ])
            .unwrap();
        drop(encoder);
        drop(file);

        let animation = decode_animation(&path, 7, 8192).unwrap();
        assert_eq!(animation.index, 7);
        assert_eq!((animation.width, animation.height), (4, 3));
        assert_eq!(animation.frames.len(), 2);
        assert_eq!(animation.frames[0].delay, Duration::from_millis(100));
        assert_eq!(animation.frames[1].delay, Duration::from_millis(250));
        assert_eq!(animation.frames[0].rgba.len(), 4 * 3 * 4);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn resized_animation_frames_match_the_target_texture() {
        let directory =
            std::env::temp_dir().join(format!("picasa-rs-animation-resize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("two.gif");

        let mut file = std::fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut file);
        encoder
            .encode_frames([
                Frame::from_parts(
                    RgbaImage::from_pixel(4, 2, Rgba([255, 0, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ),
                Frame::from_parts(
                    RgbaImage::from_pixel(4, 2, Rgba([0, 255, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ),
            ])
            .unwrap();
        drop(encoder);
        drop(file);

        let animation = decode_animation(&path, 0, 8192).unwrap();
        let resized = resize_animation(animation, 2, 1);
        assert_eq!((resized.width, resized.height), (2, 1));
        for frame in &resized.frames {
            assert_eq!(frame.rgba.len(), 8);
        }

        std::fs::remove_dir_all(directory).unwrap();
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
