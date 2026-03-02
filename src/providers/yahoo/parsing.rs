//! `DataFrame` construction from Yahoo Finance quotes.

use anyhow::Result;
use chrono::NaiveDate;
use polars::prelude::*;
use yahoo_finance_api as yahoo;

/// Build a polars `DataFrame` from Yahoo Finance quotes.
///
/// Constructs a `DataFrame` with columns: date, open, high, low, close, adjclose, volume.
/// Skips quotes with invalid timestamps, logging a warning for each.
pub fn build_dataframe_from_quotes(quotes: &[yahoo::Quote], symbol: &str) -> Result<DataFrame> {
    let mut dates: Vec<NaiveDate> = Vec::with_capacity(quotes.len());
    let mut open: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut high: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut low: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut close: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut adjclose: Vec<f64> = Vec::with_capacity(quotes.len());
    let mut volume: Vec<u64> = Vec::with_capacity(quotes.len());

    for q in quotes {
        let Some(dt) = chrono::DateTime::from_timestamp(q.timestamp, 0) else {
            tracing::warn!(
                timestamp = q.timestamp,
                "Skipping quote with invalid timestamp"
            );
            continue;
        };
        dates.push(dt.naive_utc().date());
        open.push(q.open);
        high.push(q.high);
        low.push(q.low);
        close.push(q.close);
        adjclose.push(q.adjclose);
        volume.push(q.volume);
    }

    if dates.is_empty() {
        anyhow::bail!("All quotes for {symbol} had invalid timestamps");
    }

    let df = df! {
        "open" => &open,
        "high" => &high,
        "low" => &low,
        "close" => &close,
        "adjclose" => &adjclose,
        "volume" => &volume,
    }?;

    // Add date column
    let date_series =
        DateChunked::from_naive_date(PlSmallStr::from("date"), dates.iter().copied()).into_column();

    let mut df = df.hstack(&[date_series])?;

    // Reorder so date is first
    df = df.select(["date", "open", "high", "low", "close", "adjclose", "volume"])?;

    Ok(df)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_dataframe_basic() {
        let quotes = vec![
            yahoo::Quote {
                timestamp: 1_609_459_200, // 2021-01-01
                open: 100.0,
                high: 105.0,
                low: 99.0,
                close: 103.0,
                adjclose: 103.0,
                volume: 1_000_000,
            },
            yahoo::Quote {
                timestamp: 1_609_545_600, // 2021-01-02
                open: 104.0,
                high: 106.0,
                low: 101.0,
                close: 102.0,
                adjclose: 102.0,
                volume: 900_000,
            },
        ];

        let df = build_dataframe_from_quotes(&quotes, "TEST").expect("should build dataframe");

        assert_eq!(df.height(), 2);
        let names: Vec<_> = df.schema().iter_names().collect();
        assert_eq!(
            names,
            vec!["date", "open", "high", "low", "close", "adjclose", "volume"]
        );

        // Verify date column type
        let date_col = df.column("date").expect("date column exists");
        assert_eq!(date_col.dtype(), &DataType::Date);
    }

    #[test]
    fn test_build_dataframe_skips_invalid_timestamps() {
        let quotes = vec![
            yahoo::Quote {
                timestamp: 1_609_459_200, // 2021-01-01 — valid
                open: 100.0,
                high: 105.0,
                low: 99.0,
                close: 103.0,
                adjclose: 103.0,
                volume: 1_000_000,
            },
            yahoo::Quote {
                timestamp: i64::MIN, // Invalid timestamp
                open: 104.0,
                high: 106.0,
                low: 101.0,
                close: 102.0,
                adjclose: 102.0,
                volume: 900_000,
            },
        ];

        let df = build_dataframe_from_quotes(&quotes, "TEST").expect("should build dataframe");

        // Should only have 1 row (the valid one)
        assert_eq!(df.height(), 1);
    }
}
