//! EODHD data provider for historical US equity options chains.
//!
//! Ports the key functionality from `optopy-mcp/src/data/eodhd.rs` with adaptations for the
//! `DataProvider` trait and pipeline architecture.

pub mod http;
pub mod pagination;
pub mod parsing;
pub mod types;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Duration;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use pagination::Paginator;
use polars::prelude::*;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Compute resume date for options, handling `option_type` filtering and weekend awareness.
///
/// For a given option type (call/put), finds the max `quote_date` in the cache,
/// then uses the prices cache to find the next trading day (skipping weekends/holidays).
fn compute_options_resume_date(
    df: &DataFrame,
    option_type: &str,
    prices_df: Option<&DataFrame>,
) -> Option<chrono::NaiveDate> {
    // Get the option_type column and filter for matching rows
    let type_col = df.column("option_type").ok()?;
    let type_str = type_col.str().ok()?;

    // Get the quote_date column and access its physical (i32) representation
    let date_col = df.column("quote_date").ok()?;
    let date_chunked = date_col.date().ok()?;
    let date_phys = &date_chunked.phys;

    // Filter to rows where option_type matches the current type's first letter
    let type_char = option_type.chars().next()?.to_lowercase().to_string();

    let mut max_date: Option<chrono::NaiveDate> = None;

    // Iterate through both columns together
    let type_strs: Vec<Option<&str>> = type_str.iter().collect();
    let date_vals: Vec<Option<i32>> = date_phys.iter().collect();

    for (opt_type_str, date_val) in type_strs.iter().zip(date_vals.iter()) {
        if let (Some(ot), Some(date_i32)) = (opt_type_str, date_val) {
            let ot_first_char = ot.chars().next();
            if ot_first_char.map(|c: char| c.to_lowercase().to_string()) == Some(type_char.clone())
            {
                // Convert i32 days since epoch to NaiveDate
                // Polars uses days since 1900-01-01, so offset is (1900 - -4713) * 365.25 ≈ 719_163
                if let Some(date) = chrono::NaiveDate::from_num_days_from_ce_opt(date_i32 + 719_162)
                {
                    if max_date.is_none() || date > max_date.unwrap() {
                        max_date = Some(date);
                    }
                }
            }
        }
    }

    // Find the next trading day after max_date using prices cache
    max_date.map(|d| {
        let candidate = d + Duration::days(1);

        // If prices cache available, find next trading day (skip weekends/holidays)
        if let Some(prices) = prices_df {
            if let Ok(price_date_col) = prices.column("date") {
                if let Ok(price_dates) = price_date_col.date() {
                    let price_dates_phys = &price_dates.phys;

                    // Collect all available trading dates
                    let trading_dates: Vec<chrono::NaiveDate> = price_dates_phys
                        .iter()
                        .filter_map(|d_i32| {
                            d_i32.and_then(|di| {
                                chrono::NaiveDate::from_num_days_from_ce_opt(di + 719_162)
                            })
                        })
                        .collect();

                    // Find first trading date >= candidate
                    for trading_date in trading_dates {
                        if trading_date >= candidate {
                            tracing::debug!(
                                "Resuming {option_type} options from {trading_date} (latest cached: {d}, \
                                 skipping weekends/holidays)"
                            );
                            return trading_date;
                        }
                    }

                    // No future trading date found in prices cache
                    tracing::debug!(
                        "No future trading date found in prices cache after {d}. \
                         Resuming from calendar date {candidate} (may skip to next trading day on retry)"
                    );
                }
            }
        }

        // Fallback: return calendar day + 1 (market closed on weekends, so this will be caught on retry)
        tracing::info!(
            "Resuming {option_type} options from {candidate} (latest cached: {d})"
        );
        candidate
    })
}

/// EODHD options data provider.
pub struct EodhdProvider {
    paginator: Paginator,
}

impl EodhdProvider {
    /// Create a new EODHD provider.
    pub fn new(api_key: String, _rate_limit_per_sec: u32) -> Self {
        Self {
            paginator: Paginator::new(api_key),
        }
    }
}

#[async_trait]
impl crate::providers::DataProvider for EodhdProvider {
    fn name(&self) -> &'static str {
        "EODHD"
    }

    fn category(&self) -> &'static str {
        "options"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
    ) -> Result<DownloadResult> {
        let symbol = symbol.to_uppercase();
        let request_count_before = self.paginator.http.request_count.load(Ordering::Relaxed);

        let mp = MultiProgress::new();
        let bar_style = ProgressStyle::default_bar()
            .template("  {prefix:.bold} [{bar:30.cyan/dim}] {pos}/{len} windows  {msg}")
            .expect("valid template")
            .progress_chars("=> ");

        let mut errors: Vec<String> = Vec::new();
        let mut new_rows_total: usize = 0;

        // Check cache to enable resume: find the latest quote_date for each option_type
        let cached_options = cache
            .read_parquet(&cache.options_path(&symbol).unwrap_or_default())
            .await
            .ok()
            .flatten();
        let cached_df = if let Some(lf) = cached_options {
            tokio::task::spawn_blocking(move || lf.collect().ok())
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // Load prices cache for weekend-aware trading day detection
        let cached_prices = cache
            .read_parquet(&cache.prices_path(&symbol).unwrap_or_default())
            .await
            .ok()
            .flatten();
        let prices_df = if let Some(lf) = cached_prices {
            tokio::task::spawn_blocking(move || lf.collect().ok())
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        for option_type in &["call", "put"] {
            let pb = mp.add(ProgressBar::new(0));
            pb.set_style(bar_style.clone());
            pb.set_prefix(format!("{symbol} {option_type}s"));

            // Determine resume point from cache
            let resume_from = if let Some(ref df) = cached_df {
                // Filter to this option type and find max quote_date
                compute_options_resume_date(df, option_type, prices_df.as_ref())
            } else {
                None
            };

            let (new_rows, error) = self
                .paginator
                .fetch_all_for_type(&symbol, option_type, resume_from, &tx, &pb, cache)
                .await;

            if let Some(err) = error {
                pb.abandon_with_message(format!("error: {err}"));
                errors.push(format!("{option_type}: {err}"));
            }

            new_rows_total += new_rows;
        }

        let api_requests =
            self.paginator.http.request_count.load(Ordering::Relaxed) - request_count_before;
        tracing::info!("EODHD: {symbol} completed ({api_requests} API requests)");

        // Note: total_rows and date_range are populated by the orchestrator
        // after the consumer finishes writing to cache.
        Ok(
            DownloadResult::success(symbol, self.name().to_string(), new_rows_total, 0, None)
                .with_warnings(errors),
        )
    }
}
