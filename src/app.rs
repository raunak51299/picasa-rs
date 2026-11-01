//! Application state machine and winit event handling.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::loader::decoder;
use crate::loader::{Loader, LoaderEvent};
use crate::preview::PreviewWindow;
use crate::renderer::{GpuPhoto, PhotoDraw, Renderer};
use crate::viewer::camera::{Camera, Vec2};
use crate::viewer::navigation::Navigation;

const FULL_PREFETCH_RADIUS: usize = 3;
const PHOTO_CACHE_RADIUS: usize = 4;
const THUMB_RADIUS: usize = 16;
const THUMB_CACHE_LIMIT: usize = 320;
const HUD_IDLE: Duration = Duration::from_millis(1600);
const PHOTO_FADE_SECONDS: f32 = 0.18;
const HELP_DURATION: Duration = Duration::from_secs(7);
const TOAST_DURATION: Duration = Duration::from_secs(6);

struct Toast {
    text: String,
    until: Instant,
}

pub enum UserEvent {
    /// A loader result is waiting in the loader channel.
    LoaderReady,
}

#[derive(Clone, Copy, Debug)]
enum UiAction {
    Navigate(usize),
    Fit,
    ActualSize,
    RotateLeft,
    RotateRight,
    ZoomIn,
    ZoomOut,
    SetZoom(f32),
    Close,
}

pub struct App {
    nav: Navigation,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// Native dim + photo preview shown until the GPU presents its first frame.
    preview: Option<PreviewWindow>,
    /// When to drop the preview after the first GPU present. It lingers for a
    /// few frames so DWM has composited the swapchain before the layered
    /// window disappears, otherwise the desktop can flash for one frame.
    preview_until: Option<Instant>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    loader: Option<Loader>,
    camera: Camera,
    photos: HashMap<PathBuf, GpuPhoto>,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    /// Prefetch and thumbnail results held back until the first photo is on
    /// screen, so their CPU/GPU uploads cannot delay the initial present.
    deferred_loader_events: Vec<LoaderEvent>,
    failed_full: HashMap<PathBuf, String>,
    failed_thumb: HashSet<PathBuf>,
    cursor: Option<PhysicalPosition<f64>>,
    dragging: bool,
    drag_last: Option<PhysicalPosition<f64>>,
    last_activity: Instant,
    last_frame: Instant,
    hud_alpha: f32,
    ui_hovered: bool,
    filmstrip_hovered: bool,
    scroll_filmstrip: bool,
    photo_fade: f32,
    photo_fade_start: Instant,
    show_help_until: Instant,
    toast: Option<Toast>,
    last_click: Option<(Instant, PhysicalPosition<f64>)>,
    quit_requested: bool,
    startup_photo_ready: bool,
    startup_photo_presented: bool,
}

impl App {
    pub fn new(initial_path: PathBuf, proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let scan_start = Instant::now();
        let nav = Navigation::new(&initial_path).with_context(|| {
            format!(
                "could not build a folder listing for {}",
                initial_path.display()
            )
        })?;
        log::info!(
            "startup[{:.1}ms]: folder scanned in {:.1}ms ({} images)",
            crate::startup_ms(),
            scan_start.elapsed().as_secs_f64() * 1000.0,
            nav.len()
        );

        let egui_ctx = egui::Context::default();
        egui_ctx.set_theme(egui::Theme::Dark);
        egui_ctx.all_styles_mut(|style| {
            style.visuals = egui::Visuals::dark();
            style.visuals.override_text_color = Some(egui::Color32::from_rgb(232, 234, 240));
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
        });

        // Start the first decode before the window and GPU exist. Adapter and
        // device creation can take hundreds of milliseconds; decoding in
        // parallel hides most of that latency. 8192 is a safe initial texture
        // limit and is replaced with the real GPU limit once it is known.
        let loader = Loader::new(proxy.clone(), 8192);
        loader.set_current(nav.current());
        loader.request_full(nav.current_path(), nav.current(), 3);
        log::info!(
            "startup[{:.1}ms]: loader started, current image queued",
            crate::startup_ms()
        );

        let now = Instant::now();

        Ok(Self {
            nav,
            window: None,
            renderer: None,
            preview: None,
            preview_until: None,
            egui_ctx,
            egui_state: None,
            loader: Some(loader),
            camera: Camera::default(),
            photos: HashMap::new(),
            thumbs: HashMap::new(),
            deferred_loader_events: Vec::new(),
            failed_full: HashMap::new(),
            failed_thumb: HashSet::new(),
            cursor: None,
            dragging: false,
            drag_last: None,
            last_activity: now,
            last_frame: now,
            hud_alpha: 1.0,
            ui_hovered: false,
            filmstrip_hovered: false,
            scroll_filmstrip: true,
            photo_fade: 1.0,
            photo_fade_start: now,
            show_help_until: now + HELP_DURATION,
            toast: None,
            last_click: None,
            quit_requested: false,
            startup_photo_ready: false,
            startup_photo_presented: false,
        })
    }

