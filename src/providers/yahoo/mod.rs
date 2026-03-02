//! Yahoo Finance data provider for OHLCV prices.
//!
//! Ports functionality from `optopy-mcp/src/tools/fetch.rs`.
//!
//! Supports incremental updates by computing resume date from cached data.

mod http;
mod parsing;

pub use http::YahooHttpClient;
pub use parsing::build_dataframe_from_quotes;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::{compute_resume_date, extract_date_range, PRICES_DATE_COLUMN};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
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

        // Determine period: use cached resume date if available, otherwise use params
        let prices_path = cache.prices_path(&symbol_upper)?;
        let cached_lf = cache.read_parquet(&prices_path).await?;

        let fetch_period = if let Some(lf) = cached_lf.clone() {
            // Read cached data to check for resume opportunity
            match tokio::task::spawn_blocking(move || lf.collect()).await {
                Ok(Ok(df)) => {
                    // Found cached data - compute resume date and gap
                    if let Some(resume_date) = compute_resume_date(&df, PRICES_DATE_COLUMN) {
                        let today = Utc::now().date_naive();
                        let days_gap = (today - resume_date).num_days();

                        if days_gap <= 0 {
                            // Already up to date, no need to fetch
                            tracing::info!(
                                "Yahoo: {symbol_upper} already up to date (last cached: {resume_date})"
                            );
                            return Ok(DownloadResult::success(
                                symbol_upper,
                                self.name().to_string(),
                                0,
                                df.height(),
                                extract_date_range(&df, PRICES_DATE_COLUMN),
                            ));
                        }

                        // Determine appropriate period for the gap
                        let gap_period = if days_gap < 30 {
                            "1mo"
                        } else if days_gap < 90 {
                            "3mo"
                        } else if days_gap < 180 {
                            "6mo"
                        } else if days_gap < 365 {
                            "1y"
                        } else {
                            "5y"
                        };

                        tracing::info!(
                            "Yahoo: {symbol_upper} gap detected ({days_gap} days), \
                             fetching {gap_period} to fill from {resume_date} to {today}"
                        );
                        gap_period.to_string()
                    } else {
                        // No resume date found, use params
                        params.period.clone()
                    }
                }
                _ => params.period.clone(),
            }
        } else {
            // No cached data, use params (full history fetch)
            params.period.clone()
        };

        let quotes = YahooHttpClient::fetch_quotes(&symbol_upper, &fetch_period).await?;

        if quotes.is_empty() {
            anyhow::bail!("No data returned for {symbol_upper} (period: {})", fetch_period);
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

        // Read cache to get totals (including newly merged data)
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
