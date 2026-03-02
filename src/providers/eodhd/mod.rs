//! EODHD data provider for historical US equity options chains.
//!
//! Ports the key functionality from `optopy-mcp/src/data/eodhd.rs` with adaptations for the
//! DataProvider trait and pipeline architecture.

pub mod http;
pub mod pagination;
pub mod parsing;
pub mod types;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::extract_date_range;
use anyhow::Result;
use async_trait::async_trait;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use pagination::Paginator;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
    fn name(&self) -> &str {
        "EODHD"
    }

    fn category(&self) -> &str {
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

        for option_type in &["call", "put"] {
            let pb = mp.add(ProgressBar::new(0));
            pb.set_style(bar_style.clone());
            pb.set_prefix(format!("{symbol} {option_type}s"));

            let (new_rows, error) = self
                .paginator
                .fetch_all_for_type(&symbol, option_type, None, &tx, &pb)
                .await;

            if let Some(err) = error {
                pb.abandon_with_message(format!("error: {err}"));
                errors.push(format!("{option_type}: {err}"));
            }

            new_rows_total += new_rows;
        }

        // Read cache to get totals
        let options_path = cache.options_path(&symbol)?;
        let cached_lf = cache.read_parquet(&options_path).await?;

        let (total_rows, date_range) = if let Some(lf) = cached_lf {
            if let Ok(df) = lf.collect() {
                let rows = df.height();
                let date_range = extract_date_range(&df, "quote_date");
                (rows, date_range)
            } else {
                (0, None)
            }
        } else {
            (0, None)
        };

        let api_requests =
            self.paginator.http.request_count.load(Ordering::Relaxed) - request_count_before;
        tracing::info!("EODHD: {symbol} completed ({api_requests} API requests)");

        Ok(DownloadResult::success(
            symbol,
            self.name().to_string(),
            new_rows_total,
            total_rows,
            date_range,
        )
        .with_errors(errors))
    }
}
