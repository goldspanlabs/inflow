//! Yahoo Finance data provider for OHLCV prices.
//!
//! Ports functionality from `optopy-mcp/src/tools/fetch.rs`.

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::extract_date_range;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::NaiveDate;
use polars::prelude::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yahoo_finance_api as yahoo;

pub struct YahooProvider;

impl YahooProvider {
    /// Create a new Yahoo provider.
    pub fn new() -> Self {
        Self
    }

    /// Fetch quotes from Yahoo Finance API.
    async fn fetch_quotes(&self, symbol: &str, period: &str) -> Result<Vec<yahoo::Quote>> {
        let provider = yahoo::YahooConnector::new()
            .context("Failed to create Yahoo Finance connector")?;
        let resp = provider
            .get_quote_range(symbol, "1d", period)
            .await
            .with_context(|| format!("Failed to fetch data for {symbol} (period: {period})"))?;
        resp.quotes()
            .with_context(|| format!("Failed to parse quotes for {symbol}"))
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

        let quotes = self.fetch_quotes(&symbol_upper, &params.period).await?;

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
            if let Ok(df) = lf.collect() {
                let rows = df.height();
                let date_range = extract_date_range(&df, "date");
                (rows, date_range)
            } else {
                (new_rows, None)
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

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn build_dataframe_from_quotes(quotes: &[yahoo::Quote], symbol: &str) -> Result<DataFrame> {
    let mut dates: Vec<NaiveDate> = Vec::with_capacity(quotes.len());
    let mut open: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut high: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut low: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut close: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut adjclose: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut volume: Vec<u64> = Vec::with_capacity(quotes.len());

    for q in quotes {
        let Some(dt) = chrono::DateTime::from_timestamp(q.timestamp, 0) else {
            tracing::warn!(
                timestamp = q.timestamp,
                "Skipping quote with invalid timestamp"
            );
            continue;
        };
        dates.push(dt.naive_utc().date());
        open.push(q.open);
        high.push(q.high);
        low.push(q.low);
        close.push(q.close);
        adjclose.push(q.adjclose);
        volume.push(q.volume);
    }

    if dates.is_empty() {
        anyhow::bail!("All quotes for {symbol} had invalid timestamps");
    }

    let df = df! {
        "open" => &open,
        "high" => &high,
        "low" => &low,
        "close" => &close,
        "adjclose" => &adjclose,
        "volume" => &volume,
    }?;

    // Add date column
    let date_series = DateChunked::from_naive_date(
        PlSmallStr::from("date"),
        dates.iter().copied(),
    )
    .into_column();

    let mut df = df.hstack(&[date_series])?;

    // Reorder so date is first
    df = df.select(["date", "open", "high", "low", "close", "adjclose", "volume"])?;

    Ok(df)
}

