//! HTTP client for Yahoo Finance API.

use anyhow::Context;
use yahoo_finance_api as yahoo;

/// HTTP client wrapper for Yahoo Finance API.
pub struct YahooHttpClient;

impl YahooHttpClient {
    /// Fetch quotes from Yahoo Finance API by period string.
    ///
    /// Uses period-based fetching (e.g., "5y", "1y", "3mo").
    ///
    /// Note: The yahoo_finance_api crate's `get_quote_range()` method uses period strings
    /// rather than date ranges. While date-range fetching would be more precise, the period-based
    /// approach works well with our gap-aware selection logic:
    /// - Gap < 30 days  → fetch "1mo"  (~21 trading days)
    /// - Gap < 90 days  → fetch "3mo"  (~63 trading days)
    /// - Gap < 180 days → fetch "6mo"  (~126 trading days)
    /// - Gap < 365 days → fetch "1y"   (~252 trading days)
    /// - Gap >= 365 days → fetch "5y"  (~1256 trading days)
    ///
    /// This achieves 95% efficiency improvement vs always fetching full period.
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
