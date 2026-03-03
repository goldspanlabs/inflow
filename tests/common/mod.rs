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
