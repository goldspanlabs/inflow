//! Integration tests for the Yahoo download pipeline using a mock `QuoteFetcher`.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use inflow::pipeline::types::{DownloadParams, WindowChunk};
use inflow::providers::yahoo::{QuoteFetcher, YahooProvider};
use inflow::providers::DataProvider;
use yahoo_finance_api as yahoo;

use indicatif::MultiProgress;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Mock fetcher that returns pre-configured quotes.
struct MockFetcher {
    quotes: Vec<yahoo::Quote>,
    /// Captures the period argument passed to `fetch_quotes`.
    captured_period: Arc<Mutex<Option<String>>>,
}

impl MockFetcher {
    fn new(quotes: Vec<yahoo::Quote>) -> Self {
        Self {
            quotes,
            captured_period: Arc::new(Mutex::new(None)),
        }
    }

    fn with_capture(quotes: Vec<yahoo::Quote>, captured: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            quotes,
            captured_period: captured,
        }
    }
}

#[async_trait]
impl QuoteFetcher for MockFetcher {
    async fn fetch_quotes(&self, _symbol: &str, period: &str) -> anyhow::Result<Vec<yahoo::Quote>> {
        *self.captured_period.lock().unwrap() = Some(period.to_string());
        Ok(self.quotes.clone())
    }
}

/// Build a simple test quote with the given timestamp.
fn make_quote(timestamp: i64, close: f64) -> yahoo::Quote {
    yahoo::Quote {
        timestamp,
        open: close - 1.0,
        high: close + 1.0,
        low: close - 2.0,
        close,
        adjclose: close,
        volume: 1_000_000,
    }
}

/// Collect all `WindowChunk`s from a receiver until it closes.
async fn collect_chunks(mut rx: mpsc::Receiver<WindowChunk>) -> Vec<WindowChunk> {
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        chunks.push(chunk);
    }
    chunks
}

#[tokio::test(flavor = "multi_thread")]
async fn test_yahoo_basic_download() {
    let quotes = vec![
        make_quote(1_709_510_400, 500.0), // 2024-03-04
        make_quote(1_709_596_800, 502.0), // 2024-03-05
        make_quote(1_709_683_200, 501.0), // 2024-03-06
        make_quote(1_709_769_600, 503.0), // 2024-03-07
        make_quote(1_709_856_000, 504.0), // 2024-03-08
    ];

    let fetcher = MockFetcher::new(quotes);
    let provider = YahooProvider::with_fetcher(Box::new(fetcher));

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();
    let params = DownloadParams::default();

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success());
    assert_eq!(result.new_rows, 5);

    // Should have exactly one PricesComplete chunk
    assert_eq!(chunks.len(), 1);
    if let WindowChunk::PricesComplete { symbol, df } = &chunks[0] {
        assert_eq!(symbol, "SPY");
        assert_eq!(df.height(), 5);

        let names: Vec<_> = df
            .schema()
            .iter_names()
            .map(polars::prelude::PlSmallStr::to_string)
            .collect();
        assert!(names.contains(&"date".to_string()));
        assert!(names.contains(&"open".to_string()));
        assert!(names.contains(&"close".to_string()));
        assert!(names.contains(&"volume".to_string()));
    } else {
        panic!("Expected PricesComplete chunk");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_yahoo_empty_quotes() {
    let fetcher = MockFetcher::new(vec![]);
    let provider = YahooProvider::with_fetcher(Box::new(fetcher));

    let cache = common::temp_cache();
    let (tx, _rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();
    let params = DownloadParams::default();

    let result = provider
        .download("SPY", &params, &cache, tx, shutdown, &mp)
        .await;

    assert!(result.is_err(), "empty quotes should return an error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("No data returned"),
        "error should mention no data: {err_msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_yahoo_resume_up_to_date() {
    // First, populate the cache with prices up to today
    let today = chrono::Utc::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);

    let quotes = vec![
        make_quote(
            yesterday
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp(),
            500.0,
        ),
        make_quote(
            today.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            502.0,
        ),
    ];

    let cache = common::temp_cache();
    let mp = MultiProgress::new();

    // First download to populate cache
    {
        let fetcher = MockFetcher::new(quotes.clone());
        let provider = YahooProvider::with_fetcher(Box::new(fetcher));
        let (tx, rx) = mpsc::channel(64);
        let shutdown = CancellationToken::new();
        let params = DownloadParams::default();

        let cache_clone = cache.clone();
        let result_handle = tokio::spawn(async move {
            provider
                .download("SPY", &params, &cache_clone, tx, shutdown, &mp)
                .await
        });

        let chunks = collect_chunks(rx).await;
        let result = result_handle.await.unwrap().unwrap();
        assert!(result.is_success());

        // Write the chunk to cache so resume logic can find it
        if let Some(WindowChunk::PricesComplete { symbol, mut df }) = chunks.into_iter().next() {
            let path = cache.prices_path(&symbol).unwrap();
            cache.atomic_write(&path, &mut df).await.unwrap();
        }
    }

    // Second download — should detect up-to-date cache and return 0 new rows
    {
        // The fetcher should NOT be called since cache is up to date
        let fetcher = MockFetcher::new(vec![]);
        let provider = YahooProvider::with_fetcher(Box::new(fetcher));
        let (tx, _rx) = mpsc::channel(64);
        let mp2 = MultiProgress::new();
        let shutdown = CancellationToken::new();
        let params = DownloadParams::default();

        let result = provider
            .download("SPY", &params, &cache, tx, shutdown, &mp2)
            .await
            .unwrap();

        assert!(result.is_success());
        assert_eq!(
            result.new_rows, 0,
            "should report 0 new rows when up to date"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_yahoo_gap_period_selection() {
    // Populate cache with data from ~60 days ago
    let today = chrono::Utc::now().date_naive();
    let old_date = today - chrono::Duration::days(60);

    let old_quotes = vec![make_quote(
        old_date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        480.0,
    )];

    let cache = common::temp_cache();

    // Write initial cache
    {
        let fetcher = MockFetcher::new(old_quotes);
        let provider = YahooProvider::with_fetcher(Box::new(fetcher));
        let (tx, rx) = mpsc::channel(64);
        let mp = MultiProgress::new();
        let shutdown = CancellationToken::new();
        let params = DownloadParams::default();

        let cache_clone = cache.clone();
        let handle = tokio::spawn(async move {
            provider
                .download("SPY", &params, &cache_clone, tx, shutdown, &mp)
                .await
        });

        let chunks = collect_chunks(rx).await;
        handle.await.unwrap().unwrap();

        if let Some(WindowChunk::PricesComplete { symbol, mut df }) = chunks.into_iter().next() {
            let path = cache.prices_path(&symbol).unwrap();
            cache.atomic_write(&path, &mut df).await.unwrap();
        }
    }

    // Now do a second fetch — with a 60-day gap, it should select "3mo"
    let captured = Arc::new(Mutex::new(None));
    let fresh_quotes = vec![make_quote(
        today.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        500.0,
    )];

    let fetcher = MockFetcher::with_capture(fresh_quotes, captured.clone());
    let provider = YahooProvider::with_fetcher(Box::new(fetcher));
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();
    let params = DownloadParams::default();

    let cache_clone = cache.clone();
    let handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache_clone, tx, shutdown, &mp)
            .await
    });

    let _chunks = collect_chunks(rx).await;
    handle.await.unwrap().unwrap();

    let period = captured.lock().unwrap().clone();
    assert_eq!(
        period,
        Some("3mo".to_string()),
        "60-day gap should select 3mo period, got {period:?}"
    );
}
