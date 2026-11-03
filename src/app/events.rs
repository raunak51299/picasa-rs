//! Loader result handling.

use std::time::Instant;

use crate::loader::LoaderEvent;
use crate::loader::decoder;

use super::{App, PHOTO_CACHE_RADIUS, THUMB_RADIUS, TOAST_DURATION, Toast, truncate};

impl App {
    pub(super) fn drain_loader_events(&mut self) {
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

    pub(super) fn on_loader_event(&mut self, event: LoaderEvent) {
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

        let event_path = match &event {
            LoaderEvent::Full(full) => &full.path,
            LoaderEvent::Thumb(thumb) => &thumb.path,
            LoaderEvent::Animation(animation) => &animation.path,
            LoaderEvent::Error { path, .. } | LoaderEvent::Stale { path, .. } => path,
        };
        if self.nav.index_of(event_path).is_none() {
            // The file was deleted (or the folder changed) while a decode was
            // in flight; its result must not resurrect the old entry.
            return;
        }

        match event {
            LoaderEvent::Full(full) => {
                let index = full.index;
                let path = full.path.clone();

                if self.reload_full.remove(&path) {
                    self.request_window();
                    return;
                }

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
                    self.apply_session_rotation();
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
                    self.request_animation_for_current();
                }

                self.evict_photo_cache();
                self.request_window();
            }
            LoaderEvent::Thumb(thumb) => {
                if self.reload_thumb.remove(&thumb.path) {
                    self.request_window();
                    return;
                }
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
            LoaderEvent::Animation(animation) => {
                if animation.index != self.nav.current() {
                    return;
                }
                if animation.frames.len() < 2 {
                    self.no_animation.insert(animation.path);
                    return;
                }
                self.start_animation(animation);
            }
            LoaderEvent::Error {
                path,
                index,
                kind,
                message,
            } => {
                if kind == crate::loader::JobKind::Full && self.reload_full.remove(&path) {
                    self.request_window();
                    return;
                }
                if kind == crate::loader::JobKind::Thumb && self.reload_thumb.remove(&path) {
                    self.request_window();
                    return;
                }
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
                    crate::loader::JobKind::Animation => {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        self.toast = Some(Toast {
                            text: truncate(&format!("Could not play {name}: {message}"), 150),
                            until: Instant::now() + TOAST_DURATION,
                        });
                        self.failed_animation.insert(path);
                    }
                }
                self.request_window();
            }
            LoaderEvent::Stale { path, index, kind } => {
                log::trace!("skipped stale job for {}", path.display());
                if kind == crate::loader::JobKind::Full && self.reload_full.remove(&path) {
                    self.request_window();
                    return;
                }
                if kind == crate::loader::JobKind::Thumb && self.reload_thumb.remove(&path) {
                    self.request_window();
                    return;
                }
                let needed = match kind {
                    crate::loader::JobKind::Full => index == self.nav.current(),
                    crate::loader::JobKind::Thumb => self.nav.is_near(index, THUMB_RADIUS),
                    crate::loader::JobKind::Animation => index == self.nav.current(),
                };
                if needed {
                    if kind == crate::loader::JobKind::Animation {
                        self.request_animation_for_current();
                    } else {
                        self.request_window();
                    }
                }
            }
        }

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
