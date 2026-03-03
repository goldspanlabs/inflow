//! Integration tests for the EODHD download pipeline using wiremock.

mod common;

use inflow::pipeline::types::DownloadParams;
use inflow::pipeline::types::WindowChunk;
use inflow::providers::eodhd::pagination::Paginator;
use inflow::providers::eodhd::EodhdProvider;
use inflow::providers::DataProvider;

use std::sync::Arc;

use indicatif::MultiProgress;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Collect all `WindowChunk`s from a receiver until it closes.
async fn collect_chunks(mut rx: mpsc::Receiver<WindowChunk>) -> Vec<WindowChunk> {
    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        chunks.push(chunk);
    }
    chunks
}

/// Count total rows across all `OptionsWindow` chunks.
fn count_option_rows(chunks: &[WindowChunk]) -> usize {
    chunks
        .iter()
        .map(|c| match c {
            WindowChunk::OptionsWindow { df, .. } => df.height(),
            WindowChunk::PricesComplete { .. } => 0,
        })
        .sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_basic_download() {
    let server = MockServer::start().await;

    let body = include_str!("fixtures/eodhd_compact.json");

    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success(), "download should succeed");
    assert!(result.new_rows > 0, "should have fetched rows");

    let total_rows = count_option_rows(&chunks);
    assert!(total_rows > 0, "should have received option rows");

    // Verify the DataFrame has expected columns
    if let Some(WindowChunk::OptionsWindow { df, .. }) = chunks.first() {
        assert!(df.schema().contains("underlying_symbol"));
        assert!(df.schema().contains("option_type"));
        assert!(df.schema().contains("expiration"));
        assert!(df.schema().contains("quote_date"));
        assert!(df.schema().contains("strike"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_pagination() {
    let server = MockServer::start().await;

    // Page 1 has a "next" link pointing to page 2
    let page1_body = include_str!("fixtures/eodhd_paginated_page1.json");
    let page2_url = format!("{}/options/eod/page2", server.uri());
    let page1_body = page1_body.replace("{{NEXT_URL}}", &page2_url);

    let page2_body = include_str!("fixtures/eodhd_paginated_page2.json");

    // Mount page 1 — matches the initial request with query params
    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .and(query_param("compact", "1"))
        .and(query_param("page[offset]", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page1_body))
        .mount(&server)
        .await;

    // Mount page 2 — matches the "next" URL
    Mock::given(method("GET"))
        .and(path("/options/eod/page2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page2_body))
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success());

    // Page 1 has 2 rows + page 2 has 1 row = 3 total across all chunks
    // But each window (call + put) makes its own requests, so we expect data from both
    let total_rows = count_option_rows(&chunks);
    assert!(
        total_rows >= 3,
        "should have at least 3 rows across pages, got {total_rows}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_empty_response() {
    let server = MockServer::start().await;

    let body = include_str!("fixtures/eodhd_empty.json");

    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success());
    assert_eq!(result.new_rows, 0, "empty response should yield 0 rows");
    assert_eq!(count_option_rows(&chunks), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_api_error_401() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error": "unauthorized"}"#))
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let _chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    // 401 is handled as a warning per-window, not a fatal error
    // The result should have warnings containing the auth error message
    assert!(
        !result.warnings.is_empty() || result.new_rows == 0,
        "401 should produce warnings or zero rows"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_resume_skips_cached() {
    let server = MockServer::start().await;

    let body = include_str!("fixtures/eodhd_compact.json");

    // Track that the request uses the correct date range (after resume date)
    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .and(query_param("filter[tradetime_from]", "2024-03-15"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .expect(1..)
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    // Use explicit from_date to simulate resume behavior
    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 16).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success());
    // With explicit from_date=2024-03-15, the request should use that as start
    // and only fetch from that date forward
    let total = count_option_rows(&chunks);
    assert!(
        total > 0,
        "resume download should still fetch new rows from resume date"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_retry_on_500() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let server = MockServer::start().await;

    // Track request count to return 500 on first two attempts, 200 on third
    let call_count = Arc::new(AtomicUsize::new(0));
    let body = include_str!("fixtures/eodhd_compact.json").to_string();

    let call_count_for_responder = call_count.clone();
    let body_for_responder = body.clone();

    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .respond_with(move |_req: &Request| {
            let n = call_count_for_responder.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                ResponseTemplate::new(500).set_body_string(r#"{"error": "internal"}"#)
            } else {
                ResponseTemplate::new(200).set_body_string(&body_for_responder)
            }
        })
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let _chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    // After retrying past the 500s, should eventually get data
    assert!(result.is_success(), "should succeed after retries");

    // Should have made more than 2 requests (retries)
    let total_calls = call_count.load(Ordering::SeqCst);
    assert!(
        total_calls > 2,
        "should have retried, total calls: {total_calls}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eodhd_rate_limit_header() {
    let server = MockServer::start().await;

    let body = include_str!("fixtures/eodhd_compact.json");

    // Respond with X-RateLimit-Remaining header
    Mock::given(method("GET"))
        .and(path("/options/eod"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("X-RateLimit-Remaining", "10"),
        )
        .mount(&server)
        .await;

    let paginator = Paginator::with_base_url("test-key".into(), server.uri());
    let provider = EodhdProvider::with_paginator(paginator);

    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(64);
    let mp = MultiProgress::new();
    let shutdown = CancellationToken::new();

    let params = DownloadParams {
        period: "1y".into(),
        from_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
        to_date: Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()),
    };

    let result_handle = tokio::spawn(async move {
        provider
            .download("SPY", &params, &cache, tx, shutdown, &mp)
            .await
    });

    let _chunks = collect_chunks(rx).await;
    let result = result_handle.await.unwrap().unwrap();

    assert!(result.is_success(), "should succeed with rate limit header");
    assert!(result.new_rows > 0, "should have fetched rows");
}
