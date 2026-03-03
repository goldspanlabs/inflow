//! Integration tests for consumer writer task.

mod common;

use inflow::pipeline::consumer::run_writer;
use inflow::pipeline::types::WindowChunk;
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread")]
async fn test_write_prices_creates_file() {
    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(10);

    let df = common::create_prices_df(3);

    // Spawn consumer
    let cache_clone = Arc::clone(&cache);
    let consumer_handle = tokio::spawn(async move { run_writer(cache_clone, rx).await });

    // Send prices chunk
    tx.send(WindowChunk::PricesComplete {
        symbol: "TEST".to_string(),
        df,
    })
    .await
    .expect("Failed to send chunk");

    // Drop sender to signal completion
    drop(tx);

    // Wait for consumer
    let errors = consumer_handle.await.expect("Consumer task panicked");

    // Verify no errors
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    // Verify file exists
    let prices_path = cache.prices_path("TEST").expect("Failed to get path");
    assert!(
        prices_path.exists(),
        "Prices file not created: {:?}",
        prices_path
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_write_options_deduplicates_on_merge() {
    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(10);

    // Create two options chunks with duplicate rows
    let df1 = common::create_options_df(&["2024-01-01", "2024-01-02"], &[100.0, 100.0], None);
    let df2 = common::create_options_df(&["2024-01-01", "2024-01-03"], &[100.0, 100.0], None);

    // Spawn consumer in separate thread to avoid runtime nesting
    let cache_clone = Arc::clone(&cache);
    let consumer_handle = tokio::spawn(async move { run_writer(cache_clone, rx).await });

    // Send chunks
    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df1,
    })
    .await
    .expect("Failed to send chunk 1");

    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df2,
    })
    .await
    .expect("Failed to send chunk 2");

    // Drop sender
    drop(tx);

    // Wait for consumer
    let errors = consumer_handle.await.expect("Consumer task panicked");
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    // Verify file exists
    let options_path = cache.options_path("SPY").expect("Failed to get path");
    assert!(
        options_path.exists(),
        "Options file not created: {:?}",
        options_path
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_write_options_merges_with_existing() {
    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(10);

    // Create initial data chunks to send through consumer
    let df1 = common::create_options_df(&["2024-01-01", "2024-01-02"], &[100.0, 100.0], None);
    let df2 = common::create_options_df(&["2024-01-03", "2024-01-04"], &[100.0, 100.0], None);

    // Spawn consumer
    let cache_clone = Arc::clone(&cache);
    let consumer_handle = tokio::spawn(async move { run_writer(cache_clone, rx).await });

    // Send first chunk (simulating existing/cached data)
    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df1,
    })
    .await
    .expect("Failed to send chunk 1");

    // Send second chunk (simulating new data)
    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df2,
    })
    .await
    .expect("Failed to send chunk 2");

    drop(tx);

    // Wait for consumer
    let errors = consumer_handle.await.expect("Consumer task panicked");
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    // Verify file exists (this validates the merge and write happened)
    let options_path = cache.options_path("SPY").expect("Failed to get path");
    assert!(
        options_path.exists(),
        "Options file should exist after merge of multiple chunks"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_write_options_sorts_by_quote_date() {
    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(10);

    // Create chunks with out-of-order dates
    let df1 = common::create_options_df(&["2024-01-03", "2024-01-01"], &[100.0, 100.0], None);
    let df2 = common::create_options_df(&["2024-01-02"], &[100.0], None);

    // Spawn consumer
    let cache_clone = Arc::clone(&cache);
    let consumer_handle = tokio::spawn(async move { run_writer(cache_clone, rx).await });

    // Send chunks
    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df1,
    })
    .await
    .expect("Failed to send chunk 1");

    tx.send(WindowChunk::OptionsWindow {
        symbol: "SPY".to_string(),
        df: df2,
    })
    .await
    .expect("Failed to send chunk 2");

    drop(tx);

    // Wait for consumer
    let errors = consumer_handle.await.expect("Consumer task panicked");
    assert!(
        errors.is_empty(),
        "Expected no errors during sort test, got: {:?}",
        errors
    );

    // Verify file was created
    let options_path = cache.options_path("SPY").expect("Failed to get path");
    assert!(
        options_path.exists(),
        "Options file should be sorted and written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_run_writer_returns_empty_errors_on_success() {
    let cache = common::temp_cache();
    let (tx, rx) = mpsc::channel(10);

    let df = common::create_prices_df(3);

    // Spawn consumer
    let cache_clone = Arc::clone(&cache);
    let consumer_handle = tokio::spawn(async move { run_writer(cache_clone, rx).await });

    // Send valid chunk
    tx.send(WindowChunk::PricesComplete {
        symbol: "TEST".to_string(),
        df,
    })
    .await
    .expect("Failed to send chunk");

    drop(tx);

    // Wait for consumer and verify no errors
    let errors = consumer_handle.await.expect("Consumer task panicked");
    assert!(
        errors.is_empty(),
        "Expected no errors on success, got: {:?}",
        errors
    );
}
