//! Distance-based cache eviction.

use std::path::PathBuf;

use super::{App, PHOTO_CACHE_RADIUS, THUMB_CACHE_LIMIT};

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
