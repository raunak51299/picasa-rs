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

| Feature | Picasa Photo Viewer behaviour | picasa-rs plan |
| :--- | :--- | :--- |
| Persistent rotation | Rotate buttons wrote the rotation to the file. | `R`/`L` and `Ctrl+Left/Right` rewrite the EXIF orientation of JPEGs after confirmation. `Shift+R`/`L` keeps the old view-only rotation. |
| Fit / 1:1 | Fit, actual size, and a zoom percentage. | Already present; keep the existing controls. |
| Slideshow | Play/pause, interval, full-screen advance. | Play/pause button, `Space`/`S`, configurable interval, optional crossfade, `Esc` exits. |
| Delete | `Delete` moved the photo to the Recycle Bin and advanced. | Confirmation, Recycle Bin via the Windows shell, advance to the next image. |
| Info panel | `I` showed file and camera metadata. | Overlay with filename, dimensions, size, date, camera, exposure, ISO, focal length, and GPS. |
| One-click fixes | Auto colour/contrast, lighten, darken, then save. | GPU brightness/contrast/gamma/level uniforms, non-destructive until Save or Save a copy, plus Revert. JPEG saves keep the original EXIF APP1 segment and re-encode at quality 92. |
| Open and share | Reveal in Explorer, open in an editor, copy. | Reveal, open with the default editor, copy image to clipboard, copy file path. |
| Print | Printed the current photo with the shell print verb. | Windows shell `print` verb on the current file. |
| Animated GIF | Played GIF frames with their delays. | Frame-delay playback, with WebP animation if the decoder support is cheap. |
| Drag and drop | Dropping a file or folder opened it. | Accept dropped files and folders and rebuild the folder listing. |
| Open with / associations | Installed as an "Open with" handler. | Keep the CLI path argument working and register associations in the Windows installer. |
| Single instance | A second launch reused the running viewer. | Forward the new path to the existing process and reuse its window. |
| Preferences | Remembered slideshow and viewer settings. | `%APPDATA%\picasa-rs\config.toml` for slideshow interval, scrim opacity, and delete confirmation. |
| Optional formats | Picasa read a wide range of camera formats. | Optional AVIF/HEIC/RAW features only where a dependency can be verified; otherwise document the gap. |

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
