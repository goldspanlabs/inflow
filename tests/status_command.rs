//! Integration tests for the status command.

mod common;

use chrono::NaiveDate;
use inflow::commands::status;
use polars::prelude::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_status_empty_cache() {
    let cache = common::temp_cache();
    let result = status::execute(&cache).await;
    assert!(result.is_ok(), "empty cache should not panic");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_status_with_prices() {
    let cache = common::temp_cache();

    // Write a small prices Parquet file
    let dates = common::make_date_series(
        "date",
        &[
            NaiveDate::from_ymd_opt(2024, 3, 14).unwrap(),
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
        ],
    );
    let close = Series::new(PlSmallStr::from("close"), &[500.0_f64, 502.0]);
    let mut df = DataFrame::new(2, vec![dates.into_column(), close.into_column()]).unwrap();

    let path = cache.prices_path("SPY").unwrap();
    cache.atomic_write(&path, &mut df).await.unwrap();

    let result = status::execute(&cache).await;
    assert!(result.is_ok(), "prices cache should display without error");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_status_with_options_and_prices() {
    let cache = common::temp_cache();

    // Write prices
    let dates = common::make_date_series("date", &[NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()]);
    let close = Series::new(PlSmallStr::from("close"), &[500.0_f64]);
    let mut prices_df = DataFrame::new(1, vec![dates.into_column(), close.into_column()]).unwrap();
    let prices_path = cache.prices_path("SPY").unwrap();
    cache
        .atomic_write(&prices_path, &mut prices_df)
        .await
        .unwrap();

    // Write options
    let quote_dates = common::make_date_series(
        "quote_date",
        &[NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()],
    );
    let symbol = Series::new(PlSmallStr::from("underlying_symbol"), &["SPY"]);
    let option_type = Series::new(PlSmallStr::from("option_type"), &["call"]);
    let strike = Series::new(PlSmallStr::from("strike"), &[500.0_f64]);
    let mut options_df = DataFrame::new(
        1,
        vec![
            quote_dates.into_column(),
            symbol.into_column(),
            option_type.into_column(),
            strike.into_column(),
        ],
    )
    .unwrap();
    let options_path = cache.options_path("SPY").unwrap();
    cache
        .atomic_write(&options_path, &mut options_df)
        .await
        .unwrap();

    let result = status::execute(&cache).await;
    assert!(
        result.is_ok(),
        "both options+prices should display without error"
    );
}
