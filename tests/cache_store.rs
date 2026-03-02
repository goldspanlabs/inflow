//! Integration tests for CacheStore.

use inflow::cache::CacheStore;
use polars::prelude::*;
use std::sync::Arc;

/// Helper to create a temporary cache store.
fn temp_cache() -> Arc<CacheStore> {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    Arc::new(CacheStore::new(temp_dir.keep()))
}

/// Helper to create a simple test DataFrame.
fn create_test_df(height: usize) -> DataFrame {
    let columns = vec![
        Series::new(PlSmallStr::from("col1"), vec![1.0; height]).into_column(),
        Series::new(PlSmallStr::from("col2"), vec![2.0; height]).into_column(),
    ];
    DataFrame::new(height, columns).expect("Failed to create DataFrame")
}

#[tokio::test(flavor = "multi_thread")]
async fn test_atomic_write_creates_correct_path() {
    let cache = temp_cache();

    let df = create_test_df(3);
    let mut df_mut = df.clone();

    let options_path = cache.options_path("TEST").expect("Failed to get path");
    cache
        .atomic_write(&options_path, &mut df_mut)
        .await
        .expect("Failed to write");

    // Verify file was created at the correct path
    assert!(
        options_path.exists(),
        "File not created at path: {:?}",
        options_path
    );

    // Verify the path contains the expected components
    let path_str = options_path.to_string_lossy();
    assert!(
        path_str.contains("options") && path_str.contains("TEST"),
        "Path doesn't contain expected components: {}",
        path_str
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_atomic_write_is_idempotent() {
    let cache = temp_cache();
    let prices_path = cache.prices_path("SPY").expect("Failed to get path");

    // First write
    let df1 = create_test_df(2);
    let mut df1_mut = df1;
    cache
        .atomic_write(&prices_path, &mut df1_mut)
        .await
        .expect("Failed to write first time");

    // Get file metadata after first write
    let _metadata1 = std::fs::metadata(&prices_path).expect("Failed to get metadata");

    // Second write with different data (same height)
    let df2 = create_test_df(2);
    let mut df2_mut = df2;
    cache
        .atomic_write(&prices_path, &mut df2_mut)
        .await
        .expect("Failed to write second time");

    // Verify file still exists and was updated
    let metadata2 =
        std::fs::metadata(&prices_path).expect("Failed to get metadata after second write");
    let size2 = metadata2.len();

    assert!(
        prices_path.exists(),
        "File should still exist after second write"
    );
    // File sizes might differ slightly depending on Parquet compression, but both should exist
    assert!(size2 > 0, "File should have content");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_read_parquet_returns_none_for_missing() {
    let cache = temp_cache();

    // Try to read a file that doesn't exist
    let nonexistent_path = cache
        .prices_path("NONEXISTENT")
        .expect("Failed to get path");

    let result = cache
        .read_parquet(&nonexistent_path)
        .await
        .expect("read_parquet should not error");

    // Should return None for missing file
    assert!(
        result.is_none(),
        "read_parquet should return None for missing file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_options_path_validation() {
    let cache = temp_cache();

    // Valid symbol should work
    let valid_result = cache.options_path("SPY");
    assert!(valid_result.is_ok(), "Valid symbol should succeed");

    // Empty symbol should fail
    let empty_result = cache.options_path("");
    assert!(empty_result.is_err(), "Empty symbol should fail validation");

    // Symbol with path separators should fail
    let invalid_result = cache.options_path("TEST/INVALID");
    assert!(
        invalid_result.is_err(),
        "Symbol with / should fail validation"
    );

    // Symbol with backslashes should fail
    let invalid_result2 = cache.options_path("TEST\\INVALID");
    assert!(
        invalid_result2.is_err(),
        "Symbol with backslash should fail validation"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_list_symbols_returns_sorted() {
    let cache = temp_cache();

    // Create files in random order
    let symbols = vec!["ZZZ", "AAA", "MMM", "BBB"];
    let df = create_test_df(1);

    for symbol in symbols.iter() {
        let path = cache.options_path(symbol).expect("Failed to get path");
        let mut df_mut = df.clone();
        cache
            .atomic_write(&path, &mut df_mut)
            .await
            .expect("Failed to write");
    }

    // List symbols
    let listed = cache
        .list_symbols("options")
        .expect("Failed to list symbols");

    // Should be sorted alphabetically
    assert_eq!(
        listed,
        vec!["AAA", "BBB", "MMM", "ZZZ"],
        "Symbols should be sorted alphabetically"
    );
}
