# Adding a New Data Provider

This guide walks you through adding a new data provider to Inflow. The architecture is designed to be extensible—you only need to implement a simple trait and organize your code into focused modules.

## Overview

All data providers implement the `DataProvider` trait, which is registered in the pipeline during initialization. The trait handles concurrency, channel communication, and result aggregation automatically.

**Time estimate:** 1-2 hours for a straightforward API
**Example:** Adding an "Alpha Vantage" prices provider (~250 LOC)

---

## Step 1: Understand the DataProvider Trait

Located in `src/providers/mod.rs`:

```rust
#[async_trait]
pub trait DataProvider: Send + Sync {
    /// Human-readable provider name (e.g., "Yahoo", "EODHD").
    fn name(&self) -> &str;

    /// Category of data this provider supplies ("options" or "prices").
    fn category(&self) -> &str;

    /// Download data for a symbol.
    ///
    /// Called once per symbol. Send data chunks via `tx`.
    /// Return a DownloadResult summarizing the operation.
    async fn download(
        &self,
        symbol: &str,
        params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        shutdown: CancellationToken,
    ) -> Result<DownloadResult>;
}
```

**Key points:**
- `name()` and `category()` are metadata
- `download()` is where all work happens
- Send `WindowChunk` via `tx` as data becomes available
- Return a `DownloadResult` with summary statistics
- `shutdown` token allows graceful cancellation

---

## Step 2: Choose Complexity Level

### Simple (Yahoo pattern - 250 LOC)
Use this if your API:
- Returns complete dataset in a single request
- Has simple REST endpoint (no pagination needed)
- Returns data in a well-structured format (JSON, CSV)

**Structure:**
```
src/providers/alpha_vantage/
├── mod.rs        (~100 LOC) - Provider struct + trait impl
├── http.rs       (~30 LOC)  - HTTP client wrapper
└── parsing.rs    (~120 LOC) - DataFrame builder + 2 unit tests
```

### Complex (EODHD pattern - 750 LOC)
Use this if your API:
- Requires pagination or recursive windowing
- Has rate limits or complex retry logic
- Needs stateful request handling

**Structure:**
```
src/providers/eodhd/
├── mod.rs        (~113 LOC)  - Provider struct + trait impl
├── http.rs       (~143 LOC)  - HttpClient with retry/rate limiting
├── pagination.rs (~332 LOC)  - Pagination state machine
├── parsing.rs    (~152 LOC)  - DataFrame normalization + tests
└── types.rs      (~28 LOC)   - API response types
```

**This guide covers the simple (Yahoo) pattern.** See `src/providers/eodhd/` for complex examples.

---

## Step 3: Create the Module Directory

```bash
mkdir -p src/providers/alpha_vantage
touch src/providers/alpha_vantage/{mod.rs,http.rs,parsing.rs}
```

---

## Step 4: Implement HTTP Client (`http.rs`)

Start with a lightweight wrapper around your API:

```rust
//! HTTP client for Alpha Vantage API.

use anyhow::Context;

/// HTTP client wrapper for Alpha Vantage API.
pub struct AlphaVantageHttpClient;

impl AlphaVantageHttpClient {
    /// Fetch daily OHLCV quotes for a symbol.
    pub async fn fetch_quotes(
        symbol: &str,
        api_key: &str,
        period: &str,
    ) -> anyhow::Result<Vec<AlphaVantageQuote>> {
        let client = reqwest::Client::new();

        // Build query (adjust based on Alpha Vantage API)
        // period: "1d", "5m", etc.
        let url = format!(
            "https://www.alphavantage.co/query?function=TIME_SERIES_DAILY&symbol={}&apikey={}",
            symbol, api_key
        );

        let response = client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from Alpha Vantage API")?;

        let json = response
            .json::<serde_json::Value>()
            .await
            .context("Failed to parse Alpha Vantage response")?;

        // Parse response into Vec<AlphaVantageQuote>
        parse_alpha_vantage_response(&json)
    }
}

/// Represents a single quote from Alpha Vantage API.
#[derive(Debug, Clone)]
pub struct AlphaVantageQuote {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
}

fn parse_alpha_vantage_response(
    json: &serde_json::Value,
) -> anyhow::Result<Vec<AlphaVantageQuote>> {
    // TODO: Implement parsing based on Alpha Vantage JSON structure
    // This is API-specific; refer to Alpha Vantage documentation
    anyhow::bail!("TODO: Implement Alpha Vantage response parsing")
}
```

**Tips:**
- Keep this focused on HTTP communication only
- Don't parse into Polars DataFrames here
- Reuse `reqwest::Client` or add retry logic if needed
- Define intermediate types (e.g., `AlphaVantageQuote`) that match the API

---

## Step 5: Implement DataFrame Builder (`parsing.rs`)

Convert API responses into Polars DataFrames:

