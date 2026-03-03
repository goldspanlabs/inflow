//! Integration tests for cache file scanning.

mod common;

use chrono::NaiveDate;
use inflow::cache::scan::scan_file;
use inflow::utils::OPTIONS_DATE_COLUMN;
use polars::prelude::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_scan_file_basic() {
    let cache = common::temp_cache();

    // Build a small DataFrame with a Date column and write it
    let dates = vec![
        NaiveDate::from_ymd_opt(2024, 1, 10).unwrap(),
        NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
        NaiveDate::from_ymd_opt(2024, 1, 20).unwrap(),
    ];
    let date_col = common::make_date_series(OPTIONS_DATE_COLUMN, &dates).into_column();
    let val_col = Series::new(PlSmallStr::from("strike"), &[100.0, 105.0, 110.0]).into_column();
    let mut df = DataFrame::new(3, vec![date_col, val_col]).unwrap();

    let path = cache.options_path("TEST").unwrap();
    cache.atomic_write(&path, &mut df).await.unwrap();

    let info = scan_file(&path, OPTIONS_DATE_COLUMN).await.unwrap();

    assert_eq!(info.row_count, 3);
    assert!(info.size_bytes > 0);
    assert_eq!(info.date_min, Some(dates[0]));
    assert_eq!(info.date_max, Some(dates[2]));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scan_file_column_fallback() {
    let cache = common::temp_cache();

    // Write a Parquet with "date" column, but request "quote_date" — should fall back to "date"
    let dates = vec![
        NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
        NaiveDate::from_ymd_opt(2024, 3, 5).unwrap(),
    ];
    let date_col = common::make_date_series("date", &dates).into_column();
    let val_col = Series::new(PlSmallStr::from("close"), &[100.0, 105.0]).into_column();
    let mut df = DataFrame::new(2, vec![date_col, val_col]).unwrap();

    let path = cache.prices_path("FALLBACK").unwrap();
    cache.atomic_write(&path, &mut df).await.unwrap();

    let info = scan_file(&path, "quote_date").await.unwrap();

    assert_eq!(info.row_count, 2);
    // Should fall back to "date" column and still find dates
    assert_eq!(info.date_min, Some(dates[0]));
    assert_eq!(info.date_max, Some(dates[1]));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scan_file_missing_file() {
    let result = scan_file(
        std::path::Path::new("/tmp/nonexistent_inflow_test.parquet"),
        "date",
    )
    .await;
    assert!(result.is_err());
}
