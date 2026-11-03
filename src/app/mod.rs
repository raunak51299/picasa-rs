//! Application state machine and winit event handling.

mod animation;
mod cache;
mod events;
mod input;
pub(crate) mod slideshow;
mod ui;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use egui_wgpu::ScreenDescriptor;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::adjust::Adjustments;
use crate::animation::Playback;
use crate::info::{self, PhotoInfo};
use crate::loader::{Loader, LoaderEvent};
use crate::preferences::Preferences;
use crate::preview::PreviewWindow;
use crate::renderer::{GpuPhoto, PhotoDraw, Renderer};
use crate::save::SaveMode;
use crate::viewer::camera::{Camera, Vec2};
use crate::viewer::navigation::Navigation;

use self::slideshow::Slideshow;

const FULL_PREFETCH_RADIUS: usize = 3;
const PHOTO_CACHE_RADIUS: usize = 4;
/// Upper bound on cached photo textures (about 1 GiB of GPU memory).
const PHOTO_CACHE_BYTE_BUDGET: u64 = 1 << 30;
const THUMB_RADIUS: usize = 16;
const THUMB_CACHE_LIMIT: usize = 320;
const HUD_IDLE: Duration = Duration::from_millis(3000);
/// Physical pixels the pointer must travel to wake the idle HUD. Smaller
/// jitter must not bring the control bar back.
const HUD_WAKE_DISTANCE: f64 = 12.0;
const PHOTO_FADE_SECONDS: f32 = 0.18;
const HELP_DURATION: Duration = Duration::from_secs(7);
const TOAST_DURATION: Duration = Duration::from_secs(6);

struct Toast {
    text: String,
    until: Instant,
}

struct SaveRequest {
    path: PathBuf,
    adjustments: Adjustments,
}

pub enum UserEvent {
    /// A loader result is waiting in the loader channel.
    LoaderReady,
    /// Another launch asked this viewer to come to the foreground.
    Focus,
    /// Another launch asked this viewer to open a file or directory.
    OpenPath(PathBuf),
    /// A background save or copy finished.
    JobDone,
}

/// Result of slow file work that ran on a worker thread.
pub(crate) enum JobResult {
    Saved {
        name: String,
        result: Result<crate::save::SavedFile>,
    },
    Copied {
        path: PathBuf,
        name: String,
        result: Result<()>,
    },
}