```rust
//! DataFrame construction from Alpha Vantage quotes.

use super::http::AlphaVantageQuote;
use crate::utils::PRICES_DATE_COLUMN;
use anyhow::Result;
use chrono::NaiveDate;
use polars::prelude::*;

/// Build a polars DataFrame from Alpha Vantage quotes.
///
/// Creates columns: date, open, high, low, close, volume
/// Skips quotes with invalid timestamps, logging a warning for each.
pub fn build_dataframe_from_quotes(
    quotes: &[AlphaVantageQuote],
    symbol: &str,
) -> Result<DataFrame> {
    let mut dates: Vec<NaiveDate> = Vec::with_capacity(quotes.len());
    let mut open: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut high: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut low: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut close: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut volume: Vec<u64> = Vec::with_capacity(quotes.len());

    for q in quotes {
        // Convert timestamp to NaiveDate
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
        volume.push(q.volume);
    }

    if dates.is_empty() {
        anyhow::bail!("All quotes for {symbol} had invalid timestamps");
    }

    // Create DataFrame with OHLCV columns
    let df = df! {
        "open" => &open,
        "high" => &high,
        "low" => &low,
        "close" => &close,
        "volume" => &volume,
    }?;

    // Add date column
    let date_series = DateChunked::from_naive_date(
        PlSmallStr::from(PRICES_DATE_COLUMN),
        dates.iter().copied(),
    )
    .into_column();

    let mut df = df.hstack(&[date_series])?;

    // Reorder so date is first
    df = df.select([PRICES_DATE_COLUMN, "open", "high", "low", "close", "volume"])?;

    Ok(df)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_dataframe_basic() {
        let quotes = vec![
            AlphaVantageQuote {
                timestamp: 1_609_459_200, // 2021-01-01
                open: 100.0,
                high: 105.0,
                low: 99.0,
                close: 103.0,
                volume: 1_000_000,
            },
            AlphaVantageQuote {
                timestamp: 1_609_545_600, // 2021-01-02
                open: 104.0,
                high: 106.0,
                low: 101.0,
                close: 102.0,
                volume: 900_000,
            },
        ];

        let df = build_dataframe_from_quotes(&quotes, "TEST")
            .expect("should build dataframe");

        assert_eq!(df.height(), 2);
        assert!(df.column(PRICES_DATE_COLUMN).is_ok());
    }

    #[test]
    fn test_build_dataframe_skips_invalid_timestamps() {
        let quotes = vec![
            AlphaVantageQuote {
                timestamp: 1_609_459_200,
                open: 100.0,
                high: 105.0,
                low: 99.0,
                close: 103.0,
                volume: 1_000_000,
            },
            AlphaVantageQuote {
                timestamp: i64::MIN, // Invalid
                open: 104.0,
                high: 106.0,
                low: 101.0,
                close: 102.0,
                volume: 900_000,
            },
        ];

        let df = build_dataframe_from_quotes(&quotes, "TEST")
            .expect("should build dataframe");

        assert_eq!(df.height(), 1); // Only valid row
    }
}
```

**Tips:**
- Use `PRICES_DATE_COLUMN` constant instead of hardcoding
- Handle timestamps carefully (timezones, invalid values)
- Add 2 unit tests (basic + edge case)
- Return `anyhow::Result<DataFrame>` for error context
- Use `tracing::warn!` for skipped rows

---

## Step 6: Implement Provider Struct (`mod.rs`)

Glue everything together:

```rust
//! Alpha Vantage data provider for OHLCV prices.

mod http;
mod parsing;

pub use http::AlphaVantageHttpClient;
pub use parsing::build_dataframe_from_quotes;

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::utils::{extract_date_range, PRICES_DATE_COLUMN};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Alpha Vantage data provider.
pub struct AlphaVantageProvider {
    api_key: String,
}

impl AlphaVantageProvider {
    /// Create a new Alpha Vantage provider.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait]
impl crate::providers::DataProvider for AlphaVantageProvider {
    fn name(&self) -> &str {
        "AlphaVantage"
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

        // 1. Fetch quotes from Alpha Vantage
        let quotes = AlphaVantageHttpClient::fetch_quotes(
            &symbol_upper,
            &self.api_key,
            &params.period,
        )
        .await?;

        if quotes.is_empty() {
            anyhow::bail!(
                "No data returned for {symbol_upper} (period: {})",
                params.period
            );
        }

        // 2. Build DataFrame
        let df = build_dataframe_from_quotes(&quotes, &symbol_upper)?;
        let new_rows = df.height();

        // 3. Send to consumer via channel
        tx.send(WindowChunk::PricesComplete {
            symbol: symbol_upper.clone(),
            df,
        })
        .await
        .ok();

        // 4. Read cache to get total rows and date range
        let prices_path = cache.prices_path(&symbol_upper)?;
        let cached_lf = cache.read_parquet(&prices_path).await?;

        let (total_rows, date_range) = if let Some(lf) = cached_lf {
            if let Ok(df) = lf.collect() {
                let rows = df.height();
                let date_range = extract_date_range(&df, PRICES_DATE_COLUMN);
                (rows, date_range)
            } else {
                (new_rows, None)
            }
        } else {
            (new_rows, None)
        };

        tracing::info!("AlphaVantage: {symbol_upper} completed ({new_rows} new rows)");

        Ok(DownloadResult::success(
            symbol_upper,
            self.name().to_string(),
            new_rows,
            total_rows,
            date_range,
        ))
    }
}
```

