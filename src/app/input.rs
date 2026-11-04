//! Keyboard, mouse, and HUD action handling.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;
use winit::window::{CursorIcon, Fullscreen, ResizeDirection};

use crate::delete;
use crate::loader::JobKind;
use crate::save::{self, SaveMode};
use crate::share;
use crate::viewer::camera::Vec2;

use super::slideshow::{Slideshow, stepped_interval};
use super::{
    App, HELP_DURATION, JobResult, SaveRequest, TOAST_DURATION, Toast, UiAction, truncate,
};

impl App {
    pub(super) fn handle_key(&mut self, event_loop: &ActiveEventLoop, code: KeyCode, repeat: bool) {
        if self.pending_delete.is_some() {
            if !repeat {
                match code {
                    KeyCode::Enter | KeyCode::KeyY => self.confirm_delete(),
                    KeyCode::Escape | KeyCode::KeyN => self.pending_delete = None,
                    _ => {}
                }
            }
            self.record_activity();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.pending_save.is_some() {
            if !repeat {
                match code {
                    KeyCode::Enter | KeyCode::KeyY => self.confirm_save(),
                    KeyCode::Escape | KeyCode::KeyN => self.pending_save = None,
                    _ => {}
                }
            }
            self.record_activity();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        match code {
            KeyCode::Escape => {
                if !repeat {
                    if self.show_info {
                        self.show_info = false;
                    } else if self.show_fixes {
                        self.show_fixes = false;
                    } else if self.show_share {
                        self.show_share = false;
                    } else if self.slideshow.take().is_some() {
                        self.toast = Some(Toast {
                            text: "Slideshow stopped".to_string(),
                            until: Instant::now() + TOAST_DURATION,
                        });
                    } else {
                        event_loop.exit();
                    }
                }
            }
            KeyCode::KeyQ => {
                if !repeat {
                    event_loop.exit();
                }
            }
            KeyCode::ArrowLeft => {
                if self.modifiers.control_key() {
                    self.rotate_view(false);
                } else {
                    self.navigate_by(-1);
                }
            }
            KeyCode::ArrowRight => {
                if self.modifiers.control_key() {
                    self.rotate_view(true);
                } else {
                    self.navigate_by(1);
                }
            }
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
            KeyCode::Space | KeyCode::KeyS => {
                if !repeat {
                    self.toggle_slideshow();
                }
            }
            KeyCode::Digit0 | KeyCode::Numpad0 => {
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
                    self.rotate_view(true);
                }
            }
            KeyCode::KeyL => {
                if !repeat {
                    self.rotate_view(false);
                }
            }
            KeyCode::Delete => {
                if !repeat {
                    self.request_delete();
                }
            }
            KeyCode::KeyI => {
                if !repeat {
                    self.show_info = !self.show_info;
                    if self.show_info {
                        self.photo_info = self.nav.current_path().map(crate::info::read);
                    }
                }
            }
            KeyCode::KeyF => {
                if !repeat {
                    self.show_fixes = !self.show_fixes;
                }
            }
            KeyCode::KeyC => {
                if !repeat {
                    if self.modifiers.control_key() {
                        if self.modifiers.shift_key() {
                            self.copy_current_path();
                        } else {
                            self.copy_current_image();
                        }
                    } else {
                        self.show_share = !self.show_share;
                    }
                }
            }
            KeyCode::KeyE => {
                if !repeat && self.modifiers.control_key() {
                    self.reveal_current();
                }
            }
            KeyCode::Enter => {
                if !repeat && self.modifiers.control_key() {
                    self.open_current();
                }
            }
            KeyCode::KeyP => {
                if !repeat && self.modifiers.control_key() {
                    self.print_current();
                }
            }
            KeyCode::KeyH => {
                if !repeat {
                    self.show_help_until = Instant::now() + HELP_DURATION;
                }
            }
            KeyCode::F5 => {
                if !repeat {
                    self.rescan_folder();
                    self.failed_full.clear();
                    self.failed_thumb.clear();
                    self.failed_animation.clear();
                    self.no_animation.clear();
                    self.animation = None;
                    self.request_window();
                    self.request_animation_for_current();
                }
            }
            KeyCode::BracketLeft => {
                if !repeat {
                    self.adjust_slideshow(-2);
                }
            }
            KeyCode::BracketRight => {
                if !repeat {
                    self.adjust_slideshow(2);
                }
            }
            KeyCode::ArrowUp => self.zoom_in(),
            KeyCode::ArrowDown => self.zoom_out(),
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

    fn toggle_slideshow(&mut self) {
        if self.slideshow.take().is_some() {
            self.toast = Some(Toast {
                text: "Slideshow paused".to_string(),
                until: Instant::now() + TOAST_DURATION,
            });
        } else {
            self.slideshow = Some(Slideshow::new(self.slideshow_interval));
            self.toast = Some(Toast {
                text: format!(
                    "Slideshow every {} s â€” Space pauses, Esc stops",
                    self.slideshow_interval.as_secs()
                ),
                until: Instant::now() + TOAST_DURATION,
            });
        }
    }

    fn adjust_slideshow(&mut self, step_seconds: i64) {
        let previous = self.slideshow_interval;
        self.slideshow_interval = stepped_interval(self.slideshow_interval, step_seconds);
        if let Some(slideshow) = &mut self.slideshow {
            slideshow.set_interval(self.slideshow_interval, Instant::now());
        }
        if self.slideshow_interval != previous {
            self.preferences.slideshow_interval = self.slideshow_interval;
            self.preferences.save();
        }
        self.toast = Some(Toast {
            text: format!(
                "Slideshow interval: {} s",
                self.slideshow_interval.as_secs()
            ),
            until: Instant::now() + TOAST_DURATION,
        });
    }

    fn request_delete(&mut self) {
        let Some(path) = self.nav.current_path().map(|path| path.to_path_buf()) else {
            return;
        };
        if self.preferences.confirm_delete {
            self.pending_delete = Some(path);
        } else {
            self.delete_path(path);
        }
    }

    fn confirm_delete(&mut self) {
        let Some(path) = self.pending_delete.take() else {
            return;
        };
        self.delete_path(path);
    }

    fn delete_path(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match delete::move_to_recycle_bin(&path) {
            Ok(()) => {
                self.photos.remove(&path);
                self.thumbs.remove(&path);
                self.failed_full.remove(&path);
                self.failed_thumb.remove(&path);
                self.reload_full.remove(&path);
                self.reload_thumb.remove(&path);
                self.failed_animation.remove(&path);
                self.no_animation.remove(&path);
                self.session_rotation.remove(&path);
                if self
                    .animation
                    .as_ref()
                    .is_some_and(|show| show.path() == path)
                {
                    self.animation = None;
                }

                if let Some(index) = self.nav.index_of(&path) {
                    self.nav.set_current(index);
                    self.nav.remove_current();
                }

                self.toast = Some(Toast {
                    text: format!("Moved {name} to the Recycle Bin"),
                    until: Instant::now() + TOAST_DURATION,
                });
                log::info!("moved {} to the Recycle Bin", path.display());

                if self.nav.is_empty() {
                    self.quit_requested = true;
                } else {
                    self.on_current_changed();
                }
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not delete {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn invalidate_path(&mut self, path: &Path) {
        self.photos.remove(path);
        self.thumbs.remove(path);
        self.failed_full.remove(path);
        self.failed_thumb.remove(path);
        self.failed_animation.remove(path);
        self.no_animation.remove(path);
        if self
            .animation
            .as_ref()
            .is_some_and(|show| show.path() == path)
        {
            self.animation = None;
        }
        self.reload_full.insert(path.to_path_buf());
        self.reload_thumb.insert(path.to_path_buf());

        if let Some(loader) = &self.loader {
            if !loader.has_in_flight(path, JobKind::Full) {
                self.reload_full.remove(path);
            }
            if !loader.has_in_flight(path, JobKind::Thumb) {
                self.reload_thumb.remove(path);
            }
        }
    }

    fn request_save(&mut self, mode: SaveMode, mut adjustments: crate::adjust::Adjustments) {
        adjustments.clamp();
        self.adjustments = adjustments;
        if adjustments.is_neutral() {
            self.toast = Some(Toast {
                text: "Adjust the photo before saving".to_string(),
                until: Instant::now() + TOAST_DURATION,
            });
            return;
        }
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let request = SaveRequest { path, adjustments };
        if mode == SaveMode::Overwrite {
            self.pending_save = Some(request);
        } else {
            self.save_now(request, mode);
        }
    }

    fn confirm_save(&mut self) {
        let Some(request) = self.pending_save.take() else {
            return;
        };
        self.save_now(request, SaveMode::Overwrite);
    }

    fn save_now(&mut self, request: SaveRequest, mode: SaveMode) {
        if self.jobs_busy() {
            return;
        }
        let name = request
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        self.show_toast(format!("Saving {name}â€¦"));
        self.spawn_job(move || JobResult::Saved {
            name,
            result: save::save_adjusted(&request.path, request.adjustments, mode),
        });
    }

    pub(super) fn finish_save(&mut self, name: String, result: anyhow::Result<save::SavedFile>) {
        match result {
            Ok(saved) => {
                let saved_name = saved
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if saved.is_copy {
                    self.nav.insert_sorted(saved.path.clone());
                    self.toast = Some(Toast {
                        text: format!("Saved copy as {saved_name}"),
                        until: Instant::now() + TOAST_DURATION,
                    });
                    log::info!("saved adjusted copy {}", saved.path.display());
                    self.request_window();
                } else {
                    let is_current = self.nav.current_path() == Some(saved.path.as_path());
                    self.invalidate_path(&saved.path);
                    self.toast = Some(Toast {
                        text: format!("Saved fixes to {name}"),
                        until: Instant::now() + TOAST_DURATION,
                    });
                    log::info!("saved color fixes to {}", saved.path.display());
                    if is_current {
                        self.camera.clear_image();
                        self.on_current_changed();
                    } else {
                        self.request_window();
                    }
                }
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not save {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn copy_current_image(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if self.jobs_busy() {
            return;
        }
        let adjustments = self.adjustments;
        self.spawn_job(move || JobResult::Copied {
            result: share::copy_image(&path, adjustments),
            path,
            name,
        });
    }

    pub(super) fn finish_copy(&mut self, path: PathBuf, name: String, result: anyhow::Result<()>) {
        match result {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: format!("Copied {name} to the clipboard"),
                    until: Instant::now() + TOAST_DURATION,
                });
                log::info!("copied {} to the clipboard", path.display());
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not copy {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn copy_current_path(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        match share::copy_path(&path) {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: "Path copied to the clipboard".to_string(),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not copy the path: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn open_current(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match share::open_default(&path) {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: format!("Opened {name} in its default application"),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not open {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn open_gps_map(&mut self) {
        let Some(url) = self
            .photo_info
            .as_ref()
            .and_then(|info| info.gps_map_url.as_deref())
        else {
            return;
        };
        match share::open_url(url) {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: "Opened the photo location in Google Maps".to_string(),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not open Google Maps: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn print_current(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match share::print(&path) {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: format!("Opened the print dialog for {name}"),
                    until: Instant::now() + TOAST_DURATION,
                });
                log::info!("sent {} to the print handler", path.display());
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not print {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    fn reveal_current(&mut self) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match share::reveal(&path) {
            Ok(()) => {
                self.toast = Some(Toast {
                    text: format!("Showing {name} in Explorer"),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not show {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    /// Zoom in around the viewport centre, matching the HUD `+` button.
    fn zoom_in(&mut self) {
        self.camera.zoom_at(self.viewport_center(), 1.25);
    }

    /// Zoom out around the viewport centre, matching the HUD `-` button.
    fn zoom_out(&mut self) {
        self.camera.zoom_at(self.viewport_center(), 0.8);
    }

    /// Rotate the current photo for this session only. The file is untouched.
    fn rotate_view(&mut self, clockwise: bool) {
        let Some(path) = self.nav.current_path().map(Path::to_path_buf) else {
            return;
        };
        let turns = self.session_rotation.get(&path).copied().unwrap_or(0);
        let turns = super::stepped_quarter_turn(turns, clockwise);
        if turns == 0 {
            self.session_rotation.remove(&path);
        } else {
            self.session_rotation.insert(path, turns);
        }
        self.camera.rotate(if clockwise {
            std::f32::consts::FRAC_PI_2
        } else {
            -std::f32::consts::FRAC_PI_2
        });
        self.request_window();
    }

    pub(super) fn apply_action(&mut self, action: UiAction) {
        match action {
            UiAction::Navigate(index) => self.navigate_to(index),
            UiAction::Fit => self.camera.reset_fit(),
            UiAction::ActualSize => self.camera.toggle_actual_size(self.viewport_center()),
            UiAction::RotateView(clockwise) => self.rotate_view(clockwise),
            UiAction::Delete => self.request_delete(),
            UiAction::ConfirmDelete => self.confirm_delete(),
            UiAction::CancelDelete => self.pending_delete = None,
            UiAction::ToggleInfo => {
                self.show_info = !self.show_info;
                if self.show_info {
                    self.photo_info = self.nav.current_path().map(crate::info::read);
                }
            }
            UiAction::ToggleFixes => self.show_fixes = !self.show_fixes,
            UiAction::ToggleShare => self.show_share = !self.show_share,
            UiAction::CopyImage => self.copy_current_image(),
            UiAction::CopyPath => self.copy_current_path(),
            UiAction::OpenDefault => self.open_current(),
            UiAction::OpenGpsMap => self.open_gps_map(),
            UiAction::Reveal => self.reveal_current(),
            UiAction::Print => self.print_current(),
            UiAction::SetAdjustments(mut adjustments) => {
                adjustments.clamp();
                self.adjustments = adjustments;
            }
            UiAction::Save { mode, adjustments } => self.request_save(mode, adjustments),
            UiAction::ConfirmSave => self.confirm_save(),
            UiAction::CancelSave => self.pending_save = None,
            UiAction::ZoomIn => self.zoom_in(),
            UiAction::ZoomOut => self.zoom_out(),
            UiAction::SetZoom(scale) => self
                .camera
                .set_target_scale_at(scale, self.viewport_center()),
            UiAction::ToggleSlideshow => self.toggle_slideshow(),
            UiAction::SlideshowSlower => self.adjust_slideshow(-2),
            UiAction::SlideshowFaster => self.adjust_slideshow(2),
            UiAction::Close => self.quit_requested = true,
        }

        self.record_activity();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    pub(super) fn handle_primary_press(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(position) = self.cursor else {
            return;
        };
        let point = Vec2::new(position.x as f32, position.y as f32);
        let now = Instant::now();

        // Borderless windows have no native resize border, so start the OS
        // resize loop ourselves when the press lands on an edge.
        if let Some(direction) = self.resize_direction(point) {
            self.last_click = None;
            self.resize_hovered = false;
            if let Some(window) = &self.window
                && let Err(error) = window.drag_resize_window(direction)
            {
                log::warn!("could not start a window resize: {error}");
            }
            return;
        }

        if self.camera.has_image() && !self.camera.contains_screen_point(point) {
            // In fullscreen, clicking the transparent area drops into a
            // borderless window sized around the photo. Once windowed, the
            // same click is intentionally inert.
            self.last_click = None;
            self.switch_to_windowed();
            return;
        }

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

        if self.camera.has_image() {
            self.dragging = true;
            self.drag_last = Some(position);
            if let Some(window) = &self.window {
                window.set_cursor(CursorIcon::Grabbing);
            }
            self.record_activity();
        }
    }

    /// Which window edge, if any, the pointer is on.
    ///
    /// Fullscreen windows are not resizable this way, and the 10-pixel band
    /// keeps the HUD and filmstrip (inset by more than that) clickable.
    pub(super) fn resize_direction(&self, point: Vec2) -> Option<ResizeDirection> {
        let window = self.window.as_ref()?;
        if window.fullscreen().is_some() {
            return None;
        }
        let size = window.inner_size();
        edge_resize_direction(size.width as f32, size.height as f32, point)
    }

    /// Leave fullscreen and size a borderless, resizable window around the
    /// photo at its current on-screen size.
    fn switch_to_windowed(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.fullscreen().is_none() {
            return;
        }

        let monitor = window
            .current_monitor()
            .or_else(|| window.primary_monitor());
        let half_size = self.camera.screen_half_size();
        let mut width = (half_size.x * 2.0).max(1.0);
        let mut height = (half_size.y * 2.0).max(1.0);

        // Never make a window so small that the HUD and filmstrip have no room.
        const MIN_WIDTH: f32 = 480.0;
        const MIN_HEIGHT: f32 = 360.0;
        let minimum_scale = (MIN_WIDTH / width).max(MIN_HEIGHT / height).max(1.0);
        width *= minimum_scale;
        height *= minimum_scale;

        // Leave room for the native title bar and the taskbar.
        if let Some(monitor) = &monitor {
            let size = monitor.size();
            let max_width = (size.width as f32 * 0.90).max(1.0);
            let max_height = (size.height as f32 * 0.90).max(1.0);
            let fit = (max_width / width).min(max_height / height).min(1.0);
            width *= fit;
            height *= fit;
        }

        let width = width.round().max(1.0) as u32;
        let height = height.round().max(1.0) as u32;

        window.set_fullscreen(None);
        // Windowed mode uses the native title bar so minimize, maximize, and
        // close match Explorer.
        window.set_decorations(true);
        #[cfg(windows)]
        super::reapply_window_icon(&window);
        window.set_resizable(true);
        window.set_min_inner_size(Some(PhysicalSize::new(MIN_WIDTH as u32, MIN_HEIGHT as u32)));
        let _ = window.request_inner_size(PhysicalSize::new(width, height));

        if let Some(monitor) = monitor {
            let monitor_size = monitor.size();
            let monitor_position = monitor.position();
            let x = monitor_position.x + (monitor_size.width as i32 - width as i32) / 2;
            let y = monitor_position.y + (monitor_size.height as i32 - height as i32) / 2;
            window.set_outer_position(PhysicalPosition::new(x, y));
        }

        self.record_activity();
        window.request_redraw();
    }

    /// Present the window as borderless fullscreen. The native title bar's
    /// maximize button would otherwise just grow the window.
    pub(super) fn switch_to_fullscreen(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.fullscreen().is_some() {
            return;
        }
        // Unmaximizing restores the decorated window for a moment. Windows
        // leaves that restored frame behind as stale desktop pixels, and the
        // transparent fullscreen window then reveals it as a ghost because
        // DWM does not repaint the desktop underneath. Hide the window across
        // the transition so the desktop repaints that area first.
        let visible = window.is_visible().unwrap_or(true);
        window.set_visible(false);
        // Drop the maximize first: winit saves the old placement when entering
        // fullscreen, and that placement has to describe the normal window so
        // leaving fullscreen restores it instead of a still-maximized one.
        window.set_maximized(false);
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        if visible {
            window.set_visible(true);
            window.focus_window();
        }
        self.record_activity();
        window.request_redraw();
    }
}

/// Map a pointer position to a window resize edge.
fn edge_resize_direction(width: f32, height: f32, point: Vec2) -> Option<ResizeDirection> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    const EDGE: f32 = 10.0;
    let left = point.x <= EDGE;
    let right = point.x >= width - EDGE;
    let top = point.y <= EDGE;
    let bottom = point.y >= height - EDGE;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, _, _, _) => ResizeDirection::West,
        (_, true, _, _) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::edge_resize_direction;
    use crate::viewer::camera::Vec2;
    use winit::window::ResizeDirection;

    const WIDTH: f32 = 800.0;
    const HEIGHT: f32 = 600.0;

    fn direction(x: f32, y: f32) -> Option<ResizeDirection> {
        edge_resize_direction(WIDTH, HEIGHT, Vec2::new(x, y))
    }

    #[test]
    fn the_centre_is_not_a_resize_edge() {
        assert_eq!(direction(400.0, 300.0), None);
        assert_eq!(direction(11.0, 11.0), None);
        assert_eq!(direction(789.0, 589.0), None);
    }

    #[test]
    fn corners_resize_in_both_axes() {
        assert_eq!(direction(0.0, 0.0), Some(ResizeDirection::NorthWest));
        assert_eq!(direction(WIDTH, 0.0), Some(ResizeDirection::NorthEast));
        assert_eq!(direction(0.0, HEIGHT), Some(ResizeDirection::SouthWest));
        assert_eq!(direction(WIDTH, HEIGHT), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn edges_resize_in_one_axis() {
        assert_eq!(direction(0.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(direction(WIDTH, 300.0), Some(ResizeDirection::East));
        assert_eq!(direction(400.0, 0.0), Some(ResizeDirection::North));
        assert_eq!(direction(400.0, HEIGHT), Some(ResizeDirection::South));
    }

    #[test]
    fn the_band_is_ten_pixels_wide() {
        assert_eq!(direction(10.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(direction(10.5, 300.0), None);
        assert_eq!(direction(790.0, 300.0), Some(ResizeDirection::East));
        assert_eq!(direction(789.5, 300.0), None);
    }

    #[test]
    fn zero_sized_windows_have_no_edges() {
        assert_eq!(edge_resize_direction(0.0, 600.0, Vec2::new(0.0, 0.0)), None);
        assert_eq!(edge_resize_direction(800.0, 0.0, Vec2::new(0.0, 0.0)), None);
    }
}
