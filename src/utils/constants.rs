//! Shared constants across inflow modules.

/// Columns used to deduplicate options data.
///
/// When merging options chains, rows are deduplicated based on these columns,
/// keeping the last occurrence of each unique combination.
pub const OPTIONS_DEDUP_COLS: &[&str] = &[
    "quote_date",
    "expiration",
    "strike",
    "option_type",
    "expiration_type",
];

/// Date column name in options data.
pub const OPTIONS_DATE_COLUMN: &str = "quote_date";

/// Date column name in prices data.
pub const PRICES_DATE_COLUMN: &str = "date";
