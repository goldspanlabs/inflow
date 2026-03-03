//! Configuration loading from environment and `.env` files.

use crate::error::InflowError;
use std::path::PathBuf;

/// Runtime configuration for inflow.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root cache directory (default: `~/.optopsy/cache`).
    pub data_root: PathBuf,

    /// Optional EODHD API key. If unset, EODHD provider is disabled.
    pub eodhd_api_key: Option<String>,
}

impl Config {
    /// Load configuration from environment and `.env` files.
    ///
    /// Loads from (in order):
    /// 1. `~/.env`
    /// 2. `./.env` (current directory)
    /// 3. Environment variables
    #[allow(clippy::unnecessary_wraps)]
    pub fn from_env() -> Result<Self, InflowError> {
        // Load .env files
        if let Ok(home) = std::env::var("HOME") {
            let home_env = PathBuf::from(home).join(".env");
            dotenvy::from_path(&home_env).ok();
        }
        dotenvy::from_filename(".env").ok();

        // Read DATA_ROOT
        let data_root = if let Ok(root) = std::env::var("DATA_ROOT") {
            let expanded = shellexpand::tilde(&root);
            PathBuf::from(expanded.as_ref())
        } else {
            default_cache_dir()
        };

        // Read EODHD_API_KEY (optional)
        let eodhd_api_key = std::env::var("EODHD_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());

        Ok(Self {
            data_root,
            eodhd_api_key,
        })
    }
}

/// Default cache directory: `~/.optopsy/cache`
fn default_cache_dir() -> PathBuf {
    const TEMPLATE: &str = "~/.optopsy/cache";
    let expanded = shellexpand::tilde(TEMPLATE);
    // If tilde was not expanded (no home directory available), fall back to temp
    if expanded.as_ref() == TEMPLATE {
        return std::env::temp_dir().join("optopsy").join("cache");
    }
    PathBuf::from(expanded.as_ref())
}
