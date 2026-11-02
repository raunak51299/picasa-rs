# picasa-rs

A Picasa-inspired quick photo viewer for the desktop: a borderless, fullscreen
overlay that dims the desktop behind the photo, opens quickly, and lets you
scrub through a folder with the arrow keys. Clicking the dimmed desktop drops
into a photo-sized window with the native Windows title bar.

Built with Rust, [`winit`](https://crates.io/crates/winit),
[`wgpu`](https://crates.io/crates/wgpu), and [`egui`](https://crates.io/crates/egui).

## Features

- **Transparent desktop backdrop** — the desktop stays visible through a 75%
  black scrim, so the photo floats over your workspace instead of replacing it.
- **Instant display** — a native layered preview paints the dim backdrop in about
  20 ms and the first photo in about 0.1 s while `wgpu` initializes. The GPU
  frame takes over with the same fit, orientation, and position.
- **Cursor-anchored zoom and pan** — the point under the pointer stays put while
  zooming. Motion is critically damped and frame-rate independent.
- **EXIF-aware orientation** — all eight EXIF orientation values are applied
  correctly. Full-resolution photos are oriented on the GPU; thumbnails are
  oriented on the CPU.
- **Folder navigation** — `Left` / `Right` wrap around the folder, with natural
  filename sorting and neighbor prefetching for near-instant paging.
- **Drag and drop** — drop a photo to open it, or a folder to open its first
  supported image and rebuild the listing around that folder.
- **One window per session** — launching picasa-rs again focuses the existing
  viewer and opens the requested file or folder there instead of starting a
  second copy.
- **Persistent preferences** — the slideshow interval, desktop scrim opacity,
  and delete confirmation are read from `%APPDATA%\picasa-rs\config.toml`.
- **Filmstrip and HUD** — a thumbnail strip stays pinned to the bottom of the
  viewer, even while the compact control bar is hidden. The control bar appears
  on real mouse movement, then fades after three seconds idle; small jitter does
  not bring it back. Drag the filmstrip to scroll it, or click a thumbnail to
  jump to that photo.
- **Windowed mode** — clicking the dimmed desktop leaves fullscreen for a
  resizable window sized to the photo and centred on the monitor. It uses the
  native Windows title bar with minimize, maximize, and close, while the client
  area stays transparent and keeps the HUD and filmstrip. Drag any edge or
  corner to resize it; the maximize button returns to borderless fullscreen;
  clicking outside the photo while windowed does nothing.
- **Slideshow** — `Space` or `S` plays the folder with a 2–30 second interval
  (`[` / `]`), fades each photo in, and wraps at the end. `Esc` stops the
  slideshow without closing the viewer.
- **Recycle Bin delete** — `Delete` asks for confirmation (unless disabled in
  preferences) and moves the current photo to the Windows Recycle Bin, then
  advances to the next image.
- **Info panel** — `I` shows the file name, dimensions, size, capture date,
  camera, lens, exposure, aperture, ISO, focal length, and GPS coordinates when
  the file has them.
- **Animated photos** — animated GIF and WebP files play automatically with
  their frame delays, looping while the photo is on screen. Playback restarts
  from the first frame when you navigate back. Animations larger than 256 MiB
  fall back to their first frame.
- **Share actions** — `C` opens a share panel with Copy image, Copy path, Open,
  Show in folder, and Print. Copy image captures the current view, including
  EXIF orientation and color fixes, so it can be pasted into chat or an editor.
  Print hands the file to the shell `print` handler.
- **GPU color fixes** — `F` opens non-destructive brightness, contrast, gamma,
  and per-channel controls plus Auto, Lighten, Darken, Warm, and Cool presets.
  The shader applies them live without touching the file; Revert clears them.
  Save rewrites the file, Save a copy writes a sibling `(fixed)` file, and
  adjustments reset when you move to another photo.
- **Large images** — mipmaps are generated on the GPU, so multi-megapixel photos
  downscale cleanly and quickly.
- **Persistent rotation** — `R`/`L` or `Ctrl+Left`/`Right` rotate a JPEG by
  rewriting its EXIF orientation after confirmation; the compressed pixels are
  never re-encoded. `Shift+R`/`L` keeps the old view-only rotation.

## Requirements

- Rust 1.88 or newer (edition 2024 and let-chains).
- Windows 10/11 for the transparent fullscreen experience. Linux and macOS
  builds use the same rendering path, but the desktop-composition behavior has
  only been exercised on Windows.
- A GPU and driver supported by `wgpu` (DX12 on Windows).

### Windows toolchain

The project builds with either the MSVC or the GNU toolchain. If you use the GNU
toolchain, make sure a MinGW-w64 `bin` directory is on `PATH`:

```powershell
rustup default stable-x86_64-pc-windows-gnu
$env:Path = "C:\path\to\mingw64\bin;$env:USERPROFILE\.cargo\bin;$env:Path"
cargo build --release
```

With the MSVC toolchain, install the Visual Studio C++ build tools and run:

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo build --release
```

## Build and run

```powershell
cargo run --release -- "C:\Photos\holiday.jpg"
```

You can also pass a directory; the viewer opens the first supported image in it:

```powershell
cargo run --release -- "C:\Photos"
```

With no argument, the first supported image in the current directory is used.

The release binary is written to `target\release\picasa-rs.exe`. Debug builds
keep a console window for logs; release builds hide it.

Logging is controlled by `RUST_LOG`:

```powershell
$env:RUST_LOG = "info"
.\target\release\picasa-rs.exe "C:\Photos\holiday.jpg"
```

## Controls

| Input | Action |
| :--- | :--- |
| `Left` / `Right` | Previous / next image (wraps) |
| `Home` / `End` | First / last image |
| Mouse wheel | Zoom around the cursor |
| Drag | Pan (drag the filmstrip to scroll it) |
| Drop a file or folder | Open it and rebuild the folder listing |
| Double-click / `1` | Toggle fit and 1:1 |
| `0` | Reset the view |
| `Space` / `S` | Start / pause the slideshow |
| `[` / `]` | Shorten / lengthen the slideshow interval |
| `I` | Show / hide the metadata panel |
| `F` | Show / hide the color-fix panel |
| `C` | Show / hide the share panel |
| `Ctrl+C` | Copy the current view (including fixes) to the clipboard |
| `Ctrl+Shift+C` | Copy the file path |
| `Ctrl+E` | Show the file in Explorer |
| `Ctrl+Enter` | Open the file in its default application |
| `Ctrl+P` | Print the file using the registered print handler |
| `Delete` | Move the current photo to the Recycle Bin (asks first) |
| `R` / `L` | Rotate clockwise / counter-clockwise and save (JPEG EXIF) |
| `Ctrl+Left` / `Ctrl+Right` | Rotate and save |
| `Shift+R` / `Shift+L` | Rotate the view only |
| `H` | Show the help hint |
| `Esc` | Stop the slideshow, or close when it is not playing |
| `Q` | Close |
| Click outside the photo | Leave fullscreen for a photo-sized window (inert when windowed) |
| Drag a window edge or corner | Resize the windowed view |
| Top-right `×` (fullscreen) | Close |
| Native title-bar buttons (windowed) | Minimize or close |
| Native maximize button (windowed) | Return to fullscreen |

### Saving color fixes

The fix panel previews adjustments on the GPU. **Save** asks for confirmation
and replaces the original file; **Save a copy** writes
`<name> (fixed).<ext>` (then `(fixed 2)`, and so on) and leaves the original
untouched. JPEGs are re-encoded at quality 92 and keep their original EXIF APP1
segment byte for byte, including the orientation tag. Other formats are
re-encoded by the `image` crate with the orientation baked into the pixels,
because their metadata is dropped. Saving color fixes for GIF files is not
supported yet.

## Preferences

picasa-rs reads `%APPDATA%\picasa-rs\config.toml` at startup and creates it the
first time you change the slideshow interval with `[` or `]`:

```toml
# picasa-rs preferences
slideshow_interval_secs = 4   # 2-30
scrim_opacity = 0.75          # 0.0 = clear, 1.0 = black
confirm_delete = true
```

- `slideshow_interval_secs` is clamped to 2-30 seconds and is updated when you
  change the interval from the keyboard or HUD.
- `scrim_opacity` controls the black desktop backdrop for both the native
  startup preview and the GPU renderer.
- `confirm_delete = false` moves the current photo to the Recycle Bin as soon as
  you press `Delete`, without the confirmation card.
- Unknown keys and `[section]` headers are ignored. A malformed file is logged
  and the defaults are used for that run.

## Transparency on Windows

The window starts as a borderless fullscreen `winit` window created with
`with_transparent(true)`. On Windows the renderer forces the DX12 backend and
selects wgpu's DirectComposition presentation path
(`wgpu::Dx12SwapchainKind::DxgiFromVisual`). Unlike a plain HWND swapchain,
which only advertises `CompositeAlphaMode::Opaque`, a DirectComposition visual
supports `PreMultiplied` alpha. The renderer:

1. clears the swapchain to premultiplied black with alpha `0.75`;
2. draws the photo and egui overlay with premultiplied-alpha blending;
3. lets DWM composite the result over the desktop.

The result is true per-pixel transparency: the desktop is visible at 25% of its
original brightness around the photo. Clicking that dimmed area leaves
fullscreen for a resizable window sized to the photo and centred on the monitor,
where the same transparency is kept. Windowed mode uses the native Windows title
bar (minimize, maximize, and close). In fullscreen, a quarter-circle `×` in the
top-right corner closes the viewer, and clicking the dimmed area while windowed
is inert. To change the dim level, edit `DIM_BACKDROP_ALPHA` in
`src/renderer/mod.rs` (`0.0` is fully clear, `1.0` is fully opaque).

Notes:

- DWM (Desktop Window Manager) composition must be enabled. It is on by default
  on Windows 10/11.
- The backdrop is dimmed, not blurred. There is no acrylic/mica effect.
- If the compositor does not offer `PreMultiplied`, the renderer falls back to an
  opaque near-black backdrop and logs a warning.
- The Vulkan backend is intentionally not used on Windows: on the development
  machine (Intel Iris Xe) it crashes inside `vkCreateDevice`. DX12 is stable.
- A borderless fullscreen window combined with `WindowLevel::AlwaysOnTop`
  produced a DXGI swapchain creation failure (`0x887A0001`), so the viewer does
  not set always-on-top. The fullscreen window already covers the taskbar.

## Startup performance

On Windows the first frame has two separate costs:

1. **Adapter and device creation.** `wgpu`'s DX12 backend enumerates every
   exposed adapter and creates a D3D12 device for each one. On the development
   machine (Intel Iris Xe) this takes roughly 0.6–0.8 s warm and 1.2 s cold, and
   it happens before a single pixel can be presented. This is the floor for a
   `wgpu`/DX12 viewer.
2. **Decoding and uploading the photo.** The background loader starts before the
   GPU is initialized, so by the time the device is ready the first image is
   usually decoded. Upload plus mipmap generation takes a few milliseconds.

To hide the adapter cost, picasa-rs paints a native layered Win32 preview before
`wgpu` starts: the 75% scrim appears in about 20 ms, and the first photo is
decoded, scaled, and composited on a worker thread in roughly 0.1 s for a
typical 1600×1000 JPEG and about 0.2 s for a 24 MP (6000×4000) image. The
preview is removed a few frames after the real swapchain presents, so the
hand-off is invisible.

Warm, alternating launches of the same 1600×1000 JPEG (medians of five runs on
the development machine; expect ±15% run-to-run variance):

| Milestone | Before | After |
| :--- | ---: | ---: |
| Photo visible | ~1.37 s | ~0.1 s (native preview) |
| GPU window visible | ~1.16 s | ~1.07 s |
| GPU frame presented | ~1.38 s | ~1.17 s |

The remaining ~1 s is DX12 adapter/device initialization. It can only be removed
by changing the architecture: keeping a process resident (a tray app or
pre-warmed helper), using a lighter presentation path, or accepting a less
capable renderer. For a one-shot, transparent, GPU-composited viewer, the native
preview is the largest win available.

Compared with Picasa: Picasa 3 was a long-lived native application. Its window,
image pipeline, and thumbnail database were already initialized, so opening a
photo only required decoding and drawing it. picasa-rs starts from a cold process
each time and must create a GPU device before it can draw. The preview closes
most of the perceived gap for a single photo, but the GPU path still has a
one-time cost that Picasa did not pay on every launch.

## Packaging

Helper files live in `packaging/`:

- `packaging/windows/install.ps1` — copies the release executable to
  `%LOCALAPPDATA%\Programs\picasa-rs`, registers a per-user ProgID, associates
  common image extensions, registers with Default Programs, and adds an
  "Open with picasa-rs" context-menu entry for images and folders. No
  administrator rights required.
- `packaging/windows/uninstall.ps1` — removes those associations and optionally
  deletes the installed files.
- `packaging/windows/picasa-rs.ico` — application icon.
- `packaging/linux/picasa-rs.desktop` — desktop entry.
- `packaging/macos/Info.plist` — bundle metadata.

Example:

```powershell
cd packaging\windows
.\install.ps1
```

The scripts only touch `HKCU`, so other users and system-wide associations are
left alone. Run `.\uninstall.ps1 -RemoveFiles` to undo the install.

## Development

```powershell
cargo fmt
cargo clippy --all-targets
cargo test
cargo build --release
```

The test suite covers EXIF orientation for all eight values and natural filename
sorting.

### Project layout

```
src/
  main.rs                 CLI, event loop, initial path resolution
  app/                    window state, input, HUD/filmstrip, frame loop
    mod.rs                application state and winit event handling
    animation.rs          animated photo playback and GPU uploads
    cache.rs              distance-based photo and thumbnail eviction
    events.rs             loader result handling
    input.rs              keyboard, mouse, and HUD action handling
    slideshow.rs          timed slideshow state
    ui.rs                 HUD, filmstrip, help, and panels
  adjust.rs               color-fix presets and pixel math
  animation.rs            animated photo playback timing
  delete.rs               Recycle Bin shell call
  info.rs                 EXIF metadata reading and formatting
  preview.rs              native layered startup preview (Windows)
  rotate.rs               lossless JPEG EXIF orientation rewriting
  save.rs                 adjusted-image saving and JPEG EXIF preservation
  share.rs                clipboard, Explorer, open, and print actions
  loader/
    mod.rs                priority worker queue, prefetch, shutdown
    decoder.rs            image decode, EXIF orientation, thumbnails, animation
  renderer/
    mod.rs                wgpu device/surface, DirectComposition, draw calls
    quad_pipeline.rs      photo and shadow pipelines
    mipmap.rs             GPU mipmap generation
    shaders/              WGSL photo, shadow, and mipmap shaders
  viewer/
    camera.rs             cursor-anchored zoom/pan/rotation
    navigation.rs         folder scan, natural sort, wrapping
```

## Known limitations

- Transparency is Windows-only in practice; other platforms fall back to the
  compositor's available alpha modes. The native startup preview is Windows-only
  too; elsewhere the window appears when the GPU is ready.
- Very large images are limited by the GPU's maximum texture size (typically
  8192 or 16384 px). Larger images are downscaled before upload.
- Animated files are decoded into full-canvas frames and capped at 600 frames
  or 256 MiB; larger animations show their first frame instead.
- AVIF, HEIC/HEIF, and camera RAW are not decoded. AVIF would need `image`'s
  `avif-native` feature and a system libdav1d found by pkg-config (the pure-Rust
  `avif` feature only encodes); HEIC needs the native libheif library; RAW would
  need `rawler` (LGPL-2.1) plus demosaicing. These dependencies could not be
  verified on the development machine, so no optional features were added.
- The viewer is a single-image overlay, not a library manager: it does not index
  folders recursively or maintain a database.
