# Picasa Photo Viewer parity

Google's Picasa Photo Viewer was the lightweight, full-screen viewer opened by
double-clicking a photo when Picasa was installed. It was not the Picasa library
manager: it showed one photo at a time over a dimmed desktop, with a filmstrip,
arrow-key browsing, zoom/pan, and a small toolbar for the actions most people
needed from a viewer.

This document compares that viewer with picasa-rs and records what this branch
adds.

## Already present

- Borderless, transparent window with a dimmed desktop backdrop.
- Opens a file or a folder, scans the folder, and navigates with `Left`/`Right`,
  `Home`/`End`, wrapping at either end.
- Filmstrip thumbnails, auto-fading HUD, and a loading indicator.
- Cursor-anchored zoom, drag panning, 90-degree *view* rotation, and EXIF
  orientation for all eight orientation values.
- Fit-to-screen and 1:1 toggles, double-click toggling, a zoom slider, and a
  percentage readout.
- Background decoding with prefetch, stale-job cancellation, and a native
  startup preview.

## Gaps addressed in this branch

| Feature | Picasa Photo Viewer behaviour | picasa-rs status |
| :--- | :--- | :--- |
| Persistent rotation | Rotate buttons wrote the rotation to the file. | Done: `R`/`L` and `Ctrl+Left/Right` rewrite the EXIF orientation of JPEGs after confirmation. `Shift+R`/`L` keeps the old view-only rotation. |
| Fit / 1:1 | Fit, actual size, and a zoom percentage. | Already present; the existing controls were kept. |
| Slideshow | Play/pause, interval, full-screen advance. | Done: play/pause button, `Space`/`S`, 2-30 second interval with `[`/`]`, fade-in per photo, and `Esc` stops without closing. |
| Delete | `Delete` moved the photo to the Recycle Bin and advanced. | Done: confirmation (disable with `confirm_delete = false`), Recycle Bin via the Windows shell, and advance to the next image. |
| Info panel | `I` showed file and camera metadata. | Done: overlay with filename, dimensions, size, date, camera, exposure, ISO, focal length, and GPS. |
| One-click fixes | Auto colour/contrast, lighten, darken, then save. | Done: GPU brightness/contrast/gamma/level uniforms, non-destructive until Save or Save a copy, plus Revert. JPEG saves keep the original EXIF APP1 segment and re-encode at quality 92. |
| Open and share | Reveal in Explorer, open in an editor, copy. | Done: share panel (`C`) with Copy image, Copy path, Open, Show in folder, and Print, plus the `Ctrl+C`, `Ctrl+Shift+C`, `Ctrl+E`, `Ctrl+Enter`, and `Ctrl+P` shortcuts. |
| Print | Printed the current photo with the shell print verb. | Done: Windows shell `print` verb on the current file. |
| Animated GIF | Played GIF frames with their delays. | Done: frame-delay playback for GIF and animated WebP, looping while the photo is on screen, restarting after navigation, and capped at 600 frames or 256 MiB. |
| Drag and drop | Dropping a file or folder opened it. | Done: a dropped photo opens in its folder; a dropped folder opens its first supported image. |
| Open with / associations | Installed as an "Open with" handler. | Done: `packaging/windows/install.ps1` registers a per-user ProgID, `OpenWithProgids`, a Default Programs `Capabilities` entry, and "Open with picasa-rs" verbs for image files and folders. The CLI path argument remains the entry point. |
| Single instance | A second launch reused the running viewer. | Done: a session-scoped named mutex plus named pipe forwards the requested file or folder to the existing window, which also comes to the foreground. A launch without a path only focuses the window. |
| Preferences | Remembered slideshow and viewer settings. | Done: `%APPDATA%\picasa-rs\config.toml` stores the slideshow interval, scrim opacity, and delete confirmation. Interval changes save automatically; malformed files fall back to defaults. |
| Optional formats | Picasa read a wide range of camera formats. | Skipped after probing: AVIF needs `image`'s `avif-native` feature plus a system libdav1d ≥ 1.3 found by pkg-config (the probe failed here, and the pure-Rust `avif` feature only encodes); HEIC/HEIF needs the native libheif library; RAW would need `rawler` (LGPL-2.1) plus demosaicing. No unverifiable dependency was added. |

One intentional difference: Picasa's viewer closed when the dimmed desktop was
clicked. picasa-rs instead drops into a borderless, photo-sized window that
keeps the photo, HUD, and draggable filmstrip; a top-right `×` (or `Q` / `Esc`)
closes it. Clicking the dimmed area again while windowed is inert.

## Deliberately out of scope

These are Picasa *library* features, not Photo Viewer features:

- Albums, folders, and library management.
- Face detection and face tags.
- Geotagging and map browsing.
- Web upload, sharing services, and online albums.
- Collages, movie makers, and other creation tools.
- Database-backed thumbnail caches and non-destructive library edits.

picasa-rs remains a single-file/single-folder viewer. It does not import photos
into a library or maintain a catalogue.
