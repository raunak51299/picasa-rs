//! Share actions: clipboard, Explorer, and the default application.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::adjust::{Adjustments, apply_to_rgba};
use crate::loader::decoder;
use crate::rotate;

/// Decode the photo as it appears on screen: EXIF orientation applied and color
/// fixes baked in.
pub fn display_rgba(path: &Path, adjustments: Adjustments) -> Result<RgbaImage> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let decoded = image::load_from_memory(&data)
        .with_context(|| format!("could not decode {}", path.display()))?
        .to_rgba8();
    let orientation = rotate::read_orientation(&data).unwrap_or(1) as u8;
    let mut rgba = decoder::orient_rgba(&decoded, orientation);
    apply_to_rgba(rgba.as_mut(), adjustments);
    Ok(rgba)
}

/// Put the current view of the photo on the Windows clipboard.
pub fn copy_image(path: &Path, adjustments: Adjustments) -> Result<()> {
    let rgba = display_rgba(path, adjustments)?;
    let mut clipboard =
        arboard::Clipboard::new().context("could not open the Windows clipboard")?;
    clipboard
        .set_image(arboard::ImageData {
            width: rgba.width() as usize,
            height: rgba.height() as usize,
            bytes: Cow::Owned(rgba.into_raw()),
        })
        .context("could not put the image on the clipboard")?;
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> {
    let mut clipboard =
        arboard::Clipboard::new().context("could not open the Windows clipboard")?;
    clipboard
        .set_text(text.to_owned())
        .context("could not put text on the clipboard")?;
    Ok(())
}

pub fn copy_path(path: &Path) -> Result<()> {
    copy_text(&path.display().to_string())
}

/// Open Explorer with the file selected.
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::process::Command;

        Command::new("explorer.exe")
            .arg("/select,")
            .arg(path)
            .spawn()
            .with_context(|| format!("could not start Explorer for {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("revealing files in Explorer is only available on Windows")
    }
}

/// Open the file in whatever application is registered for its type.
pub fn open_default(path: &Path) -> Result<()> {
    shell_execute("open", path, "open with its default application")
}

/// Open a URL in the user's default browser.
pub fn open_url(url: &str) -> Result<()> {
    shell_execute(
        "open",
        Path::new(url),
        "open the link in the default browser",
    )
}

/// Send the file to the application registered for the shell `print` verb.
pub fn print(path: &Path) -> Result<()> {
    shell_execute("print", path, "print")
}

#[cfg(windows)]
fn shell_execute(verb: &str, path: &Path, description: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = verb.encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        bail!(
            "Windows could not {description} {} (code {})",
            path.display(),
            result as isize
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn shell_execute(_verb: &str, path: &Path, description: &str) -> Result<()> {
    bail!(
        "cannot {description} {}: shell actions are only available on Windows",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("picasa-rs-share-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn display_pixels_apply_adjustments() {
        let directory = temp_dir("adjust");
        let path = directory.join("gray.png");
        RgbaImage::from_pixel(2, 2, Rgba([64, 64, 64, 255]))
            .save(&path)
            .unwrap();

        let plain = display_rgba(&path, Adjustments::default()).unwrap();
        let light = display_rgba(&path, Adjustments::lighten()).unwrap();
        assert!(light.get_pixel(0, 0)[0] > plain.get_pixel(0, 0)[0]);
        assert_eq!(light.get_pixel(0, 0)[3], 255);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn display_pixels_apply_exif_orientation() {
        let directory = temp_dir("orientation");
        let path = directory.join("sideways.jpg");
        let image = RgbaImage::from_pixel(2, 3, Rgba([128, 128, 128, 255]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 100)
            .encode_image(&image::DynamicImage::ImageRgba8(image))
            .unwrap();
        let jpeg = rotate::rewrite_jpeg_orientation(&cursor.into_inner(), 6).unwrap();
        fs::write(&path, jpeg).unwrap();

        let display = display_rgba(&path, Adjustments::default()).unwrap();
        assert_eq!(display.dimensions(), (3, 2));

        fs::remove_dir_all(directory).unwrap();
    }
}
