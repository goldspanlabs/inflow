//! Data providers (EODHD, Yahoo Finance, etc.).

pub mod eodhd;
pub mod yahoo;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use anyhow::Result;
use async_trait::async_trait;
use indicatif::MultiProgress;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Trait for data providers.
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Provider name (e.g., "EODHD", "Yahoo").
    fn name(&self) -> &'static str;

    /// Category of data (e.g., "options", "prices").
    fn category(&self) -> &'static str;

    /// Download data for a symbol.
    ///
    /// Sends data chunks via `tx` as windows are completed.
    /// Progress bars should be added to `mp` for coordinated terminal output.
    /// Returns a summary of the download.
    async fn download(
        &self,
        symbol: &str,
        params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        shutdown: CancellationToken,
        mp: &MultiProgress,
    ) -> Result<DownloadResult>;
}

/// Build the list of enabled providers based on configuration.
pub fn build_providers(config: &crate::Config) -> Vec<Arc<dyn DataProvider>> {
    let mut providers: Vec<Arc<dyn DataProvider>> = vec![];

    // Yahoo Finance provider (always enabled)
    providers.push(Arc::new(yahoo::YahooProvider::new()));

    // EODHD provider (if API key is present)
    if let Some(api_key) = &config.eodhd_api_key {
        providers.push(Arc::new(eodhd::EodhdProvider::new(api_key.clone())));
    }

    providers
}

/// Filter providers by category (options, prices, etc.).
pub fn filter_providers_by_category(
    providers: &[Arc<dyn DataProvider>],
    category: &str,
) -> Vec<Arc<dyn DataProvider>> {
    providers
        .iter()
        .filter(|p| p.category() == category)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_providers_without_api_key() {
        let config = crate::Config {
            data_root: std::env::temp_dir(),
            eodhd_api_key: None,
        };
        let providers = build_providers(&config);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name(), "Yahoo");
    }

    #[test]
    fn test_build_providers_with_api_key() {
        let config = crate::Config {
            data_root: std::env::temp_dir(),
            eodhd_api_key: Some("test-key".into()),
        };
        let providers = build_providers(&config);
        assert_eq!(providers.len(), 2);
        let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"Yahoo"));
        assert!(names.contains(&"EODHD"));
    }

    #[test]
    fn test_filter_providers_by_options() {
        let config = crate::Config {
            data_root: std::env::temp_dir(),
            eodhd_api_key: Some("test-key".into()),
        };
        let providers = build_providers(&config);
        let filtered = filter_providers_by_category(&providers, "options");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name(), "EODHD");
    }

    #[test]
    fn test_filter_providers_by_prices() {
        let config = crate::Config {
            data_root: std::env::temp_dir(),
            eodhd_api_key: Some("test-key".into()),
        };
        let providers = build_providers(&config);
        let filtered = filter_providers_by_category(&providers, "prices");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name(), "Yahoo");
    }

    #[test]
    fn test_filter_providers_unknown_category() {
        let config = crate::Config {
            data_root: std::env::temp_dir(),
            eodhd_api_key: Some("test-key".into()),
        };
        let providers = build_providers(&config);
        let filtered = filter_providers_by_category(&providers, "crypto");
        assert!(filtered.is_empty());
    }
}
