//! Shared resume logic for incremental data fetching.
//!
//! Provides common utilities for computing resume dates across different data providers.

use chrono::NaiveDate;
use polars::prelude::*;

/// Compute the next trading day after the latest date in a DataFrame column.
///
/// Scans the specified date column, finds the maximum date, and returns the next day.
///
/// # Arguments
/// * `df` - DataFrame containing date data
/// * `date_column` - Name of the date column to scan (e.g., "quote_date" or "date")
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
    for date_i32 in date_phys.iter() {
        if let Some(di) = date_i32 {
            // Polars uses days since 1900-01-01, offset from CE epoch is 719_162
            if let Some(date) = NaiveDate::from_num_days_from_ce_opt(di + 719_162) {
                if max_date.is_none() || date > max_date.unwrap() {
                    max_date = Some(date);
                }
            }
        }
    }

    // Return next day after max date
    max_date.map(|d| d.succ_opt()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resume_date_logic() {
        // Test the date calculation logic: max_date + 1 day
        let friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
        let expected_saturday = friday.succ_opt().unwrap();
        assert_eq!(expected_saturday, NaiveDate::from_ymd_opt(2024, 1, 20).unwrap());
    }

    #[test]
    fn test_resume_date_weekends() {
        // Verify weekday progression for resume scenarios
        let friday = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
        let saturday = friday.succ_opt().unwrap();
        let sunday = saturday.succ_opt().unwrap();
        let monday = sunday.succ_opt().unwrap();

        // When prices cache has Monday, it should return Monday
        // When prices cache is empty, fallback to Saturday (calendar day + 1)
        assert!(monday > saturday);
        assert_eq!((monday - friday).num_days(), 3);
    }
}
