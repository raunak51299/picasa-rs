//! Distance-based cache eviction.

use std::path::PathBuf;

use super::{App, PHOTO_CACHE_BYTE_BUDGET, PHOTO_CACHE_RADIUS, THUMB_CACHE_LIMIT};

impl App {
    pub(super) fn evict_photo_cache(&mut self) {
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

        // Large photos make the entry count a poor proxy for memory use, so
        // also enforce a byte budget (the current photo is always kept).
        let sized: Vec<(PathBuf, usize, u64)> = self
            .photos
            .iter()
            .map(|(path, photo)| {
                (
                    path.clone(),
                    self.nav.index_of(path).unwrap_or(usize::MAX),
                    texture_bytes(photo.texture_width, photo.texture_height),
                )
            })
            .collect();
        for path in budget_targets(&sized, current, PHOTO_CACHE_BYTE_BUDGET) {
            self.photos.remove(&path);
        }
    }

    pub(super) fn evict_thumb_cache(&mut self) {
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
}

/// Approximate GPU memory of an RGBA8 texture including its mip chain.
fn texture_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 4 * 4 / 3
}

/// Paths to drop, furthest from `current` first, until the remaining entries
/// fit in `budget` bytes. The entry at `current` is never dropped.
fn budget_targets(entries: &[(PathBuf, usize, u64)], current: usize, budget: u64) -> Vec<PathBuf> {
    let mut total: u64 = entries.iter().map(|(_, _, bytes)| bytes).sum();
    if total <= budget {
        return Vec::new();
    }

    let mut ranked: Vec<&(PathBuf, usize, u64)> = entries
        .iter()
        .filter(|(_, index, _)| *index != current)
        .collect();
    ranked
        .sort_by_key(|(path, index, _)| (std::cmp::Reverse(index.abs_diff(current)), path.clone()));

    let mut dropped = Vec::new();
    for (path, _, bytes) in ranked {
        if total <= budget {
            break;
        }
        total -= bytes;
        dropped.push(path.clone());
    }
    dropped
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

#[cfg(test)]
mod tests {
    use super::{budget_targets, eviction_targets};
    use std::path::PathBuf;

    #[test]
    fn byte_budget_drops_far_photos_but_never_the_current_one() {
        let cached = vec![
            (PathBuf::from("a.jpg"), 0, 400),
            (PathBuf::from("b.jpg"), 1, 400),
            (PathBuf::from("c.jpg"), 5, 400),
        ];
        assert_eq!(budget_targets(&cached, 0, 800), [PathBuf::from("c.jpg")]);
        assert_eq!(
            budget_targets(&cached, 0, 1),
            [PathBuf::from("c.jpg"), PathBuf::from("b.jpg")]
        );
        assert!(budget_targets(&cached, 0, 1200).is_empty());
    }

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
