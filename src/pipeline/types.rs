//! Types for the download pipeline.

use chrono::NaiveDate;
use polars::prelude::DataFrame;

/// Chunk of data produced by a provider and sent to the consumer.
#[derive(Debug)]
pub enum WindowChunk {
    /// A window of options data (may be one of many for a symbol).
    OptionsWindow {
        /// Symbol (uppercased).
        symbol: String,
        /// `DataFrame` with options data.
        df: DataFrame,
    },

    /// A complete prices dataset for a symbol.
    PricesComplete {
        /// Symbol (uppercased).
        symbol: String,
        /// `DataFrame` with OHLCV prices.
        df: DataFrame,
    },
}

/// Parameters for a download operation.
#[derive(Debug, Clone)]
pub struct DownloadParams {
    /// Period for historical data (prices): "1mo", "3mo", "6mo", "1y", "5y", "max".
    pub period: String,
}

impl Default for DownloadParams {
    fn default() -> Self {
        Self {
            period: "1y".to_string(),
        }
    }
}

/// Result of downloading data for a single symbol.
#[derive(Debug, Clone)]
pub struct DownloadResult {
    /// The symbol downloaded.
    pub symbol: String,

    /// Provider name (e.g., "EODHD", "Yahoo").
    pub provider: String,

    /// Number of new rows downloaded in this operation.
    pub new_rows: usize,

    /// Total rows in the cache after this operation.
    pub total_rows: usize,

    /// Date range (min, max) of the final cached data.
    pub date_range: Option<(NaiveDate, NaiveDate)>,

    /// Errors encountered (non-fatal).
    pub errors: Vec<String>,
}

impl DownloadResult {
    /// Create a successful result.
    pub fn success(
        symbol: String,
        provider: String,
        new_rows: usize,
        total_rows: usize,
        date_range: Option<(NaiveDate, NaiveDate)>,
    ) -> Self {
        Self {
            symbol,
            provider,
            new_rows,
            total_rows,
            date_range,
            errors: vec![],
        }
    }

    /// Create a result with errors.
    #[must_use]
    pub fn with_errors(mut self, errors: Vec<String>) -> Self {
        self.errors = errors;
        self
    }

    /// Check if the result represents success (no errors).
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}
