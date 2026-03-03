//! Yahoo Finance data provider for OHLCV prices.
//!
//! Ports functionality from `optopy-mcp/src/tools/fetch.rs`.
//!
//! Supports incremental updates by computing resume date from cached data.

pub mod http;
mod parsing;

pub use http::{QuoteFetcher, YahooHttpClient};
pub use parsing::build_dataframe_from_quotes;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::{collect_blocking, compute_resume_date, extract_date_range, PRICES_DATE_COLUMN};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Yahoo Finance data provider.
pub struct YahooProvider {
    fetcher: Box<dyn QuoteFetcher>,
}

impl YahooProvider {
    /// Create a new Yahoo provider.
    pub fn new() -> Self {
        Self {
            fetcher: Box::new(YahooHttpClient),
        }
    }

    /// Create a provider with a custom quote fetcher (for testing).
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn with_fetcher(fetcher: Box<dyn QuoteFetcher>) -> Self {
        Self { fetcher }
    }
}

impl Default for YahooProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl crate::providers::DataProvider for YahooProvider {
    fn name(&self) -> &'static str {
        "Yahoo"
    }

    fn category(&self) -> &'static str {
        "prices"
    }

    async fn download(
        &self,
        symbol: &str,
        params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        let symbol_upper = symbol.to_uppercase();

        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("  {prefix:.bold} {spinner} {msg}")
                .expect("valid template"),
        );
        pb.set_prefix(format!("{symbol_upper} prices"));
        pb.enable_steady_tick(std::time::Duration::from_millis(120));
        pb.set_message("checking cache…");

        // Determine period: use cached resume date if available, otherwise use params
        let prices_path = cache.prices_path(&symbol_upper).map_err(|e| {
            pb.abandon_with_message(format!("cache error: {e}"));
            e
        })?;
        let cached_lf = cache.read_parquet(&prices_path).await.map_err(|e| {
            pb.abandon_with_message(format!("read error: {e}"));
            e
        })?;

        // Check for resume opportunity and determine period to fetch
        let fetch_period = if let Some(lf) = cached_lf.clone() {
            // Read cached data to check for resume opportunity
            match collect_blocking(lf).await {
                Ok(df) => {
                    // Found cached data - compute resume date and gap
                    if let Some(resume_date) =
                        compute_resume_date(&df, PRICES_DATE_COLUMN, None, None)
                    {
                        let today = Utc::now().date_naive();
                        let days_gap = (today - resume_date).num_days();

                        if days_gap <= 0 {
                            // Already up to date, no need to fetch
                            pb.finish_with_message(format!(
                                "up to date (last cached: {resume_date})"
                            ));
                            return Ok(DownloadResult::success(
                                symbol_upper,
                                self.name().to_string(),
                                0,
                                df.height(),
                                extract_date_range(&df, PRICES_DATE_COLUMN),
                            ));
                        }

                        // Determine appropriate period for the gap
                        let period = if days_gap < 30 {
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

                        pb.set_message(format!("fetching {period} ({days_gap} day gap)…"));
                        period.to_string()
                    } else {
                        // No resume date found, use params
                        params.period.clone()
                    }
                }
                _ => params.period.clone(),
            }
        } else {
            // No cached data, use params (full history fetch)
            pb.set_message(format!("fetching {} history…", params.period));
            params.period.clone()
        };

        let quotes = self
            .fetcher
            .fetch_quotes(&symbol_upper, &fetch_period)
            .await?;

        if quotes.is_empty() {
            pb.abandon_with_message("no data returned");
            anyhow::bail!("No data returned for {symbol_upper} (period: {fetch_period})");
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

        pb.finish_with_message(format!("{new_rows} new rows"));

        // Note: total_rows and date_range are populated by the orchestrator
        // after the consumer finishes writing to cache.
        Ok(DownloadResult::success(
            symbol_upper,
            self.name().to_string(),
            new_rows,
            0,
            None,
        ))
    }
}
