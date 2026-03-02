//! JSON parsing and `DataFrame` normalization for EODHD API responses.

use anyhow::{Context, Result};
use polars::prelude::*;
use std::collections::HashMap;

// Constants for data transformation
const COLUMN_MAP: &[(&str, &str)] = &[
    ("underlying_symbol", "underlying_symbol"),
    ("type", "option_type"),
    ("exp_date", "expiration"),
    ("expiration_type", "expiration_type"),
    ("tradetime", "quote_date"),
    ("strike", "strike"),
    ("bid", "bid"),
    ("ask", "ask"),
    ("last", "last"),
    ("open", "open"),
    ("high", "high"),
    ("low", "low"),
    ("volume", "volume"),
    ("open_interest", "open_interest"),
    ("delta", "delta"),
    ("gamma", "gamma"),
    ("theta", "theta"),
    ("vega", "vega"),
    ("rho", "rho"),
    ("volatility", "implied_volatility"),
    ("midpoint", "midpoint"),
    ("moneyness", "moneyness"),
    ("theoretical", "theoretical"),
    ("dte", "dte"),
];

const NUMERIC_COLS: &[&str] = &[
    "strike",
    "bid",
    "ask",
    "last",
    "open",
    "high",
    "low",
    "volume",
    "open_interest",
    "delta",
    "gamma",
    "theta",
    "vega",
    "rho",
    "implied_volatility",
    "midpoint",
    "moneyness",
    "theoretical",
    "dte",
];

/// Convert raw API rows into a normalized `DataFrame`.
///
/// Applies column renames, date parsing, and numeric coercion.
#[allow(clippy::implicit_hasher)]
pub fn normalize_rows(rows: &[HashMap<String, String>]) -> Result<DataFrame> {
    if rows.is_empty() {
        return Ok(DataFrame::empty());
    }

    let column_map: HashMap<&str, &str> = COLUMN_MAP.iter().copied().collect();

    // Collect all API field names actually present in the data
    let mut seen = std::collections::HashSet::new();
    let mut api_fields: Vec<String> = Vec::new();
    for row in rows {
        for key in row.keys() {
            if seen.insert(key.clone()) {
                api_fields.push(key.clone());
            }
        }
    }

    // Build string columns, using the internal name.
    // Normalize option_type to lowercase ("call"/"put") during construction.
    let n = rows.len();
    let columns: Vec<Column> = api_fields
        .iter()
        .map(|api_name| {
            let fallback = api_name.as_str();
            let internal_name = *column_map.get(api_name.as_str()).unwrap_or(&fallback);
            if internal_name == "option_type" {
                let values: Vec<Option<String>> = rows
                    .iter()
                    .map(|row| row.get(api_name).map(|s| s.to_lowercase()))
                    .collect();
                Column::new(internal_name.into(), values)
            } else {
                let values: Vec<Option<&str>> = rows
                    .iter()
                    .map(|row| row.get(api_name).map(String::as_str))
                    .collect();
                Column::new(internal_name.into(), values)
            }
        })
        .collect();

    let df = DataFrame::new(n, columns).context("Failed to build DataFrame from API rows")?;

    // Cast date columns from string → Date, numeric columns → Float64
    let schema = df.schema().clone();
    let mut lf = df.lazy();

    if schema.contains("expiration") {
        lf = lf.with_column(col("expiration").cast(DataType::Date).alias("expiration"));
    }
    if schema.contains("quote_date") {
        lf = lf.with_column(col("quote_date").cast(DataType::Date).alias("quote_date"));
    }

    let numeric_exprs: Vec<Expr> = NUMERIC_COLS
        .iter()
        .filter(|c| schema.contains(c))
        .map(|c| col(*c).cast(DataType::Float64).alias(*c))
        .collect();
    if !numeric_exprs.is_empty() {
        lf = lf.with_columns(numeric_exprs);
    }

    lf.collect()
        .context("Failed to normalize DataFrame columns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rows_applies_column_map() {
        let rows = vec![{
            let mut m = HashMap::new();
            m.insert("underlying_symbol".to_string(), "SPY".to_string());
            m.insert("type".to_string(), "Call".to_string());
            m.insert("exp_date".to_string(), "2024-03-15".to_string());
            m.insert("tradetime".to_string(), "2024-01-15".to_string());
            m.insert("strike".to_string(), "500.0".to_string());
            m.insert("bid".to_string(), "5.20".to_string());
            m.insert("ask".to_string(), "5.40".to_string());
            m.insert("delta".to_string(), "0.45".to_string());
            m
        }];
        let df = normalize_rows(&rows).unwrap();

        // Column renames applied
        assert!(df.schema().contains("option_type"));
        assert!(df.schema().contains("expiration"));
        assert!(df.schema().contains("quote_date"));
        assert!(!df.schema().contains("type"));
        assert!(!df.schema().contains("exp_date"));
        assert!(!df.schema().contains("tradetime"));

        // Numeric columns cast to f64
        assert_eq!(*df.column("strike").unwrap().dtype(), DataType::Float64);
        assert_eq!(*df.column("delta").unwrap().dtype(), DataType::Float64);

        // Date columns cast to Date
        assert_eq!(*df.column("expiration").unwrap().dtype(), DataType::Date);
        assert_eq!(*df.column("quote_date").unwrap().dtype(), DataType::Date);

        // option_type normalized to lowercase
        let ot = df.column("option_type").unwrap();
        assert_eq!(ot.str().unwrap().get(0).unwrap(), "call");
    }
}
