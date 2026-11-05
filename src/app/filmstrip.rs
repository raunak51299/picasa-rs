use std::ops::Range;

use super::{App, THUMB_RADIUS};

pub(super) const THUMB_TILE: f32 = 52.0;
pub(super) const THUMB_SPACING: f32 = 6.0;
pub(super) const THUMB_STRIDE: f32 = THUMB_TILE + THUMB_SPACING;
pub(super) const THUMB_BUFFER: usize = 8;
pub(super) const THUMB_RELEVANCE_RADIUS: usize = THUMB_RADIUS + 8;

pub(super) fn filmstrip_width(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * THUMB_TILE + (count - 1) as f32 * THUMB_SPACING
    }
}

pub(super) fn visible_range(viewport: egui::Rect, count: usize) -> Range<usize> {
    if count == 0 {
        return 0..0;
    }
    let start = ((viewport.min.x.max(0.0) / THUMB_STRIDE).floor() as usize).min(count);
    let end = ((viewport.max.x.max(0.0) / THUMB_STRIDE).ceil() as usize).min(count);
    start..end.max(start)
}

pub(super) fn expand_range(range: &Range<usize>, count: usize, buffer: usize) -> Range<usize> {
    let start = range.start.saturating_sub(buffer).min(count);
    let end = range.end.saturating_add(buffer).min(count);
    start..end.max(start)
}

pub(super) fn range_distance(
    index: usize,
    current: usize,
    viewport: Option<&Range<usize>>,
) -> usize {
    let current_distance = index.abs_diff(current);
    let Some(viewport) = viewport.filter(|range| !range.is_empty()) else {
        return current_distance;
    };
    let viewport_distance = if index < viewport.start {
        viewport.start - index
    } else if index >= viewport.end {
        index - viewport.end + 1
    } else {
        0
    };
    current_distance.min(viewport_distance)
}

pub(super) fn thumb_is_relevant(
    index: usize,
    current: usize,
    viewport: Option<&Range<usize>>,
    radius: usize,
) -> bool {
    index.abs_diff(current) <= radius || viewport.is_some_and(|range| range.contains(&index))
}

impl App {
    pub(super) fn is_thumb_relevant(&self, index: usize) -> bool {
        thumb_is_relevant(
            index,
            self.nav.current(),
            self.thumb_viewport.as_ref(),
            THUMB_RELEVANCE_RADIUS,
        )
    }

    pub(super) fn reset_thumb_viewport(&mut self) {
        self.thumb_viewport = None;
        if let Some(loader) = &self.loader {
            loader.set_thumb_viewport(None);
        }
    }

    pub(super) fn request_thumb_range(&mut self, range: Range<usize>, priority: u8) {
        let Some(loader) = &self.loader else {
            return;
        };
        for index in range {
            let Some(path) = self.nav.path(index) else {
                continue;
            };
            if self.thumbs.contains_key(path) || self.failed_thumb.contains(path) {
                continue;
            }
            loader.request_thumb(path, index, priority);
        }
    }

    pub(super) fn update_thumb_viewport(&mut self, visible: Range<usize>) {
        let expanded = expand_range(&visible, self.nav.len(), THUMB_BUFFER);
        if self.thumb_viewport.as_ref() == Some(&expanded) {
            return;
        }
        self.thumb_viewport = Some(expanded.clone());
        if let Some(loader) = &self.loader {
            loader.set_thumb_viewport(Some(expanded.clone()));
        }
        self.request_thumb_range(visible, 2);
        self.request_thumb_range(expanded, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2};

    fn viewport(min_x: f32, max_x: f32) -> Rect {
        Rect::from_min_max(pos2(min_x, 0.0), pos2(max_x, THUMB_TILE))
    }

    #[test]
    fn filmstrip_width_accounts_for_spacing() {
        assert_eq!(filmstrip_width(0), 0.0);
        assert_eq!(filmstrip_width(1), THUMB_TILE);
        assert_eq!(filmstrip_width(3), THUMB_TILE * 3.0 + THUMB_SPACING * 2.0);
    }

    #[test]
    fn visible_range_covers_every_partly_visible_tile() {
        assert_eq!(visible_range(viewport(0.0, 100.0), 10), 0..2);
        assert_eq!(visible_range(viewport(110.0, 300.0), 10), 1..6);
    }

    #[test]
    fn visible_range_clamps_at_the_end_of_the_folder() {
        assert_eq!(visible_range(viewport(100.0, 300.0), 3), 1..3);
        assert_eq!(visible_range(viewport(0.0, 10_000.0), 4), 0..4);
    }

    #[test]
    fn visible_range_is_empty_without_photos() {
        assert_eq!(visible_range(viewport(0.0, 100.0), 0), 0..0);
    }

    #[test]
    fn expand_range_adds_a_buffer_and_clamps() {
        assert_eq!(expand_range(&(10..20), 100, 8), 2..28);
        assert_eq!(expand_range(&(0..4), 100, 8), 0..12);
        assert_eq!(expand_range(&(96..100), 100, 8), 88..100);
        assert_eq!(expand_range(&(0..0), 0, 8), 0..0);
    }

    #[test]
    fn range_distance_uses_the_nearer_of_current_and_viewport() {
        assert_eq!(range_distance(60, 0, Some(&(55..65))), 0);
        assert_eq!(range_distance(50, 0, Some(&(55..65))), 5);
        assert_eq!(range_distance(90, 0, Some(&(55..65))), 26);
        assert_eq!(range_distance(2, 0, None), 2);
        assert_eq!(range_distance(2, 0, Some(&(0..0))), 2);
    }

    #[test]
    fn thumb_relevance_accepts_current_or_visible() {
        assert!(thumb_is_relevant(5, 0, None, 21));
        assert!(!thumb_is_relevant(22, 0, None, 21));
        assert!(thumb_is_relevant(500, 0, Some(&(490..510)), 21));
        assert!(!thumb_is_relevant(510, 0, Some(&(490..510)), 21));
    }
}
