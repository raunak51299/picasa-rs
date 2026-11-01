//! Keyboard, mouse, and HUD action handling.

use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;
use winit::window::CursorIcon;

use crate::viewer::camera::Vec2;

use super::{App, HELP_DURATION, UiAction};

impl App {
    pub(super) fn handle_key(&mut self, event_loop: &ActiveEventLoop, code: KeyCode, repeat: bool) {
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

    pub(super) fn apply_action(&mut self, action: UiAction) {
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
