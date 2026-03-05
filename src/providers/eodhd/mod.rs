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
use crate::utils::{collect_blocking, compute_resume_date, OPTIONS_DATE_COLUMN};
use anyhow::Result;
use async_trait::async_trait;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use pagination::Paginator;
use polars::prelude::DataFrame;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Read a cached Parquet file and collect it into a `DataFrame`.
async fn collect_cached(cache: &CacheStore, path: &std::path::Path) -> Option<DataFrame> {
    let lf = cache.read_parquet(path).await.ok()??;
    collect_blocking(lf).await.ok()
}

/// EODHD options data provider.
pub struct EodhdProvider {
    paginator: Paginator,
}

impl EodhdProvider {
    /// Create a new EODHD provider.
    pub fn new(api_key: String) -> Self {
        Self {
            paginator: Paginator::new(api_key),
        }
    }

    /// Create a provider with a custom paginator (for testing).
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn with_paginator(paginator: Paginator) -> Self {
        Self { paginator }
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
        params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        let symbol = symbol.to_uppercase();
        let request_count_before = self.paginator.http.request_count.load(Ordering::Relaxed);
        let explicit_range = params.from_date.is_some();

        let bar_style = ProgressStyle::default_bar()
            .template("  {prefix:.bold} [{bar:30.cyan/dim}] {pos}/{len} windows  {msg}")
            .expect("valid template")
            .progress_chars("=> ");

        let mut errors: Vec<String> = Vec::new();
        let mut new_rows_total: usize = 0;

        // When explicit --from is set, skip cache/resume logic entirely
        let (cached_df, prices_df) = if explicit_range {
            (None, None)
        } else {
            let cd = collect_cached(cache, &cache.options_path(&symbol)?).await;
            let pd = collect_cached(cache, &cache.prices_path(&symbol)?).await;
            (cd, pd)
        };

        for option_type in &["call", "put"] {
            let pb = mp.add(ProgressBar::new(0));
            pb.set_style(bar_style.clone());
            pb.set_prefix(format!("{symbol} {option_type}s"));

            // Determine start date: explicit --from overrides resume logic
            let resume_from = if explicit_range {
                params.from_date
            } else if let Some(ref df) = cached_df {
                compute_resume_date(
                    df,
                    OPTIONS_DATE_COLUMN,
                    Some(("option_type", option_type)),
                    prices_df.as_ref(),
                )
            } else {
                None
            };

            let (new_rows, error) = self
                .paginator
                .fetch_all_for_type(
                    &symbol,
                    option_type,
                    resume_from,
                    params.to_date,
                    &tx,
                    &pb,
                    cache,
                )
                .await;

            if let Some(err) = error {
                pb.abandon_with_message(format!("error: {err}"));
                errors.push(format!("{option_type}: {err}"));
            }

            new_rows_total += new_rows;
        }

        let api_requests =
            self.paginator.http.request_count.load(Ordering::Relaxed) - request_count_before;
        mp.println(format!(
            "  EODHD: {symbol} completed ({api_requests} API requests)"
        ))
        .ok();

        // Note: total_rows and date_range are populated by the orchestrator
        // after the consumer finishes writing to cache.
        Ok(DownloadResult::success(
            symbol,
            self.name().to_string(),
            self.category().to_string(),
            new_rows_total,
            0,
            None,
        )
        .with_warnings(errors))
    }
}