**Key points:**
- Import utilities: `extract_date_range`, `PRICES_DATE_COLUMN`
- Fetch → Parse → Send → Aggregate pattern
- Use `tracing::info!` for logging
- Return `DownloadResult::success()` (or `::failed()`)

---

## Step 7: Register Provider in `src/providers/mod.rs`

Add to the `build_providers()` function:

```rust
pub fn build_providers(config: &crate::Config) -> Vec<Arc<dyn DataProvider>> {
    let mut providers: Vec<Arc<dyn DataProvider>> = vec![];

    // Yahoo Finance provider (always enabled)
    providers.push(Arc::new(yahoo::YahooProvider::new()));

    // EODHD provider (if API key is present)
    if let Some(api_key) = &config.eodhd_api_key {
        providers.push(Arc::new(eodhd::EodhdProvider::new(
            api_key.clone(),
            config.eodhd_rate_limit,
        )));
    }

    // Alpha Vantage provider (if API key is present)
    if let Some(api_key) = &config.alpha_vantage_api_key {
        providers.push(Arc::new(alpha_vantage::AlphaVantageProvider::new(
            api_key.clone(),
        )));
    }

    providers
}
```

Don't forget to add the module declaration at the top:

```rust
pub mod alpha_vantage;  // Add this line
pub mod eodhd;
pub mod yahoo;
```

---

## Step 8: Add Config Support (`src/config.rs`)

Add API key field to `Config` struct:

```rust
pub struct Config {
    pub data_root: PathBuf,
    pub eodhd_api_key: Option<String>,
    pub eodhd_rate_limit: u32,
    pub alpha_vantage_api_key: Option<String>,  // Add this
}
```

Update the env var loader:

```rust
impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::from_path(/* ... */)?;

        Ok(Self {
            data_root: /* ... */,
            eodhd_api_key: env::var("EODHD_API_KEY").ok(),
            eodhd_rate_limit: /* ... */,
            alpha_vantage_api_key: env::var("ALPHA_VANTAGE_API_KEY").ok(),
        })
    }
}
```

---

## Step 9: Test Your Provider

### Unit Tests
Already added in `parsing.rs`. Run:

```bash
cargo test alpha_vantage
```

### Integration Test (Optional)
Add to `tests/consumer.rs` or create `tests/alpha_vantage.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_alpha_vantage_download() {
    let cache = temp_cache();
    let provider = AlphaVantageProvider::new("test_key".to_string());

    let params = DownloadParams {
        from_date: None,
        period: "1d".to_string(),
    };

    let (tx, mut rx) = mpsc::channel(10);
    let download = provider.download("SPY", &params, &cache, tx, CancellationToken::new());

    // Assert download succeeds and sends chunks
    // (Mock the HTTP client for reproducibility)
}
```

---

## Step 10: Manual Testing

```bash
# Set your API key
export ALPHA_VANTAGE_API_KEY=your_key_here

# Build and run
cargo build --release

# Test a single symbol
./target/release/inflow download prices AAPL --period 1d

# Check cache
./target/release/inflow status
```

---

## Checklist

- [ ] HTTP client implemented (`http.rs`)
- [ ] DataFrame parser implemented (`parsing.rs`)
- [ ] 2 unit tests added
- [ ] Provider struct implemented (`mod.rs`)
- [ ] `DataProvider` trait implemented
- [ ] Registered in `src/providers/mod.rs`
- [ ] Config support added (`src/config.rs`)
- [ ] Manual testing completed
- [ ] `cargo test` passes
- [ ] `cargo build --release` succeeds

---

## Troubleshooting

| Issue | Solution |
|-------|----------|
| "trait object cannot be send" | Ensure provider struct implements `Send + Sync` (usually automatic) |
| "timestamp parsing failed" | Check API response format and adjust parsing in `http.rs` |
| "DataFrame column mismatch" | Verify columns in `parsing.rs` match expectations, use constants |
| "API rate limit exceeded" | Add rate limiting logic to `http.rs` (see EODHD example) |
| "Empty results" | Check if API returns data for the symbol/period; add logging |

---

## References

- **Trait definition:** `src/providers/mod.rs`
- **Simple example:** `src/providers/yahoo/` (250 LOC)
- **Complex example:** `src/providers/eodhd/` (750 LOC, with pagination & rate limiting)
- **Utilities:** `src/utils/date.rs`, `src/utils/json.rs`, `src/utils/constants.rs`
- **Pipeline types:** `src/pipeline/types.rs` (`WindowChunk`, `DownloadResult`, `DownloadParams`)
- **Cache API:** `src/cache/store.rs` (`CacheStore::read_parquet()`, `atomic_write()`)

---

## Next Steps

1. **For options data:** Follow the EODHD pattern—you'll need pagination/windowing logic
2. **For rate limiting:** Extract logic to `src/utils/rate_limit.rs` for reuse
3. **For multiple symbols:** Ensure your provider handles `symbol` parameter in `download()`
4. **For caching:** Use `extract_date_range()` to calculate resume points

---

**Happy implementing! 🚀**
