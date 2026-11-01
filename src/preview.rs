//! Instant native preview shown while the GPU renderer is initializing.
//!
//! wgpu's DX12 adapter enumeration can take most of a second on machines with
//! several GPUs. A layered Win32 window lets the viewer paint the dim backdrop
//! in a few milliseconds, long before the swapchain is ready. The first photo
//! is decoded and composited on a worker thread so it does not delay GPU
//! initialization either. The preview is destroyed once the real photo has
//! been presented.

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};

    use anyhow::{Result, bail};
    use image::RgbaImage;
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        ReleaseDC, SelectObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_TOPMOST, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
        SWP_SHOWWINDOW, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    use crate::loader::decoder;

    const BACKDROP_ALPHA: u32 = 191; // 0.75 * 255, matching DIM_BACKDROP_ALPHA.
    const FIT_WIDTH: f32 = 0.88;
    const FIT_HEIGHT: f32 = 0.80;

    pub struct PreviewWindow {
        hwnd: HWND,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    }

    impl PreviewWindow {
        /// Create the preview and paint the dim backdrop immediately, then
        /// decode and paint the first image on a worker thread. Both steps
        /// happen while the GPU adapter is still being initialized.
        pub fn show(path: &Path, x: i32, y: i32, width: u32, height: u32) -> Result<Self> {
            let preview = Self::new(x, y, width.max(1), height.max(1))?;
            preview.paint_backdrop()?;
            preview.show_window();
            log::info!(
                "startup[{:.1}ms]: preview backdrop shown",
                crate::startup_ms()
            );
            preview.paint_photo_in_background(path.to_path_buf());
            Ok(preview)
        }

        fn new(x: i32, y: i32, width: u32, height: u32) -> Result<Self> {
            let class = wide("STATIC");
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                    class.as_ptr(),
                    null(),
                    WS_POPUP,
                    x,
                    y,
                    width as i32,
                    height as i32,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null(),
                )
            };

            if hwnd.is_null() {
                bail!(
                    "CreateWindowExW failed for the startup preview: {}",
                    std::io::Error::last_os_error()
                );
            }

            Ok(Self {
                hwnd,
                x,
                y,
                width,
                height,
            })
        }

        fn show_window(&self) {
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    self.x,
                    self.y,
                    self.width as i32,
                    self.height as i32,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }

        fn paint_backdrop(&self) -> Result<()> {
            let pixels = backdrop_pixels(self.width, self.height);
            update_layered(self.hwnd, self.x, self.y, self.width, self.height, &pixels)
        }

        fn paint_photo_in_background(&self, path: PathBuf) {
            let hwnd = self.hwnd as isize;
            let (x, y, width, height) = (self.x, self.y, self.width, self.height);
            let spawned = std::thread::Builder::new()
                .name("startup-preview".into())
                .spawn(move || {
                    let max_width = (width as f32 * FIT_WIDTH).round().max(1.0) as u32;
                    let max_height = (height as f32 * FIT_HEIGHT).round().max(1.0) as u32;
                    let photo = match decoder::decode_preview(&path, max_width, max_height) {
                        Ok(photo) => photo,
                        Err(error) => {
                            log::warn!("could not decode the startup preview: {error:#}");
                            return;
                        }
                    };
                    let pixels = photo_pixels(photo, width, height);
                    // The window may already have been destroyed if the GPU
                    // won the race; UpdateLayeredWindow then fails harmlessly.
                    if let Err(error) = update_layered(hwnd as HWND, x, y, width, height, &pixels) {
                        log::debug!("startup preview hand-off: {error:#}");
                    } else {
                        log::info!(
                            "startup[{:.1}ms]: preview photo painted",
                            crate::startup_ms()
                        );
                    }
                });
            if let Err(error) = spawned {
                log::warn!("could not start the startup preview thread: {error}");
            }
        }
    }

    impl Drop for PreviewWindow {
        fn drop(&mut self) {
            if !self.hwnd.is_null() {
                unsafe {
                    DestroyWindow(self.hwnd);
                }
            }
        }
    }

    fn backdrop_pixels(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            *pixel = [0, 0, 0, BACKDROP_ALPHA as u8];
        }
        pixels
    }

    fn photo_pixels(photo: RgbaImage, width: u32, height: u32) -> Vec<u8> {
        let photo_width = photo.width();
        let photo_height = photo.height();

        let mut pixels = backdrop_pixels(width, height);
        let offset_x = (width - photo_width) / 2;
        let offset_y = (height - photo_height) / 2;
        let source = photo.as_raw();
        let stride = photo_width as usize * 4;
        for row in 0..photo_height as usize {
            let source_row = &source[row * stride..(row + 1) * stride];
            let destination_start =
                ((offset_y as usize + row) * width as usize + offset_x as usize) * 4;
            let destination = &mut pixels[destination_start..destination_start + stride];
            for (source_pixel, destination_pixel) in source_row
                .as_chunks::<4>()
                .0
                .iter()
                .zip(destination.as_chunks_mut::<4>().0.iter_mut())
            {
                // Composite the photo over the dim backdrop in premultiplied
                // BGRA, which is what ULW_ALPHA expects.
                let alpha = source_pixel[3] as u32;
                let red = (source_pixel[0] as u32 * alpha + 127) / 255;
                let green = (source_pixel[1] as u32 * alpha + 127) / 255;
                let blue = (source_pixel[2] as u32 * alpha + 127) / 255;
                let out_alpha = alpha + (BACKDROP_ALPHA * (255 - alpha) + 127) / 255;
                *destination_pixel = [blue as u8, green as u8, red as u8, out_alpha as u8];
            }
        }
        pixels
    }

    fn update_layered(
        hwnd: HWND,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<()> {
        let screen = unsafe { GetDC(null_mut()) };
        if screen.is_null() {
            bail!(
                "GetDC failed for the startup preview: {}",
                std::io::Error::last_os_error()
            );
        }

        let memory = unsafe { CreateCompatibleDC(screen) };
        if memory.is_null() {
            unsafe { ReleaseDC(null_mut(), screen) };
            bail!(
                "CreateCompatibleDC failed for the startup preview: {}",
                std::io::Error::last_os_error()
            );
        }

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative height means a top-down DIB, matching image row order.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            ..Default::default()
        };

        let mut bits: *mut c_void = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(memory);
                ReleaseDC(null_mut(), screen);
            }
            bail!(
                "CreateDIBSection failed for the startup preview: {}",
                std::io::Error::last_os_error()
            );
        }

        unsafe {
            SelectObject(memory, bitmap);
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr(),
                bits as *mut u8,
                pixels.len().min(width as usize * height as usize * 4),
            );
        }

        let destination = POINT { x, y };
        let source = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: width as i32,
            cy: height as i32,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        let updated = unsafe {
            UpdateLayeredWindow(
                hwnd,
                screen,
                &destination,
                &size,
                memory,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        };

        unsafe {
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(null_mut(), screen);
        }

        if updated == 0 {
            bail!(
                "UpdateLayeredWindow failed for the startup preview: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(())
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use anyhow::Result;

    pub struct PreviewWindow;

    impl PreviewWindow {
        pub fn show(_path: &Path, _x: i32, _y: i32, _width: u32, _height: u32) -> Result<Self> {
            Ok(Self)
        }
    }
}

pub use platform::PreviewWindow;
