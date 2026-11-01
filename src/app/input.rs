//! Keyboard, mouse, and HUD action handling.

use std::path::Path;
use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;
use winit::window::CursorIcon;

use crate::delete;
use crate::loader::JobKind;
use crate::rotate;
use crate::save::{self, SaveMode};
use crate::share;
use crate::viewer::camera::Vec2;

use super::slideshow::{Slideshow, stepped_interval};
use super::{
    App, HELP_DURATION, RotationRequest, SaveRequest, TOAST_DURATION, Toast, UiAction, truncate,
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

        if self.pending_rotation.is_some() {
            if !repeat {
                match code {
                    KeyCode::Enter | KeyCode::KeyY => self.confirm_rotation(),
                    KeyCode::Escape | KeyCode::KeyN => self.pending_rotation = None,
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
                    self.request_rotation(false);
                } else {
                    self.navigate_by(-1);
                }
            }
            KeyCode::ArrowRight => {
                if self.modifiers.control_key() {
                    self.request_rotation(true);
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
                    if self.modifiers.shift_key() {
                        self.camera.rotate(std::f32::consts::FRAC_PI_2);
                    } else {
                        self.request_rotation(true);
                    }
                }
            }
            KeyCode::KeyL => {
                if !repeat {
                    if self.modifiers.shift_key() {
                        self.camera.rotate(-std::f32::consts::FRAC_PI_2);
                    } else {
                        self.request_rotation(false);
                    }
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
                    "Slideshow every {} s — Space pauses, Esc stops",
                    self.slideshow_interval.as_secs()
                ),
                until: Instant::now() + TOAST_DURATION,
            });
        }
    }

    fn adjust_slideshow(&mut self, step_seconds: i64) {
        self.slideshow_interval = stepped_interval(self.slideshow_interval, step_seconds);
        if let Some(slideshow) = &mut self.slideshow {
            slideshow.set_interval(self.slideshow_interval, Instant::now());
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
        self.pending_delete = Some(path);
    }

    fn confirm_delete(&mut self) {
        let Some(path) = self.pending_delete.take() else {
            return;
        };
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

                if let Some(index) = self.nav.index_of(&path) {
                    self.nav.set_current(index);
                    self.nav.remove_current();
                }

                self.toast = Some(Toast {
                    text: format!("Moved {name} to the Recycle Bin"),
                    until: Instant::now() + TOAST_DURATION,
                });
                log::info!("moved {} to the Recycle Bin", path.display());

                if self.nav.len() == 0 {
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
        let name = request
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match save::save_adjusted(&request.path, request.adjustments, mode) {
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
        match share::copy_image(&path, self.adjustments) {
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

    fn request_rotation(&mut self, clockwise: bool) {
        let Some(path) = self.nav.current_path().map(|path| path.to_path_buf()) else {
            return;
        };
        if !rotate::is_supported(&path) {
            self.toast = Some(Toast {
                text: "Persistent rotation is only supported for JPEG files".to_string(),
                until: Instant::now() + TOAST_DURATION,
            });
            return;
        }
        self.pending_rotation = Some(RotationRequest { path, clockwise });
    }

    fn confirm_rotation(&mut self) {
        let Some(request) = self.pending_rotation.take() else {
            return;
        };
        let name = request
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match rotate::rotate_file(&request.path, request.clockwise) {
            Ok(orientation) => {
                self.invalidate_path(&request.path);
                self.toast = Some(Toast {
                    text: format!("Rotated {name}"),
                    until: Instant::now() + TOAST_DURATION,
                });
                log::info!(
                    "rotated {} to EXIF orientation {orientation}",
                    request.path.display()
                );
                self.camera.clear_image();
                self.on_current_changed();
            }
            Err(error) => {
                self.toast = Some(Toast {
                    text: truncate(&format!("Could not rotate {name}: {error:#}"), 150),
                    until: Instant::now() + TOAST_DURATION,
                });
            }
        }
    }

    pub(super) fn apply_action(&mut self, action: UiAction) {
        match action {
            UiAction::Navigate(index) => self.navigate_to(index),
            UiAction::Fit => self.camera.reset_fit(),
            UiAction::ActualSize => self.camera.toggle_actual_size(self.viewport_center()),
            UiAction::RotatePersist(clockwise) => self.request_rotation(clockwise),
            UiAction::ConfirmRotation => self.confirm_rotation(),
            UiAction::CancelRotation => self.pending_rotation = None,
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
            UiAction::Reveal => self.reveal_current(),
            UiAction::SetAdjustments(mut adjustments) => {
                adjustments.clamp();
                self.adjustments = adjustments;
            }
            UiAction::Save { mode, adjustments } => self.request_save(mode, adjustments),
            UiAction::ConfirmSave => self.confirm_save(),
            UiAction::CancelSave => self.pending_save = None,
            UiAction::ZoomIn => self.camera.zoom_at(self.viewport_center(), 1.25),
            UiAction::ZoomOut => self.camera.zoom_at(self.viewport_center(), 0.8),
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
    pub(super) fn handle_primary_press(&mut self, event_loop: &ActiveEventLoop) {
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
