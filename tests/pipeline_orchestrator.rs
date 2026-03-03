//! Integration tests for Pipeline::run() using mock providers.
//!
//! Tests the full orchestrator flow: producers → mpsc channel → consumer → cache writes,
//! without hitting any real APIs.

mod common;

use anyhow::Result;
use async_trait::async_trait;
use chrono::NaiveDate;
use indicatif::MultiProgress;
use inflow::cache::CacheStore;
use inflow::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use inflow::pipeline::Pipeline;
use inflow::providers::DataProvider;
use inflow::utils::collect_blocking;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Mock providers
// ---------------------------------------------------------------------------

/// A mock options provider that sends canned OptionsWindow chunks.
///
/// Uses "EODHD" as the provider name so the orchestrator's post-write
/// logic (which hardcodes `provider == "EODHD"`) reads from the options path.
// TODO(#4): Rename to "MockOptions" once orchestrator uses category instead of provider name.
struct MockOptionsProvider {
    /// Rows to generate per symbol. Each row gets a distinct quote_date.
    rows_per_symbol: usize,
}

impl MockOptionsProvider {
    fn new(rows_per_symbol: usize) -> Self {
        Self { rows_per_symbol }
    }
}

#[async_trait]
impl DataProvider for MockOptionsProvider {
    fn name(&self) -> &'static str {
        // TODO(#4): Change to "MockOptions" once orchestrator uses category.
        "EODHD"
    }

    fn category(&self) -> &'static str {
        "options"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        _cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        _mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        let dates: Vec<String> = (0..self.rows_per_symbol)
            .map(|i| {
                NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .checked_add_days(chrono::Days::new(i as u64))
                    .unwrap()
                    .to_string()
            })
            .collect();
        let strikes: Vec<f64> = vec![100.0; self.rows_per_symbol];
        let date_strs: Vec<&str> = dates.iter().map(|s| s.as_str()).collect();
        let df = common::create_options_df(&date_strs, &strikes, Some(symbol));
        let rows = df.height();

        tx.send(WindowChunk::OptionsWindow {
            symbol: symbol.to_uppercase(),
            df,
        })
        .await?;
        Ok(DownloadResult::success(
            symbol.to_uppercase(),
            self.name().to_string(),
            rows,
            0, // total_rows populated by orchestrator post-write
            None,
        ))
    }
}

/// A mock prices provider that sends canned PricesComplete chunks.
struct MockPricesProvider {
    rows_per_symbol: usize,
}

impl MockPricesProvider {
    fn new(rows_per_symbol: usize) -> Self {
        Self { rows_per_symbol }
    }
}

#[async_trait]
impl DataProvider for MockPricesProvider {
    fn name(&self) -> &'static str {
        "MockPrices"
    }

    fn category(&self) -> &'static str {
        "prices"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        _cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        _mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        let df = common::create_prices_df(self.rows_per_symbol);
        let rows = df.height();
        tx.send(WindowChunk::PricesComplete {
            symbol: symbol.to_uppercase(),
            df,
        })
        .await?;
        Ok(DownloadResult::success(
            symbol.to_uppercase(),
            self.name().to_string(),
            rows,
            0,
            None,
        ))
    }
}

/// A mock provider that always fails with an error.
struct MockFailingProvider;

#[async_trait]
impl DataProvider for MockFailingProvider {
    fn name(&self) -> &'static str {
        "MockFailing"
    }

    fn category(&self) -> &'static str {
        "options"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        _cache: &CacheStore,
        _tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        _mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        Err(anyhow::anyhow!("Simulated API failure for {symbol}"))
    }
}

/// A mock provider that sends zero chunks (empty download).
struct MockEmptyProvider;

#[async_trait]
impl DataProvider for MockEmptyProvider {
    fn name(&self) -> &'static str {
        "MockEmpty"
    }

    fn category(&self) -> &'static str {
        "prices"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        _cache: &CacheStore,
        _tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        _mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        // Send nothing — simulates "no new data available"
        Ok(DownloadResult::success(
            symbol.to_uppercase(),
            self.name().to_string(),
            0,
            0,
            None,
        ))
    }
}

/// A mock provider that tracks concurrent active downloads via an AtomicUsize.
///
/// Used to verify that the pipeline's semaphore actually limits concurrency.
struct MockConcurrencyTracker {
    active: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
}

impl MockConcurrencyTracker {
    fn new(active: Arc<AtomicUsize>, max_seen: Arc<AtomicUsize>) -> Self {
        Self { active, max_seen }
    }
}

