# picasa-rs

A Picasa-inspired quick photo viewer for the desktop: a borderless, fullscreen
overlay that dims the desktop behind the photo, opens quickly, and lets you
scrub through a folder with the arrow keys.

Built with Rust, [`winit`](https://crates.io/crates/winit),
[`wgpu`](https://crates.io/crates/wgpu), and [`egui`](https://crates.io/crates/egui).

## Features

- **Transparent desktop backdrop** — the desktop stays visible through a 75%
  black scrim, so the photo floats over your workspace instead of replacing it.
- **Instant display** — the image is decoded on a background worker and uploaded
  straight to the GPU; the window is presented as soon as the surface is ready.
- **Cursor-anchored zoom and pan** — the point under the pointer stays put while
  zooming. Motion is critically damped and frame-rate independent.
- **EXIF-aware orientation** — all eight EXIF orientation values are applied
  correctly. Full-resolution photos are oriented on the GPU; thumbnails are
  oriented on the CPU.
- **Folder navigation** — `Left` / `Right` wrap around the folder, with natural
  filename sorting and neighbor prefetching for near-instant paging.
- **Filmstrip and HUD** — a thumbnail strip and a compact control bar appear on
  mouse movement and fade away when idle.
- **Large images** — mipmaps are generated on the GPU, so multi-megapixel photos
  downscale cleanly and quickly.
- **View rotation** — rotate the photo by 90° without re-decoding it.

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
| Drag | Pan |
| Double-click / `1` | Toggle fit and 1:1 |
| `0` / `Space` | Reset the view |
| `R` / `L` | Rotate the view clockwise / counter-clockwise |
| `H` | Show the help hint |
| `Esc` / `Q` | Close |
| Click outside the photo | Close |

## Transparency on Windows

The window is a borderless fullscreen `winit` window created with
`with_transparent(true)`. On Windows the renderer forces the DX12 backend and
selects wgpu's DirectComposition presentation path
(`wgpu::Dx12SwapchainKind::DxgiFromVisual`). Unlike a plain HWND swapchain,
which only advertises `CompositeAlphaMode::Opaque`, a DirectComposition visual
supports `PreMultiplied` alpha. The renderer:

1. clears the swapchain to premultiplied black with alpha `0.75`;
2. draws the photo and egui overlay with premultiplied-alpha blending;
3. lets DWM composite the result over the desktop.

The result is true per-pixel transparency: the desktop is visible at 25% of its
original brightness around the photo. Clicking in that dimmed area closes the
viewer, matching Picasa's behavior. To change the dim level, edit
`DIM_BACKDROP_ALPHA` in `src/renderer/mod.rs` (`0.0` is fully clear, `1.0` is
fully opaque).

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

## Packaging

Helper files live in `packaging/`:

- `packaging/windows/install.ps1` — copies the release executable to
  `%LOCALAPPDATA%\Programs\picasa-rs`, registers a per-user ProgID, and
  associates common image extensions. No administrator rights required.
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
  app.rs                  window, input, HUD/filmstrip, frame loop
  loader/
    mod.rs                priority worker queue, prefetch, shutdown
    decoder.rs            image decode, EXIF orientation, thumbnails
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
  compositor's available alpha modes.
- Very large images are limited by the GPU's maximum texture size (typically
  8192 or 16384 px). Larger images are downscaled before upload.
- GIFs display their first frame only.
- The viewer is a single-image overlay, not a library manager: it does not index
  folders recursively or maintain a database.
