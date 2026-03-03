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
