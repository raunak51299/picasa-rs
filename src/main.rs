//! Picasa-inspired quick photo viewer.
//!
//! A borderless, transparent, GPU-accelerated image viewer with cursor-anchored
//! zoom, smooth panning, EXIF-aware orientation, and instant arrow-key browsing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod animation;
mod app;
mod delete;
mod info;
mod loader;
mod preferences;
mod preview;
mod renderer;
mod rotate;
mod share;
#[cfg(windows)]
mod single_instance;
mod viewer;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::UserEvent;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the process entered `main`, for startup diagnostics.
pub(crate) fn startup_ms() -> f64 {
    PROCESS_START
        .get()
        .map(|start| start.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn main() {
    if let Err(error) = run() {
        report_fatal_error(&error);
        std::process::exit(1);
    }
}

/// The release build has no console, so surface fatal errors in a dialog.
fn report_fatal_error(error: &anyhow::Error) {
    let message = format!("{error:#}");
    log::error!("{message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
        };
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "picasa-rs".encode_utf16().chain(Some(0)).collect();
        // SAFETY: both buffers are live, NUL-terminated UTF-16 strings.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }
}

/// Make `println!` reach the parent console when started from a terminal.
/// Redirected handles are left alone.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };

    // SAFETY: plain Win32 console calls; opened handles are intentionally
    // leaked because they must stay valid for the life of the process.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for kind in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if GetStdHandle(kind).is_null() {
                if let Ok(console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                    SetStdHandle(kind, console.into_raw_handle() as _);
                }
            }
        }
    }
}

fn run() -> Result<()> {
    PROCESS_START.set(Instant::now()).ok();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,picasa_rs=info"),
    )
    .format_timestamp_millis()
    .init();
    log::info!("startup[{:.1}ms]: main", startup_ms());

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        #[cfg(all(windows, not(debug_assertions)))]
        attach_parent_console();
        print_help();
        return Ok(());
    }

    #[cfg(windows)]
    let instance = single_instance::SingleInstance::acquire()
        .context("could not check for a running picasa-rs instance")?;
    #[cfg(windows)]
    if instance.is_none() {
        let request = match args.iter().find(|arg| !arg.starts_with('-')) {
            Some(path) => {
                let path = PathBuf::from(path);
                let path = std::path::absolute(&path)
                    .with_context(|| format!("invalid path: {}", path.display()))?;
                single_instance::Request::Open(path)
            }
            None => single_instance::Request::Focus,
        };
        match single_instance::forward(&request) {
            Ok(()) => {
                log::info!(
                    "startup[{:.1}ms]: handed the request to the running viewer",
                    startup_ms()
                );
                return Ok(());
            }
            Err(error) => {
                log::warn!("another viewer is running but did not accept the request: {error:#}");
            }
        }
    }

    let initial_path = resolve_initial_image(&args)?;

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("failed to create the winit event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    log::info!("startup[{:.1}ms]: event loop created", startup_ms());

    let proxy = event_loop.create_proxy();
    #[cfg(windows)]
    if let Err(error) = single_instance::start_server(proxy.clone()) {
        log::warn!("single-instance listener unavailable: {error:#}");
    }
    let mut app = app::App::new(initial_path, proxy)?;
    log::info!("startup[{:.1}ms]: app state ready", startup_ms());

    event_loop
        .run_app(&mut app)
        .context("the window event loop terminated with an error")?;

    Ok(())
}

fn print_help() {
    println!(
        "\
picasa-rs — a fast, borderless photo viewer

USAGE:
    picasa-rs <IMAGE>
    picasa-rs <DIRECTORY>

If no path is supplied the first supported image in the current directory is used.
Launching picasa-rs while it is already running reuses that window.

CONTROLS:
    Left / Right        previous / next image
    Home / End          first / last image
    Scroll wheel        zoom around the cursor
    Up / Down           zoom in / out around the centre
    Drag                pan
    Drop a file/folder  open it and rebuild the folder listing
    Double click / 1    toggle fit and 1:1
    0                   reset view
    Space / S           start / pause slideshow
    [ / ]               shorten / lengthen slideshow interval
    Delete              move the current photo to the Recycle Bin
    I                   show or hide photo metadata
    C                   show or hide share actions
    Ctrl + C            copy the current view to the clipboard
    Ctrl + Shift + C    copy the file path
    Ctrl + E            show the file in Explorer
    Ctrl + Enter        open with the default application
    Ctrl + P            print with the registered print handler
    R / L               rotate the view clockwise / counter-clockwise (session only)
    Ctrl + Left/Right   rotate the view clockwise / counter-clockwise
    H                   show help
    F5                  refresh the folder listing and retry failed images
    Esc                 stop slideshow, or close
    Q                   close

SUPPORTED FORMATS:
    JPEG, PNG, WebP, TIFF, BMP, GIF
    Animated GIF and WebP files play automatically.

PREFERENCES:
    %APPDATA%\\picasa-rs\\config.toml
    slideshow_interval_secs = 4    (2-30)
    scrim_opacity = 0.75           (0.0-1.0)
    confirm_delete = true
"
    );
}

fn resolve_initial_image(args: &[String]) -> Result<PathBuf> {
    let candidate = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from);

    if let Some(candidate) = candidate {
        let path = std::path::absolute(&candidate)
            .with_context(|| format!("invalid path: {}", candidate.display()))?;

        if path.is_dir() {
            return first_image_in(&path).with_context(|| {
                format!("no supported images found in directory {}", path.display())
            });
        }

        if path.is_file() {
            return Ok(path);
        }

        bail!("file not found: {}", path.display());
    }

    let cwd = std::env::current_dir().context("could not read the current directory")?;
    first_image_in(&cwd)
        .context("no image supplied and no supported images found in the current directory")
}

fn first_image_in(directory: &Path) -> Result<PathBuf> {
    viewer::navigation::list_images(directory)?
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "directory {} contains no supported images",
                directory.display()
            )
        })
}