    fn try_resumed(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if self.window.is_some() {
            return Ok(());
        }

        let (position, size) = match event_loop.primary_monitor() {
            Some(monitor) => {
                let size = monitor.size();
                (
                    Some(monitor.position()),
                    PhysicalSize::new(
                        size.width.saturating_sub(1).max(1),
                        size.height.saturating_sub(1).max(1),
                    ),
                )
            }
            None => (None, PhysicalSize::new(1280, 800)),
        };

        let preview_position = position.unwrap_or(PhysicalPosition::new(0, 0));
        match PreviewWindow::show(
            self.nav.current_path(),
            preview_position.x,
            preview_position.y,
            size.width,
            size.height,
        ) {
            Ok(preview) => self.preview = Some(preview),
            Err(error) => log::warn!("could not show the startup preview: {error:#}"),
        }

        let mut attributes = Window::default_attributes()
            .with_title("picasa-rs")
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(false)
            .with_resizable(false)
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_inner_size(size);
        if let Some(position) = position {
            attributes = attributes.with_position(position);
        }

        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("could not create the viewer window")?,
        );

        let title_name = self
            .nav
            .current_path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        window.set_title(&format!("{title_name} — picasa-rs"));

        log::info!("startup[{:.1}ms]: window created", crate::startup_ms());
        let renderer =
            Renderer::new(window.clone()).context("could not initialize the wgpu renderer")?;
        log::info!("startup[{:.1}ms]: renderer ready", crate::startup_ms());

