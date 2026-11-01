//! Playback state for animated GIF and WebP photos.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::loader::decoder::{Animation, AnimationFrame};

/// A decoded animation plus the frame currently on screen and when to advance.
pub struct Playback {
    path: PathBuf,
    width: u32,
    height: u32,
    frames: Vec<AnimationFrame>,
    index: usize,
    next: Instant,
}

impl Playback {
    /// Start at the first frame. Returns `None` for an empty animation.
    pub fn new(animation: Animation, now: Instant) -> Option<Self> {
        let first = animation.frames.first()?;
        let next = now + first.delay;
        Some(Self {
            path: animation.path,
            width: animation.width,
            height: animation.height,
            frames: animation.frames,
            index: 0,
            next,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn frame(&self) -> &AnimationFrame {
        &self.frames[self.index]
    }

    pub fn next_deadline(&self) -> Instant {
        self.next
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next
    }

    /// Advance to the next frame, wrapping at the end, and return it.
    pub fn advance(&mut self, now: Instant) -> &AnimationFrame {
        self.index = (self.index + 1) % self.frames.len();
        self.next = now + self.frames[self.index].delay;
        &self.frames[self.index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn animation(frames: usize, delay_ms: u64) -> Animation {
        Animation {
            path: PathBuf::from("animation.gif"),
            index: 0,
            width: 2,
            height: 2,
            frames: (0..frames)
                .map(|_| AnimationFrame {
                    rgba: vec![0; 2 * 2 * 4],
                    delay: Duration::from_millis(delay_ms),
                })
                .collect(),
        }
    }

    #[test]
    fn playback_starts_at_the_first_frame_and_waits_for_its_delay() {
        let start = Instant::now();
        let playback = Playback::new(animation(3, 100), start).unwrap();
        assert_eq!(playback.frame().rgba.len(), 16);
        assert!(!playback.is_due(start + Duration::from_millis(99)));
        assert!(playback.is_due(start + Duration::from_millis(100)));
        assert_eq!(playback.next_deadline(), start + Duration::from_millis(100));
    }

    #[test]
    fn playback_wraps_and_uses_each_frame_delay() {
        let start = Instant::now();
        let mut playback = Playback::new(animation(2, 100), start).unwrap();

        playback.advance(start + Duration::from_millis(100));
        assert_eq!(playback.next_deadline(), start + Duration::from_millis(200));
        playback.advance(start + Duration::from_millis(200));
        assert_eq!(playback.next_deadline(), start + Duration::from_millis(300));

        // The second advance wrapped back to frame zero.
        assert_eq!(playback.frame().delay, Duration::from_millis(100));
    }

    #[test]
    fn empty_animations_do_not_start() {
        assert!(Playback::new(animation(0, 100), Instant::now()).is_none());
    }
}
