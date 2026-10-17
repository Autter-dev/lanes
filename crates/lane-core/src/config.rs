//! User settings: `~/.config/lane/config.toml` (`%APPDATA%\lane\config.toml` on Windows).
//!
//! Saves are atomic (write a temporary file, then rename), so a retry or a crash never leaves a
//! half-written file. `LANE_CONFIG_DIR` overrides the folder.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The settings file format version. Bump only with a migration.
pub const CONFIG_VERSION: u32 = 1;

const HEADER: &str = "# Lanes settings. Change them with `lane setup`, or edit this file.\n\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// The Lanes version that finished onboarding. `None` until the first run completes.
    pub onboarded: Option<String>,
    /// Send the anonymous install count. Environment opt-outs always win over this.
    pub telemetry: bool,
    /// Show tips in the terminal app.
    pub hints: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            onboarded: None,
            telemetry: true,
            hints: true,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    /// No home or config folder could be found.
    NoConfigDir,
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NoConfigDir => {
                write!(
                    f,
                    "couldn't find a folder for Lanes settings. Set LANE_CONFIG_DIR to a folder you can write to."
                )
            }
            ConfigError::Io { path, source } => {
                write!(
                    f,
                    "couldn't use {}: {source}. Check that the folder exists and you can write to it.",
                    path.display()
                )
            }
            ConfigError::Parse { path, message } => write!(
                f,
                "{} isn't valid settings ({message}). Fix it by hand, or delete it and run `lane setup`.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The folder Lanes keeps its settings in.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    if let Some(dir) = std::env::var_os("LANE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    use etcetera::BaseStrategy;
    let base = etcetera::choose_base_strategy().map_err(|_| ConfigError::NoConfigDir)?;
    Ok(base.config_dir().join("lane"))
}

/// The settings file.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

impl Config {
    pub fn is_onboarded(&self) -> bool {
        self.onboarded.is_some()
    }

    /// Load the settings, or the defaults when the file doesn't exist yet.
    pub fn load() -> Result<Config, ConfigError> {
        Config::load_from(&config_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse {
                path: path.to_path_buf(),
                message: e.message().to_string(),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Save the settings atomically. Returns the path written.
    pub fn save(&self) -> Result<PathBuf, ConfigError> {
        let path = config_path()?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io_err = |source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err)?;
        }
        let body = toml::to_string(self).expect("settings always serialize");
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, format!("{HEADER}{body}")).map_err(io_err)?;
        fs::rename(&tmp, path).map_err(|source| {
            let _ = fs::remove_file(&tmp);
            ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lane-config-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = temp_path("missing");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let config = Config {
            onboarded: Some("0.0.1".into()),
            telemetry: false,
            ..Config::default()
        };
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), config);
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "temporary file is cleaned up"
        );
        // Saving again over an existing file works (retries are safe).
        config.save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Lanes settings."));
    }

    #[test]
    fn unknown_and_missing_keys_are_tolerated() {
        let path = temp_path("partial");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "telemetry = false\nsomething_new = 3\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        assert!(!config.telemetry);
        assert!(config.hints, "missing keys fall back to defaults");
    }

    #[test]
    fn invalid_file_says_how_to_fix_it() {
        let path = temp_path("invalid");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "telemetry = maybe").unwrap();
        let err = Config::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("lane setup"), "{err}");
    }
}
