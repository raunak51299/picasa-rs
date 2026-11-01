//! Animated photo playback and its GPU uploads.

use std::time::Instant;

use crate::animation::Playback;
use crate::loader::decoder;

use super::App;

impl App {
    /// Ask the loader for the current photo's frames when the still image is
    /// already cached. When it is not cached yet, the full-image handler calls
    /// this again after the texture exists.
    pub(super) fn request_animation_for_current(&mut self) {
        let Some(path) = self.nav.current_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        if self
            .animation
            .as_ref()
            .is_some_and(|show| show.path() == path)
        {
            return;
        }
        if self.no_animation.contains(&path) || self.failed_animation.contains(&path) {
            return;
        }
        if !decoder::supports_animation(&path) || !self.photos.contains_key(&path) {
            return;
        }
        if let Some(loader) = &self.loader {
            loader.request_animation(&path, self.nav.current(), 3);
        }
    }

    /// Upload the next frame when its delay has elapsed. Returns true when the
    /// texture changed and the window needs a redraw.
    pub(super) fn advance_animation(&mut self, now: Instant) -> bool {
        let Some(playback) = self.animation.as_mut() else {
            return false;
        };
        if !playback.is_due(now) {
            return false;
        }

        let path = playback.path().to_path_buf();
        let (width, height) = (playback.width(), playback.height());
        let frame = playback.advance(now);
        if let (Some(renderer), Some(photo)) = (&self.renderer, self.photos.get(&path)) {
            renderer.update_photo(photo, &frame.rgba, width, height);
        }
        true
    }

    /// Start (or restart) playback from the first frame. The still texture must
    /// already exist; its dimensions are used to resize any animation frames
    /// that were decoded before the real GPU limit was known.
    pub(super) fn start_animation(&mut self, animation: decoder::Animation) {
        let path = animation.path.clone();
        let Some(photo) = self.photos.get(&path) else {
            return;
        };
        let (width, height) = (photo.texture_width, photo.texture_height);
        let animation = if animation.width == width && animation.height == height {
            animation
        } else {
            decoder::resize_animation(animation, width, height)
        };
        let Some(playback) = Playback::new(animation, Instant::now()) else {
            return;
        };

        if let Some(renderer) = &self.renderer {
            renderer.update_photo(photo, &playback.frame().rgba, width, height);
        }
        self.animation = Some(playback);
    }
}