#[derive(Clone, Copy, Debug)]
enum UiAction {
    Navigate(usize),
    Fit,
    ActualSize,
    RotateView(bool),
    Delete,
    ConfirmDelete,
    CancelDelete,
    ToggleInfo,
    ToggleFixes,
    ToggleShare,
    CopyImage,
    CopyPath,
    OpenDefault,
    Reveal,
    Print,
    SetAdjustments(Adjustments),
    Save {
        mode: SaveMode,
        adjustments: Adjustments,
    },
    ConfirmSave,
    CancelSave,
    ZoomIn,
    ZoomOut,
    SetZoom(f32),
    ToggleSlideshow,
    SlideshowSlower,
    SlideshowFaster,
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
    preview_drop_pending: bool,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    loader: Option<Loader>,
    camera: Camera,
    /// Quarter turns applied to each photo for this run only. The file's EXIF
    /// orientation is never rewritten.
    session_rotation: HashMap<PathBuf, u8>,
    photos: HashMap<PathBuf, GpuPhoto>,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    /// Animation of the current photo, if it has more than one frame.
    animation: Option<Playback>,
    /// GIF/WebP files known to contain a single frame.
    no_animation: HashSet<PathBuf>,
    /// Files whose animation failed to decode; F5 retries them.
    failed_animation: HashSet<PathBuf>,
    /// Prefetch and thumbnail results held back until the first photo is on
    /// screen, so their CPU/GPU uploads cannot delay the initial present.
    deferred_loader_events: Vec<LoaderEvent>,
    failed_full: HashMap<PathBuf, String>,
    failed_thumb: HashSet<PathBuf>,
    /// Files whose cached pixels must be replaced after a save.
    reload_full: HashSet<PathBuf>,
    reload_thumb: HashSet<PathBuf>,
    cursor: Option<PhysicalPosition<f64>>,
    dragging: bool,
    drag_last: Option<PhysicalPosition<f64>>,
    last_activity: Instant,
    last_frame: Instant,
    hud_alpha: f32,
    /// False until the first egui frame has been presented; that frame is
    /// slow, so the HUD fade only starts afterwards.
    hud_warmed: bool,
    hud_intro_done: bool,
    /// Pointer position where the HUD was last kept awake. The HUD only
    /// reappears once the pointer moves away from it by a visible amount.
    hud_anchor: Option<PhysicalPosition<f64>>,
    ui_hovered: bool,
    /// True while the pointer is over a borderless-window resize edge.
    resize_hovered: bool,
    scroll_filmstrip: bool,
    photo_fade: f32,
    photo_fade_start: Instant,
    show_help_until: Instant,
    toast: Option<Toast>,
    pending_delete: Option<PathBuf>,
    show_info: bool,
    photo_info: Option<PhotoInfo>,
    show_fixes: bool,
    show_share: bool,
    adjustments: Adjustments,
    pending_save: Option<SaveRequest>,
    slideshow: Option<Slideshow>,
    /// Remembered between slideshow runs.
    slideshow_interval: Duration,
    preferences: Preferences,
    /// True while a file is being dragged over the window.
    drag_hover: bool,
    modifiers: ModifiersState,
    last_click: Option<(Instant, PhysicalPosition<f64>)>,
    quit_requested: bool,
    startup_photo_ready: bool,
    startup_photo_presented: bool,
    proxy: EventLoopProxy<UserEvent>,
    jobs_tx: std::sync::mpsc::Sender<JobResult>,
    jobs_rx: std::sync::mpsc::Receiver<JobResult>,
    jobs_running: usize,
}

