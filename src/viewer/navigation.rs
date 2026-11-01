//! Directory scanning, natural sorting, and current-index tracking.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::loader::decoder;

pub struct Navigation {
    paths: Vec<PathBuf>,
    index_by_path: HashMap<PathBuf, usize>,
    current: usize,
}

/// List the supported images in `directory`, sorted naturally by filename.
pub fn list_images(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("could not read directory {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && decoder::is_supported(path))
        .collect();

    paths.sort_by(|a, b| {
        natural_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });

    Ok(paths)
}

impl Navigation {
    /// Scan the target's directory for supported images, sort them naturally,
    /// and select the target image.
    pub fn new(target: &Path) -> Result<Self> {
        let target = std::path::absolute(target)
            .with_context(|| format!("invalid image path: {}", target.display()))?;
        let directory = target
            .parent()
            .with_context(|| format!("{} has no parent directory", target.display()))?;

        let paths = list_images(directory)?;

        let current = paths.iter().position(|path| path == &target).or_else(|| {
            // Paths can differ in casing on Windows; fall back to a
            // case-insensitive filename match within the same directory.
            let target_name = target.file_name()?.to_string_lossy().to_lowercase();
            paths.iter().position(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_lowercase() == target_name)
                    .unwrap_or(false)
            })
        });

        let current = match current {
            Some(current) => current,
            None => {
                // The target exists but was filtered out (unsupported extension).
                bail!(
                    "{} is not a supported image (or the directory changed while scanning)",
                    target.display()
                );
            }
        };

        let index_by_path = paths
            .iter()
            .enumerate()
            .map(|(index, path)| (path.clone(), index))
            .collect();

        Ok(Self {
            paths,
            index_by_path,
            current,
        })
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.paths.get(self.current).map(PathBuf::as_path)
    }

    pub fn path(&self, index: usize) -> Option<&Path> {
        self.paths.get(index).map(PathBuf::as_path)
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.index_by_path.get(path).copied()
    }

    pub fn set_current(&mut self, index: usize) -> bool {
        if index >= self.paths.len() {
            return false;
        }
        let changed = index != self.current;
        self.current = index;
        changed
    }

    /// Move by `delta`, wrapping at either end so holding an arrow key cycles.
    pub fn step(&mut self, delta: isize) -> bool {
        if self.paths.len() <= 1 {
            return false;
        }
        let len = self.paths.len() as isize;
        let next = (self.current as isize + delta).rem_euclid(len) as usize;
        self.set_current(next)
    }

    pub fn first(&mut self) -> bool {
        self.set_current(0)
    }

    pub fn last(&mut self) -> bool {
        self.set_current(self.paths.len().saturating_sub(1))
    }

    /// Return an iterator of indices ordered by distance from the current
    /// image, useful for prioritised prefetching.
    pub fn indices_by_distance(&self, radius: usize) -> Vec<usize> {
        let mut indices = Vec::with_capacity(radius * 2 + 1);
        indices.push(self.current);
        for distance in 1..=radius {
            if self.current >= distance {
                indices.push(self.current - distance);
            }
            if self.current + distance < self.paths.len() {
                indices.push(self.current + distance);
            }
        }
        indices
    }

    pub fn is_near(&self, index: usize, radius: usize) -> bool {
        self.current.abs_diff(index) <= radius
    }

    /// Remove the current image and return its path. The selection moves to
    /// the image that took its place, or to the previous one at the end.
    pub fn remove_current(&mut self) -> Option<PathBuf> {
        if self.paths.is_empty() {
            return None;
        }

        let removed = self.paths.remove(self.current);
        self.index_by_path = self
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| (path.clone(), index))
            .collect();
        if self.current >= self.paths.len() {
            self.current = self.paths.len().saturating_sub(1);
        }
        Some(removed)
    }
}

/// Compare filenames using natural alphanumeric ordering:
/// `photo2.jpg` comes before `photo10.jpg`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    let mut i = 0;
    let mut j = 0;

    while i < a.len() && j < b.len() {
        let ca = a[i];
        let cb = b[j];

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let start_i = i;
            let start_j = j;

            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }

            let digits_a = &a[start_i..i];
            let digits_b = &b[start_j..j];

            // Ignore leading zeroes when comparing magnitudes.
            let trimmed_a: String = digits_a.iter().skip_while(|c| **c == '0').collect();
            let trimmed_b: String = digits_b.iter().skip_while(|c| **c == '0').collect();

            let ord = trimmed_a
                .len()
                .cmp(&trimmed_b.len())
                .then_with(|| trimmed_a.cmp(&trimmed_b))
                .then_with(|| digits_a.len().cmp(&digits_b.len()));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let lower_a = ca.to_ascii_lowercase();
            let lower_b = cb.to_ascii_lowercase();
            let ord = lower_a.cmp(&lower_b).then_with(|| ca.cmp(&cb));
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }

    (a.len() - i).cmp(&(b.len() - j))
}

#[cfg(test)]
mod tests {
    use super::{Navigation, list_images, natural_cmp};
    use std::cmp::Ordering;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "picasa-rs-navigation-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn file_names(directory: &std::path::Path) -> Vec<String> {
        list_images(directory)
            .unwrap()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn lists_supported_images_case_insensitively_in_natural_order() {
        let directory = temp_dir("listing");
        for name in [
            "photo10.JPG",
            "photo2.jpg",
            "photo1.PnG",
            "notes.txt",
            "archive.zip",
            "photo02.jpeg",
        ] {
            fs::write(directory.join(name), b"not an image").unwrap();
        }
        fs::create_dir(directory.join("folder.jpg")).unwrap();

        assert_eq!(
            file_names(&directory),
            ["photo1.PnG", "photo2.jpg", "photo02.jpeg", "photo10.JPG"]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removing_advances_and_clamps_the_selection() {
        let mut navigation = Navigation {
            paths: ["a.jpg", "b.jpg", "c.jpg"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            index_by_path: HashMap::new(),
            current: 1,
        };
        navigation.index_by_path = navigation
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| (path.clone(), index))
            .collect();

        assert_eq!(navigation.remove_current(), Some(PathBuf::from("b.jpg")));
        assert_eq!(navigation.current(), 1);
        assert_eq!(navigation.current_path(), Some(Path::new("c.jpg")));

        assert_eq!(navigation.remove_current(), Some(PathBuf::from("c.jpg")));
        assert_eq!(navigation.current(), 0);
        assert_eq!(navigation.current_path(), Some(Path::new("a.jpg")));

        assert_eq!(navigation.remove_current(), Some(PathBuf::from("a.jpg")));
        assert_eq!(navigation.current_path(), None);
        assert_eq!(navigation.remove_current(), None);
    }

    #[test]
    fn natural_numbers() {
        assert_eq!(natural_cmp("photo2.jpg", "photo10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("photo10.jpg", "photo2.jpg"), Ordering::Greater);
        assert_eq!(natural_cmp("photo02.jpg", "photo2.jpg"), Ordering::Greater);
    }

    #[test]
    fn case_insensitive_prefixes() {
        assert_eq!(natural_cmp("A.jpg", "a.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("b.jpg", "A.jpg"), Ordering::Greater);
    }

    #[test]
    fn stable_for_identical() {
        assert_eq!(natural_cmp("same.jpg", "same.jpg"), Ordering::Equal);
    }
}