        let max_texture_side = renderer.max_texture_side();
        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_texture_side as usize),
        );

        log::info!("startup[{:.1}ms]: egui state ready", crate::startup_ms());

        self.camera
            .set_viewport(size.width as f32, size.height as f32);

        self.window = Some(window.clone());
        self.renderer = Some(renderer);
        self.egui_state = Some(egui_state);

        if let Some(loader) = &self.loader {
            loader.set_max_texture_dim(max_texture_side);
            loader.set_current(self.nav.current());
        }

        // The first decode was started before the window existed and has
        // almost certainly finished by now. Drain it before the first present
        // so the window never appears without its photo.
        self.drain_loader_events();
        self.on_current_changed();

        // Acquiring a surface image for a hidden window can block on some
        // Vulkan drivers, so make the window visible before the first present.
        log::info!("startup[{:.1}ms]: showing window", crate::startup_ms());
        window.set_visible(true);
        log::info!(
            "startup[{:.1}ms]: window visible: {:?}",
            crate::startup_ms(),
            window.is_visible()
        );
        window.request_redraw();
        self.last_frame = Instant::now();
        log::info!(
            "startup[{:.1}ms]: load requests queued",
            crate::startup_ms()
        );

        Ok(())
    }

    fn request_window(&mut self) {
        let Some(loader) = &self.loader else {
            return;
        };

        let current = self.nav.current();
        loader.set_current(current);

        for (rank, index) in self
            .nav
            .indices_by_distance(FULL_PREFETCH_RADIUS)
            .into_iter()
            .enumerate()
        {
            let Some(path) = self.nav.path(index) else {
                continue;
            };
            if self.photos.contains_key(path) || self.failed_full.contains_key(path) {
                continue;
            }
            let priority = match rank {
                0 => 3,
                1 | 2 => 2,
                _ => 1,
            };
            loader.request_full(path, index, priority);
        }

        for index in self.nav.indices_by_distance(THUMB_RADIUS) {
            let Some(path) = self.nav.path(index) else {
                continue;
            };
            if self.thumbs.contains_key(path) || self.failed_thumb.contains(path) {
                continue;
            }
            loader.request_thumb(path, index, 0);
        }
    }

    fn on_current_changed(&mut self) {
        if let Some(loader) = &self.loader {
            loader.set_current(self.nav.current());
        }

        self.scroll_filmstrip = true;
        self.last_click = None;

        let path = self.nav.current_path().to_path_buf();
        if let Some(photo) = self.photos.get(&path) {
            self.camera
                .set_image(photo.display_width as f32, photo.display_height as f32);
            self.photo_fade = 0.0;
            self.photo_fade_start = Instant::now();
        } else {
            self.camera.clear_image();
            self.photo_fade = 1.0;
        }

        self.request_window();
        self.record_activity();

        if let Some(window) = &self.window {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            window.set_title(&format!("{name} — picasa-rs"));
            window.request_redraw();
        }
    }

    fn navigate_by(&mut self, delta: isize) {
        if self.nav.step(delta) {
            self.on_current_changed();
        }
    }

    fn navigate_to(&mut self, index: usize) {
        if self.nav.set_current(index) {
            self.on_current_changed();
        }
    }

    fn drain_loader_events(&mut self) {
        if self.renderer.is_none() || self.window.is_none() {
            return;
        }
        let events = match &self.loader {
            Some(loader) => loader.drain_events(),
            None => return,
        };
        for event in events {
            self.on_loader_event(event);
        }
    }

    fn on_loader_event(&mut self, event: LoaderEvent) {
        if !self.startup_photo_presented {
            let is_current_full = matches!(
                &event,
                LoaderEvent::Full(full) if full.index == self.nav.current()
            );
            if !is_current_full {
                self.deferred_loader_events.push(event);
                return;
            }
        }

        match event {
            LoaderEvent::Full(full) => {
                let index = full.index;
                let path = full.path.clone();

                if self.photos.contains_key(&path) {
                    return;
                }

                if !self.nav.is_near(index, PHOTO_CACHE_RADIUS + 1) {
                    return;
                }

                let Some(renderer) = &self.renderer else {
                    return;
                };
                let max_texture_side = renderer.max_texture_side();
                let full = if full.width > max_texture_side || full.height > max_texture_side {
                    log::warn!(
                        "{} exceeds the GPU texture limit ({}px); downscaling",
                        full.path.display(),
                        max_texture_side
                    );
                    decoder::downscale_full(full, max_texture_side)
                } else {
                    full
                };
                let gpu = renderer.create_photo(&full);

                let is_current = index == self.nav.current();
                let display_width = gpu.display_width as f32;
                let display_height = gpu.display_height as f32;

                self.photos.insert(path.clone(), gpu);
                self.failed_full.remove(&path);

                if is_current {
                    self.camera.set_image(display_width, display_height);
                    if self.startup_photo_ready {
                        self.photo_fade = 0.0;
                        self.photo_fade_start = Instant::now();
                    } else {
                        // The native preview already shows this photo. Present
                        // the GPU version at full opacity so the hand-off is
                        // seamless instead of fading in from the backdrop.
                        self.photo_fade = 1.0;
                        self.startup_photo_ready = true;
                        log::info!(
                            "startup[{:.1}ms]: current photo uploaded",
                            crate::startup_ms()
                        );
                    }
                }

                self.evict_photo_cache();
                self.request_window();
            }
            LoaderEvent::Thumb(thumb) => {
                if !self.nav.is_near(thumb.index, THUMB_RADIUS + 8) {
                    return;
                }
                if self.thumbs.contains_key(&thumb.path) {
                    return;
                }

                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [thumb.width as usize, thumb.height as usize],
                    &thumb.rgba,
                );
                let handle = self.egui_ctx.load_texture(
                    format!("thumb:{}", thumb.path.display()),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbs.insert(thumb.path.clone(), handle);
                self.failed_thumb.remove(&thumb.path);
                self.evict_thumb_cache();
            }
            LoaderEvent::Error {
                path,
                index,
                kind,
                message,
            } => {
                match kind {
                    crate::loader::JobKind::Full => {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        self.toast = Some(Toast {
                            text: truncate(&format!("Could not open {name}: {message}"), 150),
                            until: Instant::now() + TOAST_DURATION,
                        });
                        self.failed_full.insert(path, message);
                        if index == self.nav.current() {
                            // Keep the error card on screen rather than
                            // presenting an empty backdrop.
                            self.camera.clear_image();
                        }
                    }
                    crate::loader::JobKind::Thumb => {
                        self.failed_thumb.insert(path);
                    }
                }
                self.request_window();
            }
            LoaderEvent::Stale { path, index, kind } => {
                log::trace!("skipped stale job for {}", path.display());
                let needed = match kind {
                    crate::loader::JobKind::Full => index == self.nav.current(),
                    crate::loader::JobKind::Thumb => self.nav.is_near(index, THUMB_RADIUS),
                };
                if needed {
                    self.request_window();
                }
            }
        }

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn evict_photo_cache(&mut self) {
        let current = self.nav.current();
        let entries: Vec<(PathBuf, usize)> = self
            .photos
            .keys()
            .map(|path| (path.clone(), self.nav.index_of(path).unwrap_or(usize::MAX)))
            .collect();
        let limit = PHOTO_CACHE_RADIUS * 2 + 1;
        for path in eviction_targets(&entries, current, limit) {
            self.photos.remove(&path);
        }
    }

    fn evict_thumb_cache(&mut self) {
        let current = self.nav.current();
        let entries: Vec<(PathBuf, usize)> = self
            .thumbs
            .keys()
            .map(|path| (path.clone(), self.nav.index_of(path).unwrap_or(usize::MAX)))
            .collect();
        for path in eviction_targets(&entries, current, THUMB_CACHE_LIMIT) {
            // Dropping the handle frees the egui texture.
            self.thumbs.remove(&path);
        }
    }

    fn current_photo(&self) -> Option<&GpuPhoto> {
        self.photos.get(self.nav.current_path())
    }

    fn record_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    fn hud_target(&self) -> f32 {
        if self.ui_hovered || self.dragging || self.last_activity.elapsed() < HUD_IDLE {
            1.0
        } else {
            0.0
        }
    }

    fn viewport_center(&self) -> Vec2 {
        Vec2::new(self.camera.viewport.x * 0.5, self.camera.viewport.y * 0.5)
    }

    fn cursor_point(&self) -> Option<Vec2> {
        self.cursor
            .map(|position| Vec2::new(position.x as f32, position.y as f32))
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, code: KeyCode, repeat: bool) {
        match code {
            KeyCode::Escape | KeyCode::KeyQ => {
                if !repeat {
                    event_loop.exit();
                }
            }
            KeyCode::ArrowLeft => self.navigate_by(-1),
            KeyCode::ArrowRight => self.navigate_by(1),
            KeyCode::Home => {
                if self.nav.first() {
                    self.on_current_changed();
                }
            }
            KeyCode::End => {
                if self.nav.last() {
                    self.on_current_changed();
                }
            }
            KeyCode::Space | KeyCode::Digit0 | KeyCode::Numpad0 => {
                if !repeat {
                    self.camera.reset_fit();
                }
            }
            KeyCode::Digit1 | KeyCode::Numpad1 => {
                if !repeat {
                    let anchor = self
                        .cursor_point()
                        .unwrap_or_else(|| self.viewport_center());
                    self.camera.toggle_actual_size(anchor);
                }
            }
            KeyCode::KeyR => {
                if !repeat {
                    self.camera.rotate(std::f32::consts::FRAC_PI_2);
                }
            }
            KeyCode::KeyL => {
                if !repeat {
                    self.camera.rotate(-std::f32::consts::FRAC_PI_2);
                }
            }
            KeyCode::KeyH => {
                if !repeat {
                    self.show_help_until = Instant::now() + HELP_DURATION;
                }
            }
            KeyCode::F5 => {
                if !repeat {
                    self.failed_full.clear();
                    self.failed_thumb.clear();
                    self.request_window();
                }
            }
            KeyCode::Equal | KeyCode::NumpadAdd => {
                let anchor = self
                    .cursor_point()
                    .unwrap_or_else(|| self.viewport_center());
                self.camera.zoom_at(anchor, 1.2);
            }
            KeyCode::Minus | KeyCode::NumpadSubtract => {
                let anchor = self
                    .cursor_point()
                    .unwrap_or_else(|| self.viewport_center());
                self.camera.zoom_at(anchor, 1.0 / 1.2);
            }
            _ => {}
        }

        self.record_activity();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn apply_action(&mut self, action: UiAction) {
        match action {
            UiAction::Navigate(index) => self.navigate_to(index),
            UiAction::Fit => self.camera.reset_fit(),
            UiAction::ActualSize => self.camera.toggle_actual_size(self.viewport_center()),
            UiAction::RotateLeft => self.camera.rotate(-std::f32::consts::FRAC_PI_2),
            UiAction::RotateRight => self.camera.rotate(std::f32::consts::FRAC_PI_2),
            UiAction::ZoomIn => self.camera.zoom_at(self.viewport_center(), 1.25),
            UiAction::ZoomOut => self.camera.zoom_at(self.viewport_center(), 0.8),
            UiAction::SetZoom(scale) => self
                .camera
                .set_target_scale_at(scale, self.viewport_center()),
            UiAction::Close => self.quit_requested = true,
        }

        self.record_activity();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn render(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.renderer.is_none() || self.egui_state.is_none() {
            return;
        }

        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
        self.last_frame = now;

        self.camera.update(dt);

        let hud_target = self.hud_target();
        let hud_t = 1.0 - (-12.0 * dt).exp();
        self.hud_alpha += (hud_target - self.hud_alpha) * hud_t;
        if (self.hud_alpha - hud_target).abs() < 0.002 {
            self.hud_alpha = hud_target;
        }

        if self.current_photo().is_some() {
            let elapsed = now.duration_since(self.photo_fade_start).as_secs_f32();
            let linear = (elapsed / PHOTO_FADE_SECONDS).clamp(0.0, 1.0);
            self.photo_fade = 1.0 - (1.0 - linear).powi(3);
        } else {
            self.photo_fade = 1.0;
        }

        // On the first frame with a photo, skip egui entirely. That lets the
        // image appear before the HUD's shader pipeline and font atlas are
        // built; the next frame draws the chrome over the already-visible
        // photo.
        let skip_ui = self.startup_photo_ready
            && !self.startup_photo_presented
            && self.current_photo().is_some();
        let mut paint_jobs = Vec::new();
        let mut textures_delta = egui::TexturesDelta::default();
        let mut pixels_per_point = self.egui_ctx.pixels_per_point();

        if !skip_ui {
            let egui_ctx = self.egui_ctx.clone();
            let raw_input = self
                .egui_state
                .as_mut()
                .expect("egui state was checked above")
                .take_egui_input(&window);
            let mut actions = Vec::new();
            let full_output = egui_ctx.run_pass(raw_input, |ctx| {
                actions = self.build_ui(ctx);
            });

            let egui::FullOutput {
                platform_output,
                textures_delta: delta,
                shapes,
                pixels_per_point: scale,
                ..
            } = full_output;

            for action in actions {
                self.apply_action(action);
            }

            paint_jobs = egui_ctx.tessellate(shapes, scale);
            textures_delta = delta;
            pixels_per_point = scale;

            self.egui_state
                .as_mut()
                .expect("egui state was checked above")
                .handle_platform_output(&window, platform_output);
            self.ui_hovered = egui_ctx.egui_wants_pointer_input();
        } else {
            self.ui_hovered = false;
        }

        let size = window.inner_size();
        let screen = ScreenDescriptor {
            size_in_pixels: [size.width.max(1), size.height.max(1)],
            pixels_per_point,
        };

        let center = self.camera.screen_center();
        let half_size = self.camera.screen_half_size();
        let angle = self.camera.angle;
        let alpha = self.photo_fade;
        let current_path = self.nav.current_path();

        let render_result = {
            let photo = self.photos.get(current_path).map(|photo| PhotoDraw {
                photo,
                center: [center.x, center.y],
                half_size: [half_size.x, half_size.y],
                angle,
                alpha,
            });

            self.renderer
                .as_mut()
                .expect("renderer was checked above")
                .render(photo, &paint_jobs, textures_delta, screen)
        };
        if let Err(error) = render_result {
            log::error!("rendering failed: {error:#}");
            event_loop.exit();
        } else if !self.startup_photo_presented && alpha > 0.0 && self.current_photo().is_some() {
            self.startup_photo_presented = true;
            log::info!("startup[{:.1}ms]: photo presented", crate::startup_ms());
            self.preview_until = Some(Instant::now() + Duration::from_millis(120));
            if skip_ui {
                window.request_redraw();
            }

            // The first image is visible. Now it is safe to upload the
            // prefetched neighbours without delaying the initial present.
            let deferred = std::mem::take(&mut self.deferred_loader_events);
            for event in deferred {
                self.on_loader_event(event);
            }
        }
    }

    fn build_ui(&mut self, ctx: &egui::Context) -> Vec<UiAction> {
        let mut actions = Vec::new();
        let now = Instant::now();
        let screen = ctx.viewport_rect();
        let screen_size = screen.size();
        let current_path = self.nav.current_path().to_path_buf();
        let current_index = self.nav.current();
        let loading =
            self.current_photo().is_none() && !self.failed_full.contains_key(&current_path);
        let error = self.failed_full.get(&current_path).cloned();
        let show_help = now < self.show_help_until;
        let toast = self
            .toast
            .as_ref()
            .filter(|toast| now < toast.until)
            .map(|toast| toast.text.clone());

        let show_controls = self.hud_alpha > 0.01;
        let pointer_near_bottom = self
            .cursor
            .map(|position| position.y as f32 > self.camera.viewport.y - 230.0)
            .unwrap_or(false);
        let show_filmstrip = show_controls && (pointer_near_bottom || self.filmstrip_hovered);

        if !show_controls && !loading && error.is_none() && !show_help && toast.is_none() {
            return actions;
        }

        if show_controls {
            let alpha = self.hud_alpha;
            let bottom_offset = if show_filmstrip { 108.0 } else { 18.0 };
            let photo = self.current_photo();
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dimensions = photo
                .map(|photo| format!("{} × {}", photo.display_width, photo.display_height))
                .unwrap_or_else(|| "—".to_string());
            let zoom = if self.camera.has_image() {
                if self.camera.fit_mode {
                    format!("Fit {:.0}%", self.camera.zoom_percent())
                } else {
                    format!("{:.0}%", self.camera.zoom_percent())
                }
            } else {
                "—".to_string()
            };
            let rotation = self.camera.rotation_degrees();
            let index_label = format!("{} / {}", current_index + 1, self.nav.len());

            egui::Area::new(egui::Id::unique("controls"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -bottom_offset))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((190.0 * alpha) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((22.0 * alpha) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(12, 7))
                        .show(ui, |ui| {
                            ui.set_opacity(alpha);
                            ui.spacing_mut().item_spacing.x = 7.0;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(truncate(&file_name, 32))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new(dimensions)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(160)),
                                );
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(index_label)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(185)),
                                );
                                ui.label(
                                    egui::RichText::new(zoom)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(185)),
                                );
                                if rotation != 0 {
                                    ui.label(
                                        egui::RichText::new(format!("{rotation}°"))
                                            .size(12.0)
                                            .color(egui::Color32::from_gray(185)),
                                    );
                                }
                                ui.separator();

                                if small_button(ui, "−").clicked() {
                                    actions.push(UiAction::ZoomOut);
                                }

                                let (min_zoom, max_zoom) = if self.camera.has_image() {
                                    (
                                        (self.camera.min_scale * 100.0).max(1.0),
                                        (self.camera.max_scale * 100.0).max(2.0),
                                    )
                                } else {
                                    (10.0, 400.0)
                                };
                                let mut zoom_value = if self.camera.has_image() {
                                    self.camera.zoom_percent().clamp(min_zoom, max_zoom)
                                } else {
                                    100.0
                                };
                                ui.spacing_mut().slider_width = 90.0;
                                let slider =
                                    egui::Slider::new(&mut zoom_value, min_zoom..=max_zoom)
                                        .logarithmic(true)
                                        .show_value(false);
                                if ui.add(slider).changed() {
                                    actions.push(UiAction::SetZoom(zoom_value / 100.0));
                                }

                                if small_button(ui, "+").clicked() {
                                    actions.push(UiAction::ZoomIn);
                                }

                                if small_button(ui, "Fit").clicked() {
                                    actions.push(UiAction::Fit);
                                }
                                if small_button(ui, "1:1").clicked() {
                                    actions.push(UiAction::ActualSize);
                                }
                                if small_button(ui, "Rot -").clicked() {
                                    actions.push(UiAction::RotateLeft);
                                }
                                if small_button(ui, "Rot +").clicked() {
                                    actions.push(UiAction::RotateRight);
                                }
                                if small_button(ui, "X").clicked() {
                                    actions.push(UiAction::Close);
                                }
                            });
                        });
                });
        }

        if show_filmstrip {
            let alpha = self.hud_alpha;
            let mut scroll_to_current = self.scroll_filmstrip;
            let mut hovered = false;
            let start = current_index.saturating_sub(THUMB_RADIUS);
            let end = (current_index + THUMB_RADIUS + 1).min(self.nav.len());
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

            egui::Area::new(egui::Id::unique("filmstrip"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -12.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((180.0 * alpha) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((18.0 * alpha) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(9, 7))
                        .show(ui, |ui| {
                            ui.set_opacity(alpha);
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                            egui::ScrollArea::horizontal()
                                .id_salt("filmstrip_scroll")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .max_height(76.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for index in start..end {
                                            let Some(path) = self.nav.path(index) else {
                                                continue;
                                            };
                                            let selected = index == current_index;
                                            let (rect, response) = ui.allocate_exact_size(
                                                egui::vec2(64.0, 64.0),
                                                egui::Sense::click(),
                                            );

                                            if let Some(texture) = self.thumbs.get(path) {
                                                let texture_size = texture.size_vec2();
                                                let scale = (64.0 / texture_size.x.max(1.0))
                                                    .min(64.0 / texture_size.y.max(1.0))
                                                    .min(1.0);
                                                let draw_size = texture_size * scale;
                                                let draw_rect = egui::Rect::from_center_size(
                                                    rect.center(),
                                                    draw_size,
                                                );
                                                ui.painter().image(
                                                    texture.id(),
                                                    draw_rect,
                                                    uv,
                                                    egui::Color32::WHITE,
                                                );
                                            } else {
                                                ui.painter().rect_filled(
                                                    rect,
                                                    6.0,
                                                    egui::Color32::from_gray(28),
                                                );
                                                let extension = path
                                                    .extension()
                                                    .and_then(|extension| extension.to_str())
                                                    .unwrap_or("?")
                                                    .to_uppercase();
                                                ui.painter().text(
                                                    rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    extension,
                                                    egui::FontId::proportional(9.0),
                                                    egui::Color32::from_gray(105),
                                                );
                                            }

                                            if selected {
                                                ui.painter().rect_stroke(
                                                    rect.expand(1.0),
                                                    7.0,
                                                    egui::Stroke::new(
                                                        2.0,
                                                        egui::Color32::from_rgb(118, 186, 255),
                                                    ),
                                                    egui::StrokeKind::Outside,
                                                );
                                            } else if response.hovered() {
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    6.0,
                                                    egui::Stroke::new(
                                                        1.0,
                                                        egui::Color32::from_white_alpha(55),
                                                    ),
                                                    egui::StrokeKind::Inside,
                                                );
                                            }

                                            let clicked = response.clicked();
                                            let item_hovered = response.hovered();
                                            hovered |= item_hovered;

                                            if selected && scroll_to_current {
                                                response.scroll_to_me(Some(egui::Align::Center));
                                                scroll_to_current = false;
                                            }

                                            if clicked {
                                                actions.push(UiAction::Navigate(index));
                                            }

                                            let name = path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .to_string();
                                            response.on_hover_text(name);
                                        }
                                    });
                                });
                        });
                });

            self.scroll_filmstrip = scroll_to_current;
            self.filmstrip_hovered = hovered;
        } else {
            self.filmstrip_hovered = false;
        }

        if loading {
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let thumb = self.thumbs.get(&current_path);
            egui::Area::new(egui::Id::unique("loading"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(150))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                if let Some(texture) = thumb {
                                    let texture_size = texture.size_vec2();
                                    let max_width = screen_size.x * 0.36;
                                    let max_height = screen_size.y * 0.36;
                                    let scale = (max_width / texture_size.x.max(1.0))
                                        .min(max_height / texture_size.y.max(1.0))
                                        .min(1.0);
                                    let draw_size = texture_size * scale;
                                    let (rect, _) =
                                        ui.allocate_exact_size(draw_size, egui::Sense::hover());
                                    ui.painter().image(
                                        texture.id(),
                                        rect,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::from_white_alpha(190),
                                    );
                                    ui.add_space(10.0);
                                }
                                ui.add(
                                    egui::Spinner::new()
                                        .size(22.0)
                                        .color(egui::Color32::from_rgb(118, 186, 255)),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(format!("Loading {file_name}…"))
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(200)),
                                );
                            });
                        });
                });
        }

        if let Some(message) = error {
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            egui::Area::new(egui::Id::unique("error"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(190))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(180, 80, 80)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                egui::RichText::new(format!("Could not open {file_name}"))
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(message)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                )
                                .wrap(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Press Esc to close, or F5 to retry.")
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(130)),
                            );
                        });
                });
        }

        if let Some(text) = toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 64.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(200))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(190, 92, 92)))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(520.0);
                            ui.label(
                                egui::RichText::new(text)
                                    .size(12.0)
                                    .color(egui::Color32::from_gray(225)),
                            );
                        });
                });
        }

        if show_help {
            let remaining = self.show_help_until.saturating_duration_since(now);
            let fade = (remaining.as_secs_f32() / 0.6).clamp(0.0, 1.0);
            egui::Area::new(egui::Id::unique("help"))
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 22.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((155.0 * fade) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((18.0 * fade) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_opacity(fade);
                            ui.label(
                                egui::RichText::new(
                                    "Scroll: zoom   |   Drag: pan   |   Left/Right: browse   |   R/L: rotate   |   Esc: close",
                                )
                                .size(12.0)
                                .color(egui::Color32::from_gray(215)),
                            );
                        });
                });
        }

        actions
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.try_resumed(event_loop) {
            log::error!("{error:#}");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let response = match (&self.window, &mut self.egui_state) {
            (Some(window), Some(state)) => state.on_window_event(window, &event),
            _ => egui_winit::EventResponse::default(),
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size);
                }
                self.camera
                    .set_viewport(size.width as f32, size.height as f32);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    if let Some(renderer) = &mut self.renderer {
                        renderer.resize(size);
                    }
                    self.camera
                        .set_viewport(size.width as f32, size.height as f32);
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.render(event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                log::debug!("cursor moved to {position:?}");
                self.cursor = Some(position);
                self.record_activity();

                if self.dragging {
                    if let Some(last) = self.drag_last {
                        let delta =
                            Vec2::new((position.x - last.x) as f32, (position.y - last.y) as f32);
                        if delta.length() > 0.01 {
                            self.camera.pan_by(delta);
                        }
                    }
                    self.drag_last = Some(position);
                }

                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.dragging = false;
                self.drag_last = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !response.consumed && self.camera.has_image() {
                    let factor = match delta {
                        MouseScrollDelta::LineDelta(_, y) => 1.18_f32.powf(y),
                        MouseScrollDelta::PixelDelta(position) => {
                            (position.y as f32 * 0.0025).exp()
                        }
                    };
                    if factor.is_finite() && factor > 0.0 {
                        let anchor = self
                            .cursor_point()
                            .unwrap_or_else(|| self.viewport_center());
                        self.camera.zoom_at(anchor, factor);
                        self.record_activity();
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                if !response.consumed && self.camera.has_image() {
                    let factor = (1.0 + delta as f32).clamp(0.1, 10.0);
                    let anchor = self
                        .cursor_point()
                        .unwrap_or_else(|| self.viewport_center());
                    self.camera.zoom_at(anchor, factor);
                    self.record_activity();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            if !response.consumed {
                                self.handle_primary_press(event_loop);
                            }
                        }
                        ElementState::Released => {
                            self.dragging = false;
                            self.drag_last = None;
                            if let Some(window) = &self.window {
                                window.set_cursor(CursorIcon::Default);
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && !response.consumed
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    self.handle_key(event_loop, code, event.repeat);
                }
            }
            WindowEvent::Focused(false) => {
                self.dragging = false;
                self.drag_last = None;
            }
            WindowEvent::Occluded(true) => {}
            _ => {}
        }

        if response.repaint
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::LoaderReady => self.drain_loader_events(),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit_requested {
            event_loop.exit();
            return;
        }

        let now = Instant::now();
        if let Some(until) = self.preview_until
            && now >= until
        {
            self.preview = None;
            self.preview_until = None;
        }
        let hud_deadline = self.last_activity + HUD_IDLE;
        let help_deadline = self.show_help_until;

        let animating = self.camera.is_animating()
            || (self.hud_alpha - self.hud_target()).abs() > 0.002
            || (self.current_photo().is_some() && self.photo_fade < 0.999)
            || self.egui_ctx.has_requested_repaint();

        if animating {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(8)));
            return;
        }

        let mut next_deadline: Option<Instant> = self.preview_until;
        if self.hud_alpha > 0.001 && hud_deadline > now {
            next_deadline = Some(match next_deadline {
                Some(existing) => existing.min(hud_deadline),
                None => hud_deadline,
            });
        }
        if now < help_deadline {
            next_deadline = Some(match next_deadline {
                Some(existing) => existing.min(help_deadline),
                None => help_deadline,
            });
        }
        if let Some(toast) = &self.toast
            && now < toast.until
        {
            next_deadline = Some(match next_deadline {
                Some(existing) => existing.min(toast.until),
                None => toast.until,
            });
        }

        if let Some(deadline) = next_deadline {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl App {
    fn handle_primary_press(&mut self, event_loop: &ActiveEventLoop) {
        let Some(position) = self.cursor else {
            return;
        };
        let point = Vec2::new(position.x as f32, position.y as f32);
        let now = Instant::now();

        let double_click = self.last_click.is_some_and(|(last_time, last_position)| {
            now.duration_since(last_time) < Duration::from_millis(380)
                && (last_position.x - position.x).abs() < 6.0
                && (last_position.y - position.y).abs() < 6.0
        });
        self.last_click = Some((now, position));

        if double_click {
            if self.camera.has_image() {
                self.camera.toggle_actual_size(point);
                self.dragging = false;
                self.record_activity();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }

        if self.camera.has_image() && !self.camera.contains_screen_point(point) {
            event_loop.exit();
            return;
        }

        if self.camera.has_image() {
            self.dragging = true;
            self.drag_last = Some(position);
            if let Some(window) = &self.window {
                window.set_cursor(CursorIcon::Grabbing);
            }
            self.record_activity();
        }
    }
}

/// Return the cached paths to drop so at most `limit` entries remain, furthest
/// from `current` first. Paths that are no longer in the folder sort last.
fn eviction_targets(entries: &[(PathBuf, usize)], current: usize, limit: usize) -> Vec<PathBuf> {
    if entries.len() <= limit {
        return Vec::new();
    }

    let mut ranked: Vec<&(PathBuf, usize)> = entries.iter().collect();
    ranked.sort_by_key(|(path, index)| (std::cmp::Reverse(index.abs_diff(current)), path.clone()));

    ranked
        .into_iter()
        .take(entries.len() - limit)
        .map(|(path, _)| path.clone())
        .collect()
}

fn small_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(12.0))
            .fill(egui::Color32::from_white_alpha(14))
            .corner_radius(egui::CornerRadius::same(9))
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut result: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::eviction_targets;
    use std::path::PathBuf;

    fn entries(indices: &[usize]) -> Vec<(PathBuf, usize)> {
        indices
            .iter()
            .map(|&index| (PathBuf::from(format!("photo{index}.jpg")), index))
            .collect()
    }

    #[test]
    fn eviction_drops_the_furthest_entries_first() {
        let cached = entries(&[0, 1, 5, 9]);
        assert_eq!(
            eviction_targets(&cached, 0, 2),
            [PathBuf::from("photo9.jpg"), PathBuf::from("photo5.jpg")]
        );
    }

    #[test]
    fn eviction_does_nothing_within_the_limit() {
        let cached = entries(&[0, 1, 2]);
        assert!(eviction_targets(&cached, 1, 3).is_empty());
        assert!(eviction_targets(&cached, 1, 10).is_empty());
    }
}