/// Letter shortcuts follow the character the user's layout produces (so R is
/// R on AZERTY or Dvorak); everything else uses the physical key.
/// Prefer Segoe UI for the HUD. The bundled font renders stray bright boxes
/// next to some glyphs; if Segoe UI is missing the defaults stay in place.
fn install_system_font(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let path = Path::new(&windir).join("Fonts").join("segoeui.ttf");
    let Ok(bytes) = std::fs::read(&path) else {
        log::debug!("system font {} not found", path.display());
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "segoe-ui".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "segoe-ui".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Advance a session rotation by one quarter turn, wrapping after a full turn.
fn stepped_quarter_turn(turns: u8, clockwise: bool) -> u8 {
    let turns = turns % 4;
    if clockwise {
        (turns + 1) % 4
    } else {
        (turns + 3) % 4
    }
}

fn shortcut_code(logical: &Key, physical: PhysicalKey) -> Option<KeyCode> {
    if let Key::Character(text) = logical {
        let mut chars = text.chars();
        if let (Some(letter), None) = (chars.next(), chars.next()) {
            let code = match letter.to_ascii_lowercase() {
                'a' => Some(KeyCode::KeyA),
                'b' => Some(KeyCode::KeyB),
                'c' => Some(KeyCode::KeyC),
                'd' => Some(KeyCode::KeyD),
                'e' => Some(KeyCode::KeyE),
                'f' => Some(KeyCode::KeyF),
                'g' => Some(KeyCode::KeyG),
                'h' => Some(KeyCode::KeyH),
                'i' => Some(KeyCode::KeyI),
                'j' => Some(KeyCode::KeyJ),
                'k' => Some(KeyCode::KeyK),
                'l' => Some(KeyCode::KeyL),
                'm' => Some(KeyCode::KeyM),
                'n' => Some(KeyCode::KeyN),
                'o' => Some(KeyCode::KeyO),
                'p' => Some(KeyCode::KeyP),
                'q' => Some(KeyCode::KeyQ),
                'r' => Some(KeyCode::KeyR),
                's' => Some(KeyCode::KeyS),
                't' => Some(KeyCode::KeyT),
                'u' => Some(KeyCode::KeyU),
                'v' => Some(KeyCode::KeyV),
                'w' => Some(KeyCode::KeyW),
                'x' => Some(KeyCode::KeyX),
                'y' => Some(KeyCode::KeyY),
                'z' => Some(KeyCode::KeyZ),
                _ => None,
            };
            if code.is_some() {
                return code;
            }
        }
    }
    match physical {
        PhysicalKey::Code(code) => Some(code),
        PhysicalKey::Unidentified(_) => None,
    }
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
        install_system_font(&egui_ctx);
        egui_ctx.set_theme(egui::Theme::Dark);
        egui_ctx.all_styles_mut(|style| {
            style.visuals = egui::Visuals::dark();
            style.visuals.override_text_color = Some(egui::Color32::from_rgb(232, 234, 240));
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            // Subpixel-binned glyphs (".", "/", "+") render with stray bright
            // boxes in this egui version.
            style.visuals.text_options.subpixel_binning = false;
        });

        // Start the first decode before the window and GPU exist. Adapter and
        // device creation can take hundreds of milliseconds; decoding in
        // parallel hides most of that latency. 8192 is a safe initial texture
        // limit and is replaced with the real GPU limit once it is known.
        let loader = Loader::new(proxy.clone(), 8192);
        let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
        loader.set_current(nav.current());
        let first_path = nav
            .current_path()
            .expect("a fresh navigation listing always has a current image");
        loader.request_full(first_path, nav.current(), 3);
        log::info!(
            "startup[{:.1}ms]: loader started, current image queued",
            crate::startup_ms()
        );

        let now = Instant::now();
        let preferences = Preferences::load();

        Ok(Self {
            nav,
            window: None,
            renderer: None,
            preview: None,
            preview_until: None,
            preview_drop_pending: false,
            egui_ctx,
            egui_state: None,
            loader: Some(loader),
            camera: Camera::default(),
            session_rotation: HashMap::new(),
            photos: HashMap::new(),
            thumbs: HashMap::new(),
            animation: None,
            no_animation: HashSet::new(),
            failed_animation: HashSet::new(),
            deferred_loader_events: Vec::new(),
            failed_full: HashMap::new(),
            failed_thumb: HashSet::new(),
            reload_full: HashSet::new(),
            reload_thumb: HashSet::new(),
            cursor: None,
            dragging: false,
            drag_last: None,
            last_activity: now,
            last_frame: now,
            hud_alpha: 0.0,
            hud_warmed: false,
            hud_intro_done: false,
            hud_anchor: None,
            ui_hovered: false,
            resize_hovered: false,
            scroll_filmstrip: true,
            photo_fade: 1.0,
            photo_fade_start: now,
            show_help_until: now + HELP_DURATION,
            toast: None,
            pending_delete: None,
            show_info: false,
            photo_info: None,
            show_fixes: false,
            show_share: false,
            adjustments: Adjustments::default(),
            pending_save: None,
            slideshow: None,
            slideshow_interval: preferences.slideshow_interval,
            preferences,
            drag_hover: false,
            modifiers: ModifiersState::empty(),
            last_click: None,
            quit_requested: false,
            startup_photo_ready: false,
            startup_photo_presented: false,
            proxy,
            jobs_tx,
            jobs_rx,
            jobs_running: 0,
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
        if let Some(path) = self.nav.current_path() {
            match PreviewWindow::show(
                path,
                preview_position.x,
                preview_position.y,
                size.width,
                size.height,
                self.preferences.scrim_alpha(),
            ) {
                Ok(preview) => self.preview = Some(preview),
                Err(error) => log::warn!("could not show the startup preview: {error:#}"),
            }
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
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        window.set_title(&format!("{title_name} — picasa-rs"));

        log::info!("startup[{:.1}ms]: window created", crate::startup_ms());
        let mut renderer =
            Renderer::new(window.clone()).context("could not initialize the wgpu renderer")?;
        // While the native preview is up it already paints the dim backdrop;
        // stacking a second one on top would flash noticeably darker.
        renderer.set_backdrop_alpha(if self.preview.is_some() {
            0.0
        } else {
            self.preferences.scrim_opacity
        });
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

        self.animation = None;
        self.scroll_filmstrip = true;
        self.last_click = None;
        self.adjustments = Adjustments::default();
        if let Some(slideshow) = &mut self.slideshow {
            slideshow.defer(Instant::now());
        }

        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            self.camera.clear_image();
            return;
        };
        if self.show_info {
            self.photo_info = Some(info::read(&path));
        }

        if let Some(photo) = self.photos.get(&path) {
            self.camera
                .set_image(photo.display_width as f32, photo.display_height as f32);
            self.photo_fade = 0.0;
            self.photo_fade_start = Instant::now();
        } else {
            self.camera.clear_image();
            self.photo_fade = 1.0;
        }

        self.apply_session_rotation();
        self.request_window();
        self.request_animation_for_current();
        self.record_activity();

        if let Some(window) = &self.window {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            window.set_title(&format!("{name} — picasa-rs"));
            window.request_redraw();
        }
    }

    /// Reapply the current photo's session rotation after the camera has been
    /// reset for a newly selected or newly loaded image.
    fn apply_session_rotation(&mut self) {
        let turns = self
            .nav
            .current_path()
            .and_then(|path| self.session_rotation.get(path))
            .copied()
            .unwrap_or(0);
        self.camera
            .set_rotation(f32::from(turns) * std::f32::consts::FRAC_PI_2);
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

    /// Replace the current folder listing with one built around a dropped
    /// file, or the first supported image in a dropped folder.
    fn open_path(&mut self, path: &Path) -> Result<()> {
        let target = crate::viewer::navigation::resolve_drop(path)?;
        if !crate::loader::decoder::is_supported(&target) {
            bail!("not a supported image");
        }
        let nav = Navigation::new(&target)?;

        // Opening another photo from the same folder keeps decoded textures and
        // thumbnails; only the listing is refreshed and failures are retried.
        let same_folder = self
            .nav
            .current_path()
            .and_then(Path::parent)
            .is_some_and(|folder| Some(folder) == target.parent());

        self.nav = nav;
        if same_folder {
            self.failed_full.clear();
            self.failed_thumb.clear();
        } else {
            self.photos.clear();
            self.thumbs.clear();
            self.failed_full.clear();
            self.failed_thumb.clear();
        }
        self.reload_full.clear();
        self.reload_thumb.clear();
        self.failed_animation.clear();
        self.no_animation.clear();
        self.animation = None;
        self.deferred_loader_events.clear();
        self.pending_delete = None;
        self.pending_save = None;
        self.show_info = false;
        self.photo_info = None;
        self.show_fixes = false;
        self.show_share = false;
        self.adjustments = Adjustments::default();
        self.slideshow = None;
        self.drag_hover = false;

        self.on_current_changed();
        Ok(())
    }
}

impl App {
    fn current_photo(&self) -> Option<&GpuPhoto> {
        self.photos.get(self.nav.current_path()?)
    }

    fn record_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// True when the pointer moved far enough from `anchor` that the idle HUD
    /// should fade back in. Without an anchor the first movement counts.
    ///
    /// Kept free of window types beyond the position so it can be tested
    /// without a GPU or an event loop.
    fn cursor_wakes_hud(
        anchor: Option<PhysicalPosition<f64>>,
        position: PhysicalPosition<f64>,
    ) -> bool {
        let Some(anchor) = anchor else {
            return true;
        };
        let dx = position.x - anchor.x;
        let dy = position.y - anchor.y;
        dx * dx + dy * dy >= HUD_WAKE_DISTANCE * HUD_WAKE_DISTANCE
    }

    fn show_toast(&mut self, text: impl AsRef<str>) {
        self.toast = Some(Toast {
            text: truncate(text.as_ref(), 150),
            until: Instant::now() + TOAST_DURATION,
        });
    }

    /// Refresh the folder listing so files added or removed outside the viewer
    /// appear, keeping the current photo selected.
    fn rescan_folder(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        match crate::viewer::navigation::Navigation::new(&path) {
            Ok(nav) => {
                self.nav = nav;
                self.show_toast("Folder refreshed");
            }
            Err(error) => self.show_toast(format!("Could not refresh the folder: {error:#}")),
        }
    }

    /// True (with a toast) while a background save or copy runs.
    fn jobs_busy(&mut self) -> bool {
        if self.jobs_running > 0 {
            self.show_toast("Another operation is still running");
            return true;
        }
        false
    }

    /// Run slow file work off the UI thread; the result comes back through
    /// `UserEvent::JobDone`.
    fn spawn_job(&mut self, job: impl FnOnce() -> JobResult + Send + 'static) {
        self.jobs_running += 1;
        let sender = self.jobs_tx.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let _ = sender.send(job());
            let _ = proxy.send_event(UserEvent::JobDone);
        });
    }

    fn drain_jobs(&mut self) {
        while let Ok(result) = self.jobs_rx.try_recv() {
            self.jobs_running = self.jobs_running.saturating_sub(1);
            match result {
                JobResult::Saved { name, result } => self.finish_save(name, result),
                JobResult::Copied { path, name, result } => self.finish_copy(path, name, result),
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn focus_window(&self) {
        if let Some(window) = &self.window {
            window.set_minimized(false);
            window.set_visible(true);
            window.focus_window();
            window.request_redraw();
        }
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
}

impl App {
    fn render(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.renderer.is_none() || self.egui_state.is_none() {
            return;
        }

        // Window state changes (maximize, fullscreen, restore) can deliver a
        // redraw before the matching resize event. Reconcile from the window
        // first, and skip frames while minimized or otherwise zero-sized.
        let window_size = window.inner_size();
        if window.is_minimized() == Some(true) || window_size.width == 0 || window_size.height == 0
        {
            return;
        }
        let surface_size = {
            let renderer = self.renderer.as_mut().expect("renderer was checked above");
            if renderer.surface_size() != (window_size.width, window_size.height) {
                renderer.resize(window_size);
            }
            renderer.surface_size()
        };

        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().clamp(0.0, 0.05);
        self.last_frame = now;

        self.camera.update(dt);

        let hud_target = self.hud_target();
        let hud_rate = if self.hud_intro_done { 12.0 } else { 6.0 };
        let hud_t = 1.0 - (-hud_rate * dt).exp();
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
            if !self.hud_warmed {
                self.hud_alpha = 0.0;
            }
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
            // Restart the HUD fade so the chrome eases in after the photo
            // instead of popping in behind the slow first egui frame.
            self.hud_alpha = 0.0;
        }

        let screen = ScreenDescriptor {
            size_in_pixels: [surface_size.0.max(1), surface_size.1.max(1)],
            pixels_per_point,
        };

        let center = self.camera.screen_center();
        let half_size = self.camera.screen_half_size();
        let angle = self.camera.angle;
        let alpha = self.photo_fade;
        let (adjust, gains) = self.adjustments.uniforms();
        let current_path = self.nav.current_path();

        let render_result = {
            let photo = current_path
                .and_then(|path| self.photos.get(path))
                .map(|photo| PhotoDraw {
                    photo,
                    center: [center.x, center.y],
                    half_size: [half_size.x, half_size.y],
                    angle,
                    alpha,
                    adjust,
                    gains,
                });

            self.renderer
                .as_mut()
                .expect("renderer was checked above")
                .render(photo, &paint_jobs, textures_delta, screen)
        };
        let render_ok = match render_result {
            Ok(()) => true,
            Err(error) => {
                log::error!("rendering failed: {error:#}");
                event_loop.exit();
                false
            }
        };
        if render_ok && self.preview_drop_pending {
            self.preview_drop_pending = false;
            self.preview = None;
        }
        if render_ok && !skip_ui && !self.hud_warmed {
            // The slow first egui frame is on screen; restart the clock so the
            // fade is not consumed by it.
            self.hud_warmed = true;
            self.last_frame = Instant::now();
            window.request_redraw();
        } else if render_ok && !self.hud_intro_done && self.hud_alpha >= 0.98 {
            self.hud_intro_done = true;
        }

        if render_ok
            && !self.startup_photo_presented
            && alpha > 0.0
            && self.current_photo().is_some()
        {
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
                // Windows has no maximize event; detect the native maximize
                // through the window state and present it as fullscreen.
                let maximized = self
                    .window
                    .as_ref()
                    .is_some_and(|window| window.is_maximized() && window.fullscreen().is_none());
                if maximized {
                    self.switch_to_fullscreen();
                }
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
                let hud_awake = self.last_activity.elapsed() < HUD_IDLE;
                if hud_awake || Self::cursor_wakes_hud(self.hud_anchor, position) {
                    self.record_activity();
                    self.hud_anchor = Some(position);
                }

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
                    // egui owns the cursor icon everywhere else, but the
                    // resize edges are outside its UI.
                    let point = Vec2::new(position.x as f32, position.y as f32);
                    if let Some(direction) = self.resize_direction(point) {
                        window.set_cursor(CursorIcon::from(direction));
                        self.resize_hovered = true;
                    } else if self.resize_hovered {
                        window.set_cursor(CursorIcon::Default);
                        self.resize_hovered = false;
                    }
                    window.request_redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.dragging = false;
                self.drag_last = None;
                self.resize_hovered = false;
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
                    && let Some(code) = shortcut_code(&event.logical_key, event.physical_key)
                {
                    self.handle_key(event_loop, code, event.repeat);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::HoveredFile(_) => {
                self.drag_hover = true;
                self.record_activity();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::HoveredFileCancelled => {
                self.drag_hover = false;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.drag_hover = false;
                if let Err(error) = self.open_path(&path) {
                    let message = format!("{error:#}");
                    log::warn!("could not open dropped path {}: {message}", path.display());
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.show_toast(format!("Could not open {name}: {message}"));
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
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
            UserEvent::Focus => self.focus_window(),
            UserEvent::JobDone => self.drain_jobs(),
            UserEvent::OpenPath(path) => {
                if let Err(error) = self.open_path(&path) {
                    let message = format!("{error:#}");
                    log::warn!(
                        "could not open forwarded path {}: {message}",
                        path.display()
                    );
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.show_toast(format!("Could not open {name}: {message}"));
                }
                self.focus_window();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit_requested {
            event_loop.exit();
            return;
        }

        let now = Instant::now();
        let animation_advanced = self.advance_animation(now);
        if animation_advanced && let Some(window) = &self.window {
            window.request_redraw();
        }

        if self.slideshow.as_ref().is_some_and(|show| show.is_due(now)) {
            if self.pending_delete.is_some() || self.pending_save.is_some() {
                // Don't advance behind an open confirmation dialog.
                if let Some(show) = &mut self.slideshow {
                    show.defer(now);
                }
            } else {
                self.navigate_by(1);
                if let Some(show) = &mut self.slideshow {
                    show.defer(Instant::now());
                }
            }
        }

        if let Some(until) = self.preview_until
            && now >= until
        {
            // Hand the dim backdrop to the GPU window first and only then drop
            // the preview (at the end of the next render), so the two never
            // stack and there is no gap.
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_backdrop_alpha(self.preferences.scrim_opacity);
            }
            self.preview_drop_pending = true;
            self.preview_until = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        let hud_deadline = self.last_activity + HUD_IDLE;
        let help_deadline = self.show_help_until;

        let animating = self.camera.is_animating()
            || animation_advanced
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
        if let Some(show) = &self.slideshow {
            let deadline = show.next_advance();
            next_deadline = Some(match next_deadline {
                Some(existing) => existing.min(deadline),
                None => deadline,
            });
        }
        if let Some(playback) = &self.animation {
            let deadline = playback.next_deadline();
            next_deadline = Some(match next_deadline {
                Some(existing) => existing.min(deadline),
                None => deadline,
            });
        }
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
    use super::{App, HUD_WAKE_DISTANCE, shortcut_code};
    use winit::dpi::PhysicalPosition;
    use winit::keyboard::{Key, KeyCode, NativeKeyCode, PhysicalKey};

    #[test]
    fn letter_shortcuts_follow_the_layout_not_the_key_position() {
        // On AZERTY the physical Q position produces "a".
        let azerty_a = Key::Character("a".into());
        assert_eq!(
            shortcut_code(&azerty_a, PhysicalKey::Code(KeyCode::KeyQ)),
            Some(KeyCode::KeyA)
        );
        // Non-Latin layouts fall back to the physical key.
        let cyrillic = Key::Character("к".into());
        assert_eq!(
            shortcut_code(&cyrillic, PhysicalKey::Code(KeyCode::KeyR)),
            Some(KeyCode::KeyR)
        );
        assert_eq!(
            shortcut_code(
                &Key::Unidentified(NativeKeyCode::Unidentified.into()),
                PhysicalKey::Unidentified(NativeKeyCode::Unidentified)
            ),
            None
        );
    }

    fn at(x: f64, y: f64) -> PhysicalPosition<f64> {
        PhysicalPosition::new(x, y)
    }

    #[test]
    fn the_first_pointer_move_wakes_the_hud() {
        assert!(App::cursor_wakes_hud(None, at(10.0, 20.0)));
    }

    #[test]
    fn jitter_does_not_wake_the_hud() {
        let anchor = at(500.0, 500.0);
        assert!(!App::cursor_wakes_hud(Some(anchor), at(503.0, 500.0)));
        assert!(!App::cursor_wakes_hud(Some(anchor), at(500.0, 507.0)));
        assert!(!App::cursor_wakes_hud(Some(anchor), at(504.0, 505.0)));
    }

    #[test]
    fn a_long_move_wakes_the_hud() {
        let anchor = at(500.0, 500.0);
        assert!(App::cursor_wakes_hud(Some(anchor), at(530.0, 500.0)));
        assert!(App::cursor_wakes_hud(Some(anchor), at(500.0, 470.0)));
        assert!(App::cursor_wakes_hud(Some(anchor), at(520.0, 520.0)));
    }

    #[test]
    fn the_wake_threshold_is_inclusive() {
        let anchor = at(0.0, 0.0);
        assert!(App::cursor_wakes_hud(
            Some(anchor),
            at(HUD_WAKE_DISTANCE, 0.0)
        ));
        assert!(!App::cursor_wakes_hud(
            Some(anchor),
            at(HUD_WAKE_DISTANCE - 0.1, 0.0)
        ));
    }

    #[test]
    fn session_rotation_wraps_after_four_turns() {
        let mut turns = 0;
        for _ in 0..4 {
            turns = super::stepped_quarter_turn(turns, true);
        }
        assert_eq!(turns, 0);
        assert_eq!(super::stepped_quarter_turn(3, true), 0);
        assert_eq!(super::stepped_quarter_turn(0, false), 3);
        assert_eq!(super::stepped_quarter_turn(3, false), 2);
    }

    #[test]
    fn session_rotation_directions_are_inverse() {
        for turns in 0..4 {
            let clockwise = super::stepped_quarter_turn(turns, true);
            assert_eq!(super::stepped_quarter_turn(clockwise, false), turns);
        }
    }
}
