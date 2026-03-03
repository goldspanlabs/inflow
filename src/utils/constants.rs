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

/// Expected columns in options data (for schema validation).
pub const OPTIONS_EXPECTED_COLUMNS: &[&str] = &[
    "quote_date",
    "expiration",
    "strike",
    "option_type",
    "expiration_type",
    "bid",
    "ask",
    "last",
    "volume",
    "open_interest",
    "implied_volatility",
    "delta",
    "gamma",
    "theta",
    "vega",
    "rho",
    "symbol",
];

/// Critical columns in options data that must not be null.
pub const OPTIONS_CRITICAL_COLUMNS: &[&str] = &[
    "quote_date",
    "symbol",
    "option_type",
    "expiration",
    "strike",
];

/// Expected columns in prices data (for schema validation).
pub const PRICES_EXPECTED_COLUMNS: &[&str] =
    &["date", "open", "high", "low", "close", "adjclose", "volume"];
