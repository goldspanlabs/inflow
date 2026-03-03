//! Shared resume logic for incremental data fetching.
//!
//! Provides common utilities for computing resume dates across different data providers.

use chrono::NaiveDate;
use polars::prelude::*;

use super::date::EXCEL_DATE_EPOCH_OFFSET;

/// Compute the next trading day after the latest date in a `DataFrame` column.
///
/// Scans the specified date column, finds the maximum date, and returns the next day.
///
/// # Arguments
/// * `df` - `DataFrame` containing date data
/// * `date_column` - Name of the date column to scan (e.g., `quote_date` or `date`)
///
/// # Returns
/// * `Some(date)` - Next day after the maximum date found
/// * `None` - If no valid date data exists
///
/// # Example
/// ```ignore
/// let max_date = compute_resume_date(df, "quote_date")?;
/// // max_date is one day after the latest cached date
/// ```
pub fn compute_resume_date(df: &DataFrame, date_column: &str) -> Option<NaiveDate> {
    let date_col = df.column(date_column).ok()?;
    let date_chunked = date_col.date().ok()?;
    let date_phys = &date_chunked.phys;

    // Find max date by scanning physical representation
    let mut max_date: Option<NaiveDate> = None;
    for di in date_phys.iter().flatten() {
        if let Some(date) = NaiveDate::from_num_days_from_ce_opt(di + EXCEL_DATE_EPOCH_OFFSET) {
            if max_date.is_none() || date > max_date.unwrap() {
                max_date = Some(date);
            }
        }
    }

    // Return next day after max date
    max_date.and_then(|d| d.succ_opt())
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

        let result = compute_resume_date(&df, "quote_date");
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()));
    }

    #[test]
    fn test_compute_resume_date_empty() {
        let col = date_series("quote_date", &[]).into_column();
        let df = DataFrame::new(0, vec![col]).unwrap();
        assert_eq!(compute_resume_date(&df, "quote_date"), None);
    }

    #[test]
    fn test_compute_resume_date_missing_column() {
        let col = Series::new(PlSmallStr::from("other"), &[1i32, 2, 3]).into_column();
        let df = DataFrame::new(3, vec![col]).unwrap();
        assert_eq!(compute_resume_date(&df, "quote_date"), None);
    }
}
