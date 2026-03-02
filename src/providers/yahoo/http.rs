//! HTTP client for Yahoo Finance API.

use anyhow::Context;
use yahoo_finance_api as yahoo;

/// HTTP client wrapper for Yahoo Finance API.
pub struct YahooHttpClient;

impl YahooHttpClient {
    /// Fetch quotes from Yahoo Finance API.
    pub async fn fetch_quotes(symbol: &str, period: &str) -> anyhow::Result<Vec<yahoo::Quote>> {
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
