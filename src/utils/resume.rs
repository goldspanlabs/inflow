//! Shared resume logic for incremental data fetching.
//!
//! Provides common utilities for computing resume dates across different data providers.

use chrono::NaiveDate;
use polars::prelude::*;

use super::date::EXCEL_DATE_EPOCH_OFFSET;

/// Compute the next trading day after the latest date in a `DataFrame` column.
///
/// Scans the specified date column, finds the maximum date, and returns the next day.
/// Supports optional row filtering and prices-cache-aware trading day detection.
///
/// # Arguments
/// * `df` - `DataFrame` containing date data
/// * `date_column` - Name of the date column to scan (e.g., `quote_date` or `date`)
/// * `filter` - Optional `(column_name, value)` pair to filter rows before scanning
///   (e.g., `Some(("option_type", "call"))` to only consider call options)
/// * `prices_df` - Optional prices `DataFrame` to find the next actual trading day,
///   skipping weekends and holidays
///
/// # Returns
/// * `Some(date)` - Next day (or next trading day) after the maximum date found
/// * `None` - If no valid date data exists
pub fn compute_resume_date(
    df: &DataFrame,
    date_column: &str,
    filter: Option<(&str, &str)>,
    prices_df: Option<&DataFrame>,
) -> Option<NaiveDate> {
    let date_col = df.column(date_column).ok()?;
    let date_chunked = date_col.date().ok()?;
    let date_phys = &date_chunked.phys;

    // Find max date, optionally filtered by a column value
    let max_date = if let Some((filter_col, filter_val)) = filter {
        find_max_date_filtered(df, date_phys, filter_col, filter_val)
    } else {
        find_max_date(date_phys)
    };

    let max_date = max_date?;
    let candidate = max_date.succ_opt()?;

    // If prices cache available, find next trading day (skip weekends/holidays)
    if let Some(next_trading) = find_next_trading_day(prices_df, candidate) {
        return Some(next_trading);
    }

    Some(candidate)
}

/// Find the maximum date from physical date values (no filtering).
fn find_max_date(date_phys: &ChunkedArray<Int32Type>) -> Option<NaiveDate> {
    let mut max_date: Option<NaiveDate> = None;
    for di in date_phys.iter().flatten() {
        if let Some(date) = NaiveDate::from_num_days_from_ce_opt(di + EXCEL_DATE_EPOCH_OFFSET) {
            if max_date.is_none() || date > max_date.unwrap() {
                max_date = Some(date);
            }
        }
    }
    max_date
}

/// Find the maximum date from physical date values, filtered by a string column.
///
/// Compares only the first character (lowercased) of each value against the filter,
/// matching EODHD's `option_type` format ("c"/"call" for calls, "p"/"put" for puts).
fn find_max_date_filtered(
    df: &DataFrame,
    date_phys: &ChunkedArray<Int32Type>,
    filter_col: &str,
    filter_val: &str,
) -> Option<NaiveDate> {
    let col = df.column(filter_col).ok()?;
    let col_str = col.str().ok()?;
    let filter_char = filter_val.chars().next()?.to_lowercase().to_string();

    let mut max_date: Option<NaiveDate> = None;

    for (opt_val, date_val) in col_str.iter().zip(date_phys.iter()) {
        if let (Some(val), Some(di)) = (opt_val, date_val) {
            let val_char = val.chars().next().map(|c| c.to_lowercase().to_string());
            if val_char.as_deref() == Some(&filter_char) {
                if let Some(date) =
                    NaiveDate::from_num_days_from_ce_opt(di + EXCEL_DATE_EPOCH_OFFSET)
                {
                    if max_date.is_none() || date > max_date.unwrap() {
                        max_date = Some(date);
                    }
                }
            }
        }
    }

    max_date
}

/// Find the first trading day >= `candidate` in a prices `DataFrame`.
///
/// Returns `None` if no prices data is available or no future trading day exists.
fn find_next_trading_day(prices_df: Option<&DataFrame>, candidate: NaiveDate) -> Option<NaiveDate> {
    let prices = prices_df?;
    let price_date_col = prices.column("date").ok()?;
    let price_dates = price_date_col.date().ok()?;
    let price_dates_phys = &price_dates.phys;

    for d_i32 in price_dates_phys.iter().flatten() {
        if let Some(trading_date) =
            NaiveDate::from_num_days_from_ce_opt(d_i32 + EXCEL_DATE_EPOCH_OFFSET)
        {
            if trading_date >= candidate {
                return Some(trading_date);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn date_series(name: &str, dates: &[NaiveDate]) -> Series {
        let days: Vec<i32> = dates
            .iter()
            .map(|d| d.num_days_from_ce() - EXCEL_DATE_EPOCH_OFFSET)
            .collect();
        Series::new(PlSmallStr::from(name), &days)
            .cast(&DataType::Date)
            .unwrap()
    }

    #[test]
    fn test_compute_resume_date_basic() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 17).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();

        let col = date_series("quote_date", &[d1, d2, d3]).into_column();
        let df = DataFrame::new(3, vec![col]).unwrap();

        let result = compute_resume_date(&df, "quote_date", None, None);
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()));
    }

    #[test]
    fn test_compute_resume_date_empty() {
        let col = date_series("quote_date", &[]).into_column();
        let df = DataFrame::new(0, vec![col]).unwrap();
        assert_eq!(compute_resume_date(&df, "quote_date", None, None), None);
    }

    #[test]
    fn test_compute_resume_date_missing_column() {
        let col = Series::new(PlSmallStr::from("other"), &[1i32, 2, 3]).into_column();
        let df = DataFrame::new(3, vec![col]).unwrap();
        assert_eq!(compute_resume_date(&df, "quote_date", None, None), None);
    }

    #[test]
    fn test_compute_resume_date_with_filter() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 17).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap(); // call max
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();

        let date_col = date_series("quote_date", &[d1, d2, d3]).into_column();
        let type_col =
            Series::new(PlSmallStr::from("option_type"), &["call", "call", "put"]).into_column();
        let df = DataFrame::new(3, vec![date_col, type_col]).unwrap();

        // Filter for calls: max is Jan 19 → resume Jan 20
        let result = compute_resume_date(&df, "quote_date", Some(("option_type", "call")), None);
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()));

        // Filter for puts: max is Jan 18 → resume Jan 19
        let result = compute_resume_date(&df, "quote_date", Some(("option_type", "put")), None);
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2024, 1, 19).unwrap()));
    }

    #[test]
    fn test_compute_resume_date_with_prices_cache() {
        // Options cached up to Friday Jan 19
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
        let date_col = date_series("quote_date", &[d1]).into_column();
        let df = DataFrame::new(1, vec![date_col]).unwrap();

        // Prices cache has trading days (no Sat/Sun)
        let fri = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
        let mon = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();
        let prices_col = date_series("date", &[fri, mon]).into_column();
        let prices_df = DataFrame::new(2, vec![prices_col]).unwrap();

        // Should skip Saturday and return Monday
        let result = compute_resume_date(&df, "quote_date", None, Some(&prices_df));
        assert_eq!(result, Some(mon));
    }
}
