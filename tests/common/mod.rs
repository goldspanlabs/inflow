#![allow(dead_code)]
//! Shared test helpers for integration tests.

use chrono::{Datelike, NaiveDate};
use inflow::cache::CacheStore;
use inflow::utils::EXCEL_DATE_EPOCH_OFFSET;
use polars::prelude::*;
use std::sync::Arc;

/// Create a temporary cache store backed by a temp directory.
///
/// The temp directory is kept (not auto-deleted) so that tests can inspect files.
pub fn temp_cache() -> Arc<CacheStore> {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    Arc::new(CacheStore::new(temp_dir.keep()))
}

/// Build a Polars Date-typed Series from a slice of `NaiveDate` values.
///
/// Converts each date to the Polars internal i32 representation (days since Unix epoch)
/// and casts to `DataType::Date`.
pub fn make_date_series(name: &str, dates: &[NaiveDate]) -> Series {
    let days: Vec<i32> = dates
        .iter()
        .map(|d| d.num_days_from_ce() - EXCEL_DATE_EPOCH_OFFSET)
        .collect();
    Series::new(PlSmallStr::from(name), &days)
        .cast(&DataType::Date)
        .expect("Failed to cast to Date")
}

/// Create a simple prices DataFrame with OHLCV columns and Date-typed date column.
///
/// Generates `n` rows starting from 2024-01-01 with sequential dates.
pub fn create_prices_df(n: usize) -> DataFrame {
    let dates: Vec<NaiveDate> = (0..n)
        .map(|i| {
            NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .checked_add_days(chrono::Days::new(i as u64))
                .unwrap()
        })
        .collect();
    let date_series = make_date_series("date", &dates);

    let open: Vec<f64> = (0..n).map(|i| 100.0 + i as f64).collect();
    let high: Vec<f64> = (0..n).map(|i| 105.0 + i as f64).collect();
    let low: Vec<f64> = (0..n).map(|i| 95.0 + i as f64).collect();
    let close: Vec<f64> = (0..n).map(|i| 102.0 + i as f64).collect();
    let volume: Vec<u64> = vec![1_000_000; n];

    let columns = vec![
        date_series.into_column(),
        Series::new(PlSmallStr::from("open"), &open).into_column(),
        Series::new(PlSmallStr::from("high"), &high).into_column(),
        Series::new(PlSmallStr::from("low"), &low).into_column(),
        Series::new(PlSmallStr::from("close"), &close).into_column(),
        Series::new(PlSmallStr::from("volume"), &volume).into_column(),
    ];
    DataFrame::new(n, columns).expect("Failed to create prices DataFrame")
}

/// Create an options DataFrame with dedup-relevant columns.
///
/// Generates rows with the given `dates` and `strikes` (must be same length).
/// Optionally includes a `symbol` column when `symbol` is `Some`.
pub fn create_options_df(dates: &[&str], strikes: &[f64], symbol: Option<&str>) -> DataFrame {
    assert_eq!(
        dates.len(),
        strikes.len(),
        "dates and strikes must have same length"
    );

    let height = dates.len();
    let expiration = vec!["2024-02-01"; height];
    let option_type = vec!["C"; height];
    let expiration_type = vec!["standard"; height];

    let mut columns = vec![
        Series::new(PlSmallStr::from("quote_date"), dates.to_vec()).into_column(),
        Series::new(PlSmallStr::from("expiration"), expiration).into_column(),
        Series::new(PlSmallStr::from("strike"), strikes.to_vec()).into_column(),
        Series::new(PlSmallStr::from("option_type"), option_type).into_column(),
        Series::new(PlSmallStr::from("expiration_type"), expiration_type).into_column(),
    ];

    if let Some(sym) = symbol {
        let symbols: Vec<&str> = vec![sym; height];
        columns.push(Series::new(PlSmallStr::from("symbol"), symbols).into_column());
    }

    DataFrame::new(height, columns).expect("Failed to create options DataFrame")
}
