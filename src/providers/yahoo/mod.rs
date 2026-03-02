//! Yahoo Finance data provider for OHLCV prices.
//!
//! Ports functionality from `optopy-mcp/src/tools/fetch.rs`.

mod http;
mod parsing;

pub use http::YahooHttpClient;
pub use parsing::build_dataframe_from_quotes;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::{extract_date_range, PRICES_DATE_COLUMN};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Yahoo Finance data provider.
pub struct YahooProvider;

impl YahooProvider {
    /// Create a new Yahoo provider.
    pub fn new() -> Self {
        Self
    }
}

impl Default for YahooProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl crate::providers::DataProvider for YahooProvider {
    fn name(&self) -> &str {
        "Yahoo"
    }

    fn category(&self) -> &str {
        "prices"
    }

    async fn download(
        &self,
        symbol: &str,
        params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
    ) -> Result<DownloadResult> {
        let symbol_upper = symbol.to_uppercase();

        let quotes = YahooHttpClient::fetch_quotes(&symbol_upper, &params.period).await?;

        if quotes.is_empty() {
            anyhow::bail!("No data returned for {symbol_upper} (period: {})", params.period);
        }

        let df = build_dataframe_from_quotes(&quotes, &symbol_upper)?;
        let new_rows = df.height();

        // Send the complete prices DataFrame as a single chunk
        tx.send(WindowChunk::PricesComplete {
            symbol: symbol_upper.clone(),
            df,
        })
        .await
        .ok();

        // Read cache to get totals
        let prices_path = cache.prices_path(&symbol_upper)?;
        let cached_lf = cache.read_parquet(&prices_path).await?;

        let (total_rows, date_range) = if let Some(lf) = cached_lf {
            // Use spawn_blocking for the blocking Polars collect operation
            match tokio::task::spawn_blocking(move || lf.collect()).await {
                Ok(Ok(df)) => {
                    let rows = df.height();
                    let date_range = extract_date_range(&df, PRICES_DATE_COLUMN);
                    (rows, date_range)
                }
                _ => (new_rows, None),
            }
        } else {
            (new_rows, None)
        };

        tracing::info!("Yahoo: {symbol_upper} completed ({new_rows} new rows)");

        Ok(DownloadResult::success(
            symbol_upper,
            self.name().to_string(),
            new_rows,
            total_rows,
            date_range,
        ))
    }
}
