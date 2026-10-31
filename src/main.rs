//! Picasa-inspired quick photo viewer.
//!
//! A borderless, transparent, GPU-accelerated image viewer with cursor-anchored
//! zoom, smooth panning, EXIF-aware orientation, and instant arrow-key browsing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod loader;
mod renderer;
mod viewer;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::UserEvent;
use crate::loader::decoder;

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,picasa_rs=info"),
    )
    .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let initial_path = resolve_initial_image(&args)?;

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("failed to create the winit event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(initial_path, proxy)?;

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

CONTROLS:
    Left / Right        previous / next image
    Home / End          first / last image
    Scroll wheel        zoom around the cursor
    Drag                pan
    Double click / 1    toggle fit and 1:1
    0 / Space           reset view
    R / L               rotate view clockwise / counter-clockwise
    H                   show help
    Esc / Q             close

SUPPORTED FORMATS:
    JPEG, PNG, WebP, TIFF, BMP, GIF
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
    let mut images: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("could not read {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && decoder::is_supported(path))
        .collect();

    images.sort_by(|a, b| {
        viewer::navigation::natural_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });

    images.into_iter().next().with_context(|| {
        format!(
            "directory {} contains no supported images",
            directory.display()
        )
    })
}
