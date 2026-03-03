//! Date conversion utilities for Polars scalars and values.

use chrono::NaiveDate;
use polars::prelude::{AnyValue, Scalar};

/// Epoch offset: number of days from year 1 CE to Excel date epoch (1900-01-01)
pub const EXCEL_DATE_EPOCH_OFFSET: i32 = 719_163;

/// Convert a Polars date scalar to `NaiveDate`.
///
/// Handles the conversion from Polars' internal date representation
/// (days since year 1 CE) to `chrono::NaiveDate`.
pub fn scalar_to_naive_date(scalar: &Scalar) -> Option<NaiveDate> {
    anyvalue_to_naive_date(scalar.value())
}

/// Convert a Polars `AnyValue` to `NaiveDate`.
///
/// Handles the conversion from Polars' internal date representation
/// (days since year 1 CE) to `chrono::NaiveDate`.
pub fn anyvalue_to_naive_date(val: &AnyValue) -> Option<NaiveDate> {
    match val {
        AnyValue::Date(days) => {
            NaiveDate::from_num_days_from_ce_opt(days + EXCEL_DATE_EPOCH_OFFSET)
        }
        _ => None,
    }
}

/// Extract the min and max date from a `DataFrame` column.
///
/// Returns `Some((min_date, max_date))` if the column exists and contains valid dates,
/// or `None` if the column doesn't exist or contains no valid dates.
pub fn extract_date_range(
    df: &polars::prelude::DataFrame,
    col_name: &str,
) -> Option<(NaiveDate, NaiveDate)> {
    let col = df.column(col_name).ok()?;

    let min = col
        .min_reduce()
        .ok()
        .and_then(|s| scalar_to_naive_date(&s))?;

    let max = col
        .max_reduce()
        .ok()
        .and_then(|s| scalar_to_naive_date(&s))?;

    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use polars::prelude::*;

    #[test]
    fn test_epoch_offset_is_unix_epoch() {
        // EXCEL_DATE_EPOCH_OFFSET must equal num_days_from_ce for 1970-01-01.
        // If this fails, all date conversions in the codebase are off by 1 day.
        let unix_epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        assert_eq!(EXCEL_DATE_EPOCH_OFFSET, unix_epoch.num_days_from_ce());
    }

    #[test]
    fn test_anyvalue_to_naive_date_known_date() {
        // 2024-01-19 → known i32 in Polars Date representation
        let expected = NaiveDate::from_ymd_opt(2024, 1, 19).unwrap();
        let polars_i32 = expected.num_days_from_ce() - EXCEL_DATE_EPOCH_OFFSET;
        let result = anyvalue_to_naive_date(&AnyValue::Date(polars_i32));
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_anyvalue_to_naive_date_non_date() {
        // Non-Date AnyValue variants should return None
        assert_eq!(anyvalue_to_naive_date(&AnyValue::Int32(42)), None);
        assert_eq!(
            anyvalue_to_naive_date(&AnyValue::String("2024-01-19")),
            None
        );
    }

    #[test]
    fn test_scalar_to_naive_date() {
        let expected = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let polars_i32 = expected.num_days_from_ce() - EXCEL_DATE_EPOCH_OFFSET;
        let scalar = Scalar::new(DataType::Date, AnyValue::Date(polars_i32));
        assert_eq!(scalar_to_naive_date(&scalar), Some(expected));
    }

    #[test]
    fn test_extract_date_range_basic() {
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let d3 = NaiveDate::from_ymd_opt(2024, 1, 20).unwrap();

        let days: Vec<i32> = [d1, d2, d3]
            .iter()
            .map(|d| d.num_days_from_ce() - EXCEL_DATE_EPOCH_OFFSET)
            .collect();
        let date_col = Series::new(PlSmallStr::from("quote_date"), &days)
            .cast(&DataType::Date)
            .unwrap()
            .into_column();
        let df = DataFrame::new(3, vec![date_col]).unwrap();

        let (min, max) = extract_date_range(&df, "quote_date").unwrap();
        assert_eq!(min, d1);
        assert_eq!(max, d3);
    }

    #[test]
    fn test_extract_date_range_missing_column() {
        let col = Series::new(PlSmallStr::from("x"), &[1i32, 2, 3]).into_column();
        let df = DataFrame::new(3, vec![col]).unwrap();
        assert!(extract_date_range(&df, "quote_date").is_none());
    }
}
