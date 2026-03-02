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

    #[test]
    fn test_anyvalue_to_naive_date_basic() {
        // 2024-01-01 is approximately 738886 days since year 1 CE
        let date = anyvalue_to_naive_date(&AnyValue::Date(738886 - EXCEL_DATE_EPOCH_OFFSET));
        assert!(date.is_some());
    }
}