#[async_trait]
impl DataProvider for MockConcurrencyTracker {
    fn name(&self) -> &'static str {
        "MockTracker"
    }

    fn category(&self) -> &'static str {
        "prices"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        _cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
        _mp: &MultiProgress,
    ) -> Result<DownloadResult> {
        // Increment active count and record the high-water mark
        let prev = self.active.fetch_add(1, Ordering::SeqCst);
        let current = prev + 1;
        self.max_seen.fetch_max(current, Ordering::SeqCst);

        // Simulate some async work so other tasks get a chance to overlap
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let df = common::create_prices_df(1);
        tx.send(WindowChunk::PricesComplete {
            symbol: symbol.to_uppercase(),
            df,
        })
        .await?;

        // Decrement active count
        self.active.fetch_sub(1, Ordering::SeqCst);

        Ok(DownloadResult::success(
            symbol.to_uppercase(),
            self.name().to_string(),
            1,
            0,
            None,
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_single_options_provider_single_symbol() {
    let cache = common::temp_cache();
    let pipeline = Pipeline {
        providers: vec![Arc::new(MockOptionsProvider::new(5))],
        cache: Arc::clone(&cache),
        symbols: vec!["SPY".to_string()],
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].symbol, "SPY");
    assert_eq!(results[0].provider, "EODHD");
    assert!(results[0].is_success());
    assert_eq!(results[0].new_rows, 5);

    // Verify data was written to cache
    let path = cache.options_path("SPY").unwrap();
    assert!(path.exists(), "Options parquet file should exist");

    let lf = cache.read_parquet(&path).await.unwrap().unwrap();
    let df = collect_blocking(lf).await.unwrap();
    assert_eq!(df.height(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_single_prices_provider_single_symbol() {
    let cache = common::temp_cache();
    let pipeline = Pipeline {
        providers: vec![Arc::new(MockPricesProvider::new(10))],
        cache: Arc::clone(&cache),
        symbols: vec!["AAPL".to_string()],
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].symbol, "AAPL");
    assert_eq!(results[0].provider, "MockPrices");
    assert!(results[0].is_success());

    // Verify data was written to cache
    let path = cache.prices_path("AAPL").unwrap();
    assert!(path.exists(), "Prices parquet file should exist");

    let lf = cache.read_parquet(&path).await.unwrap().unwrap();
    let df = collect_blocking(lf).await.unwrap();
    assert_eq!(df.height(), 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_multiple_symbols() {
    let cache = common::temp_cache();
    let symbols = vec!["SPY".to_string(), "QQQ".to_string(), "IWM".to_string()];

    let pipeline = Pipeline {
        providers: vec![Arc::new(MockPricesProvider::new(5))],
        cache: Arc::clone(&cache),
        symbols: symbols.clone(),
        params: DownloadParams::default(),
        concurrency: 4,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 3);

    let result_symbols: std::collections::HashSet<String> =
        results.iter().map(|r| r.symbol.clone()).collect();
    for sym in &symbols {
        assert!(result_symbols.contains(sym), "Missing result for {sym}");
    }

    // All files should exist in cache
    for sym in &symbols {
        let path = cache.prices_path(sym).unwrap();
        assert!(path.exists(), "Prices file missing for {sym}");

        let lf = cache.read_parquet(&path).await.unwrap().unwrap();
        let df = collect_blocking(lf).await.unwrap();
        assert_eq!(df.height(), 5, "Wrong row count for {sym}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_multiple_providers_same_symbol() {
    let cache = common::temp_cache();

    // Both an options and prices provider for the same symbol
    let pipeline = Pipeline {
        providers: vec![
            Arc::new(MockOptionsProvider::new(3)),
            Arc::new(MockPricesProvider::new(7)),
        ],
        cache: Arc::clone(&cache),
        symbols: vec!["SPY".to_string()],
        params: DownloadParams::default(),
        concurrency: 4,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    // Should get 2 results: one per provider
    assert_eq!(results.len(), 2);

    let providers: std::collections::HashSet<String> =
        results.iter().map(|r| r.provider.clone()).collect();
    assert!(providers.contains("EODHD"));
    assert!(providers.contains("MockPrices"));

    // Both files should exist
    let options_path = cache.options_path("SPY").unwrap();
    let prices_path = cache.prices_path("SPY").unwrap();
    assert!(options_path.exists(), "Options file should exist");
    assert!(prices_path.exists(), "Prices file should exist");

    // Verify row counts
    let opt_df = collect_blocking(cache.read_parquet(&options_path).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(opt_df.height(), 3);

    let price_df = collect_blocking(cache.read_parquet(&prices_path).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(price_df.height(), 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_provider_error_returns_result_with_errors() {
    let cache = common::temp_cache();
    let pipeline = Pipeline {
        providers: vec![Arc::new(MockFailingProvider)],
        cache: Arc::clone(&cache),
        symbols: vec!["FAIL".to_string()],
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline
        .run()
        .await
        .expect("Pipeline itself should not fail");

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_success(), "Result should contain errors");
    assert!(
        results[0].errors[0].contains("Simulated API failure"),
        "Error message should describe the failure"
    );

    // No file should be written for the failed symbol
    let path = cache.options_path("FAIL").unwrap();
    assert!(
        !path.exists(),
        "No cache file should exist for failed download"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_mixed_success_and_failure() {
    let cache = common::temp_cache();

    // Use a prices provider for one symbol and a failing provider for another.
    // Since both providers run for all symbols, we test that failures for one
    // provider don't block the other.
    let pipeline = Pipeline {
        providers: vec![
            Arc::new(MockPricesProvider::new(5)),
            Arc::new(MockFailingProvider),
        ],
        cache: Arc::clone(&cache),
        symbols: vec!["SPY".to_string()],
        params: DownloadParams::default(),
        concurrency: 4,
    };

    let results = pipeline.run().await.expect("Pipeline should not fail");

    assert_eq!(results.len(), 2);

    let successful: Vec<_> = results.iter().filter(|r| r.is_success()).collect();
    let failed: Vec<_> = results.iter().filter(|r| !r.is_success()).collect();

    assert_eq!(successful.len(), 1, "One provider should succeed");
    assert_eq!(failed.len(), 1, "One provider should fail");
    assert_eq!(successful[0].provider, "MockPrices");
    assert_eq!(failed[0].provider, "MockFailing");

    // The prices file should still exist despite the other provider failing
    let path = cache.prices_path("SPY").unwrap();
    assert!(
        path.exists(),
        "Successful provider's file should still be written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_empty_provider_no_crash() {
    let cache = common::temp_cache();
    let pipeline = Pipeline {
        providers: vec![Arc::new(MockEmptyProvider)],
        cache: Arc::clone(&cache),
        symbols: vec!["SPY".to_string()],
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 1);
    assert!(results[0].is_success());
    assert_eq!(results[0].new_rows, 0);

    // No file should be created since no data was sent
    let path = cache.prices_path("SPY").unwrap();
    assert!(!path.exists(), "No file should exist for empty download");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_populates_total_rows_and_date_range() {
    let cache = common::temp_cache();
    let pipeline = Pipeline {
        providers: vec![Arc::new(MockPricesProvider::new(5))],
        cache: Arc::clone(&cache),
        symbols: vec!["SPY".to_string()],
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 1);
    let result = &results[0];

    // Pipeline::run() should populate total_rows after consumer writes
    assert_eq!(
        result.total_rows, 5,
        "total_rows should be populated from cache"
    );

    // date_range should be populated
    assert!(
        result.date_range.is_some(),
        "date_range should be populated from cache"
    );
    let (min_date, max_date) = result.date_range.unwrap();
    assert_eq!(min_date, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
    assert_eq!(max_date, NaiveDate::from_ymd_opt(2024, 1, 5).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_concurrency_limit_enforced() {
    let cache = common::temp_cache();

    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    // 8 symbols with concurrency=2
    let symbols: Vec<String> = (0..8).map(|i| format!("SYM{i}")).collect();

    let pipeline = Pipeline {
        providers: vec![Arc::new(MockConcurrencyTracker::new(
            Arc::clone(&active),
            Arc::clone(&max_seen),
        ))],
        cache: Arc::clone(&cache),
        symbols: symbols.clone(),
        params: DownloadParams::default(),
        concurrency: 2,
    };

    let results = pipeline.run().await.expect("Pipeline should succeed");

    assert_eq!(results.len(), 8);
    assert!(
        results.iter().all(|r| r.is_success()),
        "All downloads should succeed"
    );

    // The semaphore should have capped concurrent downloads at 2
    let observed_max = max_seen.load(Ordering::SeqCst);
    assert!(
        observed_max <= 2,
        "Expected at most 2 concurrent downloads, but observed {observed_max}"
    );

    // Sanity: at least 2 ran concurrently (we had 8 symbols with 50ms sleep)
    assert!(
        observed_max >= 2,
        "Expected at least 2 concurrent downloads, but observed {observed_max}"
    );

    // All 8 files should exist
    for sym in &symbols {
        let path = cache.prices_path(sym).unwrap();
        assert!(path.exists(), "File missing for {sym}");
    }
}
