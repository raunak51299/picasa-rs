//! Persistent preferences stored in `%APPDATA%\picasa-rs\config.toml`.
//!
//! The file is a small, flat TOML document. Unknown keys are ignored so a
//! future version can add settings without breaking an older one.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::app::slideshow::{DEFAULT_INTERVAL, clamp_interval};

pub const DEFAULT_SCRIM_OPACITY: f32 = 0.75;
pub const DEFAULT_CONFIRM_DELETE: bool = true;

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub slideshow_interval: Duration,
    pub scrim_opacity: f32,
    pub confirm_delete: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            slideshow_interval: DEFAULT_INTERVAL,
            scrim_opacity: DEFAULT_SCRIM_OPACITY,
            confirm_delete: DEFAULT_CONFIRM_DELETE,
        }
    }
}

impl Preferences {
    /// Load the current user's preferences, falling back to defaults when the
    /// file is missing or malformed.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match Self::load_from(&path) {
            Ok(preferences) => preferences,
            Err(error) => {
                log::warn!(
                    "could not read preferences from {}: {error:#}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("invalid preferences in {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error)
                .with_context(|| format!("could not read preferences from {}", path.display())),
        }
    }

    pub fn save(&self) {
        let Some(path) = config_path() else {
            return;
        };
        if let Err(error) = self.save_to(&path) {
            log::warn!(
                "could not save preferences to {}: {error:#}",
                path.display()
            );
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create the preferences folder {}",
                    parent.display()
                )
            })?;
        }
        fs::write(path, self.to_toml())
            .with_context(|| format!("could not write preferences to {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut preferences = Self::default();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                bail!("line {}: expected `key = value`", index + 1);
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "slideshow_interval_secs" => {
                    let seconds: f64 = value.parse().with_context(|| {
                        format!(
                            "line {}: slideshow_interval_secs must be a number",
                            index + 1
                        )
                    })?;
                    if !seconds.is_finite() {
                        bail!("line {}: slideshow_interval_secs must be finite", index + 1);
                    }
                    preferences.slideshow_interval =
                        clamp_interval(Duration::from_secs_f64(seconds.max(0.0)));
                }
                "scrim_opacity" => {
                    let opacity: f32 = value.parse().with_context(|| {
                        format!("line {}: scrim_opacity must be a number", index + 1)
                    })?;
                    if !opacity.is_finite() {
                        bail!("line {}: scrim_opacity must be finite", index + 1);
                    }
                    preferences.scrim_opacity = opacity.clamp(0.0, 1.0);
                }
                "confirm_delete" => {
                    preferences.confirm_delete = match value {
                        "true" => true,
                        "false" => false,
                        _ => bail!("line {}: confirm_delete must be true or false", index + 1),
                    };
                }
                _ => {}
            }
        }
        Ok(preferences)
    }

    pub fn to_toml(&self) -> String {
        format!(
            "# picasa-rs preferences\n\
             slideshow_interval_secs = {}\n\
             scrim_opacity = {:.2}\n\
             confirm_delete = {}\n",
            self.slideshow_interval.as_secs(),
            self.scrim_opacity,
            self.confirm_delete
        )
    }

    /// Backdrop alpha as the 0-255 value used by the native startup preview.
    pub fn scrim_alpha(&self) -> u8 {
        (self.scrim_opacity.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

/// `%APPDATA%\picasa-rs\config.toml`, or `None` when `APPDATA` is unavailable.
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(|base| PathBuf::from(base).join("picasa-rs").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::Preferences;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    fn temp_path(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "picasa-rs-preferences-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        directory.join("config.toml")
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let preferences = Preferences::default();
        assert_eq!(
            Preferences::parse(&preferences.to_toml()).unwrap(),
            preferences
        );
    }

    #[test]
    fn comments_sections_and_unknown_keys_are_ignored() {
        let text = "\
# viewer settings
[slideshow]
slideshow_interval_secs = 9 # seconds

[look]
scrim_opacity = 0.4
future_setting = 'ignored'
confirm_delete = false
";
        let preferences = Preferences::parse(text).unwrap();
        assert_eq!(preferences.slideshow_interval, Duration::from_secs(9));
        assert_eq!(preferences.scrim_opacity, 0.4);
        assert!(!preferences.confirm_delete);
    }

    #[test]
    fn values_are_clamped_to_the_supported_ranges() {
        let preferences =
            Preferences::parse("slideshow_interval_secs = 1\nscrim_opacity = 2.0\n").unwrap();
        assert_eq!(preferences.slideshow_interval, Duration::from_secs(2));
        assert_eq!(preferences.scrim_opacity, 1.0);

        let preferences =
            Preferences::parse("slideshow_interval_secs = 120\nscrim_opacity = -1\n").unwrap();
        assert_eq!(preferences.slideshow_interval, Duration::from_secs(30));
        assert_eq!(preferences.scrim_opacity, 0.0);
        assert_eq!(preferences.scrim_alpha(), 0);
        assert_eq!(Preferences::default().scrim_alpha(), 191);
    }

    #[test]
    fn malformed_values_are_rejected() {
        assert!(Preferences::parse("slideshow_interval_secs = fast\n").is_err());
        assert!(Preferences::parse("scrim_opacity = NaN\n").is_err());
        assert!(Preferences::parse("confirm_delete = yes\n").is_err());
        assert!(Preferences::parse("not a key value pair\n").is_err());
    }

    #[test]
    fn preferences_save_and_load_from_disk() {
        let path = temp_path("save");
        let preferences = Preferences {
            slideshow_interval: Duration::from_secs(12),
            scrim_opacity: 0.5,
            confirm_delete: false,
        };
        preferences.save_to(&path).unwrap();
        assert_eq!(Preferences::load_from(&path).unwrap(), preferences);

        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_file_loads_defaults() {
        let path = temp_path("missing");
        assert_eq!(
            Preferences::load_from(&path).unwrap(),
            Preferences::default()
        );
    }
}
