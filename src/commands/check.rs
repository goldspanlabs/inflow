//! Data quality check command implementation.

use crate::cache::CacheStore;
use crate::utils::{
    OPTIONS_CRITICAL_COLUMNS, OPTIONS_DATE_COLUMN, OPTIONS_DEDUP_COLS, OPTIONS_EXPECTED_COLUMNS,
    PRICES_DATE_COLUMN, PRICES_EXPECTED_COLUMNS,
};
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use console::Style;
use polars::prelude::*;
use std::collections::{BTreeSet, HashMap};

// ─── Result types ───────────────────────────────────────────────────────────

#[derive(Debug)]
enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug)]
struct CheckResult {
    name: &'static str,
    status: CheckStatus,
    message: String,
}

impl CheckResult {
    fn pass(name: &'static str, message: String) -> Self {
        Self {
            name,
            status: CheckStatus::Pass,
            message,
        }
    }
    fn warn(name: &'static str, message: String) -> Self {
        Self {
            name,
            status: CheckStatus::Warn,
            message,
        }
    }
    fn fail(name: &'static str, message: String) -> Self {
        Self {
            name,
            status: CheckStatus::Fail,
            message,
        }
    }
}

// ─── Entry point ────────────────────────────────────────────────────────────

/// Execute the check command.
pub async fn execute(cache: &CacheStore, symbols: &[String]) -> Result<()> {
    let options_symbols = resolve_symbols(cache, "options", symbols)?;
    let prices_symbols = resolve_symbols(cache, "prices", symbols)?;

    if options_symbols.is_empty() && prices_symbols.is_empty() {
        println!("\nNo cached data found. Run 'inflow download' first.\n");
        return Ok(());
    }

    println!("\nData Quality Report\n");

    // Merge symbols from both categories into a single ordered list
    let all_symbols: Vec<String> = {
        let mut seen = BTreeSet::new();
        let mut merged = Vec::new();
        for s in options_symbols.iter().chain(prices_symbols.iter()) {
            if seen.insert(s.clone()) {
                merged.push(s.clone());
            }
        }
        merged
    };
    let options_set: BTreeSet<&String> = options_symbols.iter().collect();
    let prices_set: BTreeSet<&String> = prices_symbols.iter().collect();

    for symbol in &all_symbols {
        let prices_path = cache.prices_path(symbol)?;
        let prices_data = match cache.read_parquet(&prices_path).await? {
            Some(lf) => Some(collect_blocking(lf).await?),
            None => None,
        };

        let mut has_output = false;

        if options_set.contains(symbol) {
            let opts_path = cache.options_path(symbol)?;
            if let Some(options_lf) = cache.read_parquet(&opts_path).await? {
                let options_data = collect_blocking(options_lf).await?;
                let results = check_options(&options_data, prices_data.as_ref());
                if has_issues(&results) {
                    if !has_output {
                        println!("  {symbol}");
                        has_output = true;
                    }
                    print_section("Options", &results);
                }
            }
        }

        if prices_set.contains(symbol) {
            if let Some(ref df) = prices_data {
                let results = check_prices(df);
                if has_issues(&results) {
                    if !has_output {
                        println!("  {symbol}");
                    }
                    print_section("Prices", &results);
                }
            }
        }
    }

    println!();
    Ok(())
}

/// Collect a `LazyFrame` on a blocking thread.
async fn collect_blocking(lf: LazyFrame) -> Result<DataFrame> {
    tokio::task::spawn_blocking(move || lf.collect().map_err(anyhow::Error::from)).await?
}

/// Resolve which symbols to check. If `filter` is empty, return all cached symbols.
fn resolve_symbols(cache: &CacheStore, category: &str, filter: &[String]) -> Result<Vec<String>> {
    let all = cache.list_symbols(category)?;
    if filter.is_empty() {
        return Ok(all);
    }
    let filter_upper: BTreeSet<String> = filter.iter().map(|s| s.to_uppercase()).collect();
    Ok(all
        .into_iter()
        .filter(|s| filter_upper.contains(s))
        .collect())
}

// ─── Output formatting ─────────────────────────────────────────────────────

fn has_issues(results: &[CheckResult]) -> bool {
    results
        .iter()
        .any(|r| !matches!(r.status, CheckStatus::Pass))
}

fn print_section(header: &str, results: &[CheckResult]) {
    let warn_style = Style::new().yellow().bold();
    let fail_style = Style::new().red().bold();

    println!("    {header}");
    for r in results {
        if matches!(r.status, CheckStatus::Pass) {
            continue;
        }
        let tag = match r.status {
            CheckStatus::Pass => unreachable!(),
            CheckStatus::Warn => warn_style.apply_to("[WARN]"),
            CheckStatus::Fail => fail_style.apply_to("[FAIL]"),
        };
        println!("      {tag} {}: {}", r.name, r.message);
    }
}

// ─── Options checks ────────────────────────────────────────────────────────

fn check_options(df: &DataFrame, prices_df: Option<&DataFrame>) -> Vec<CheckResult> {
    vec![
        check_options_gaps(df, prices_df),
        check_options_duplicates(df),
        check_options_schema(df),
        check_options_nulls_outliers(df),
        check_options_delta_coverage(df),
    ]
}

/// 1. Gap detection: compare options `quote_dates` against prices trading days.
fn check_options_gaps(df: &DataFrame, prices_df: Option<&DataFrame>) -> CheckResult {
    let name = "Gaps";

    let Some(prices_df) = prices_df else {
        return CheckResult::warn(name, "No prices cache to compare against".to_string());
    };

    let opts_dates = extract_date_set(df, OPTIONS_DATE_COLUMN);
    let prices_dates = extract_date_set(prices_df, PRICES_DATE_COLUMN);

    if opts_dates.is_empty() || prices_dates.is_empty() {
        return CheckResult::warn(name, "Could not extract dates for comparison".to_string());
    }

    // Find overlapping range
    let overlap_start = *opts_dates
        .iter()
        .next()
        .unwrap()
        .max(prices_dates.iter().next().unwrap());
    let overlap_end = *opts_dates
        .iter()
        .next_back()
        .unwrap()
        .min(prices_dates.iter().next_back().unwrap());

    if overlap_start > overlap_end {
        return CheckResult::warn(
            name,
            "No overlapping date range between options and prices".to_string(),
        );
    }

    // Trading days in overlap that options is missing
    let missing: Vec<NaiveDate> = prices_dates
        .range(overlap_start..=overlap_end)
        .filter(|d| !opts_dates.contains(d))
        .copied()
        .collect();

    let trading_days = prices_dates.range(overlap_start..=overlap_end).count();

    if missing.is_empty() {
        CheckResult::pass(
            name,
            format!("No gaps detected ({trading_days} trading days)"),
        )
    } else if missing.len() <= 5 {
        let dates_str: Vec<String> = missing.iter().map(ToString::to_string).collect();
        CheckResult::warn(
            name,
            format!(
                "{} missing trading day(s): {}",
                missing.len(),
                dates_str.join(", ")
            ),
        )
    } else {
        CheckResult::warn(
            name,
            format!(
                "{} missing trading days out of {trading_days}",
                missing.len()
            ),
        )
    }
}

/// 2. Duplicate detection by `OPTIONS_DEDUP_COLS`.
fn check_options_duplicates(df: &DataFrame) -> CheckResult {
    let name = "Duplicates";
    let total = df.height();

    // Check that all dedup columns exist
    for col in OPTIONS_DEDUP_COLS {
        if df.column(col).is_err() {
            return CheckResult::fail(name, format!("Missing column '{col}' for dedup check"));
        }
    }

    let dedup_cols: Vec<String> = OPTIONS_DEDUP_COLS.iter().map(|&s| s.to_string()).collect();
    match df.unique::<String, String>(Some(&dedup_cols), UniqueKeepStrategy::First, None) {
        Ok(unique_df) => {
            let unique = unique_df.height();
            let dupes = total - unique;
            if dupes == 0 {
                CheckResult::pass(name, format!("No duplicates ({total} rows)"))
            } else {
                CheckResult::warn(name, format!("{dupes} duplicate rows out of {total}"))
            }
        }
        Err(e) => CheckResult::fail(name, format!("Dedup check failed: {e}")),
    }
}

/// 3. Schema validation for options data.
fn check_options_schema(df: &DataFrame) -> CheckResult {
    let name = "Schema";
    let mut issues = Vec::new();

    for &col_name in OPTIONS_EXPECTED_COLUMNS {
        if df.column(col_name).is_err() {
            issues.push(format!("missing '{col_name}'"));
        }
    }

    // Check quote_date type
    if let Ok(col) = df.column(OPTIONS_DATE_COLUMN) {
        match col.dtype() {
            DataType::Date => {}
            other => issues.push(format!("{OPTIONS_DATE_COLUMN} is {other}, expected Date")),
        }
    }

    // Check numeric columns have numeric types
    for &col_name in &[
        "strike", "bid", "ask", "last", "delta", "gamma", "theta", "vega",
    ] {
        if let Ok(col) = df.column(col_name) {
            if !col.dtype().is_numeric() {
                issues.push(format!("'{col_name}' is {}, expected numeric", col.dtype()));
            }
        }
    }

    if issues.is_empty() {
        let ncols = df.get_column_names().len();
        CheckResult::pass(name, format!("Schema valid ({ncols} columns)"))
    } else {
        CheckResult::warn(name, issues.join("; "))
    }
}

/// 4. Null and outlier detection in options data.
fn check_options_nulls_outliers(df: &DataFrame) -> CheckResult {
    let name = "Nulls/Outliers";
    let mut issues = Vec::new();

    // Null checks on critical columns
    for &col_name in OPTIONS_CRITICAL_COLUMNS {
        if let Ok(col) = df.column(col_name) {
            let nulls = col.null_count();
            if nulls > 0 {
                issues.push(format!("{nulls} nulls in {col_name}"));
            }
        }
    }

    // Zero-price checks using native chunked array iteration (avoids df.clone())
    let total = df.height();
    let mut zero_cols = Vec::new();
    for &col_name in &["bid", "ask", "last"] {
        if let Ok(c) = df.column(col_name) {
            if let Ok(ca) = c.f64() {
                let zeros: usize = ca.into_iter().filter(|v| *v == Some(0.0)).count();
                if zeros > 0 {
                    let pct = (zeros as f64 / total as f64) * 100.0;
                    zero_cols.push(format!("{col_name}({zeros}, {pct:.1}%)"));
                }
            }
        }
    }
    if !zero_cols.is_empty() {
        issues.push(format!("zero prices in {}", zero_cols.join(", ")));
    }

    // Delta outlier check: abs(delta) > 1.0 using chunked array
    if let Ok(c) = df.column("delta") {
        if let Ok(ca) = c.f64() {
            let outliers: usize = ca
                .into_iter()
                .filter(|v| matches!(v, Some(d) if d.abs() > 1.0))
                .count();
            if outliers > 0 {
                issues.push(format!("{outliers} rows with |delta| > 1.0"));
            }
        }
    }

    if issues.is_empty() {
        CheckResult::pass(name, "No issues".to_string())
    } else {
        CheckResult::warn(name, issues.join("; "))
    }
}

/// 5. Delta coverage: % of trading dates where both calls and puts span
///    from near-ATM (|delta| ≥ 0.8) out to the wings (|delta| ≤ 0.2).
fn check_options_delta_coverage(df: &DataFrame) -> CheckResult {
    let name = "Delta Coverage";

    let Ok(date_col) = df.column(OPTIONS_DATE_COLUMN).and_then(|c| c.date()) else {
        return CheckResult::warn(name, "Missing or invalid quote_date column".to_string());
    };
    let Ok(delta_ca) = df.column("delta").and_then(|c| c.f64()) else {
        return CheckResult::warn(name, "Missing or invalid delta column".to_string());
    };
    let Ok(type_ca) = df.column("option_type").and_then(|c| c.str()) else {
        return CheckResult::warn(name, "Missing or invalid option_type column".to_string());
    };

    // Use the physical Int32Chunked for date iteration (DateChunked is a logical wrapper)
    let date_phys = date_col.cast(&DataType::Int32).unwrap();
    let date_ca = date_phys.i32().unwrap();

    // Aggregate min/max |delta| per (date, option_type) using native chunked iteration
    // Key: (date_i32, is_call) → (min_abs_delta, max_abs_delta)
    let mut group_stats: HashMap<(i32, bool), (f64, f64)> = HashMap::new();
    let mut all_dates: BTreeSet<i32> = BTreeSet::new();

    for ((date_opt, delta_opt), type_opt) in date_ca
        .into_iter()
        .zip(delta_ca.into_iter())
        .zip(type_ca.into_iter())
    {
        let (Some(date_val), Some(delta_raw), Some(opt_type)): (
            Option<i32>,
            Option<f64>,
            Option<&str>,
        ) = (date_opt, delta_opt, type_opt) else {
            continue;
        };
        let delta = delta_raw.abs();

        all_dates.insert(date_val);
        let is_call = opt_type.eq_ignore_ascii_case("call");
        let key = (date_val, is_call);

        group_stats
            .entry(key)
            .and_modify(|(min_d, max_d)| {
                if delta < *min_d {
                    *min_d = delta;
                }
                if delta > *max_d {
                    *max_d = delta;
                }
            })
            .or_insert((delta, delta));
    }

    let total_dates = all_dates.len();
    if total_dates == 0 {
        return CheckResult::warn(name, "No trading dates found".to_string());
    }

    // A date passes if both call and put groups exist and each has
    // min(|delta|) ≤ 0.2 AND max(|delta|) ≥ 0.8
    let passing_dates: BTreeSet<i32> = all_dates
        .iter()
        .copied()
        .filter(|&date| {
            let call_ok = group_stats
                .get(&(date, true))
                .is_some_and(|&(min_d, max_d)| min_d <= 0.2 && max_d >= 0.8);
            let put_ok = group_stats
                .get(&(date, false))
                .is_some_and(|&(min_d, max_d)| min_d <= 0.2 && max_d >= 0.8);
            call_ok && put_ok
        })
        .collect();

    let passing_count = passing_dates.len();
    let pct = (passing_count as f64 / total_dates as f64) * 100.0;

    // Compute median strikes per day from the raw date column
    let median_strikes = {
        let mut day_counts: HashMap<i32, usize> = HashMap::new();
        for d in date_ca.into_iter().flatten() {
            *day_counts.entry(d).or_insert(0) += 1;
        }
        let mut counts: Vec<usize> = day_counts.into_values().collect();
        counts.sort_unstable();
        if counts.is_empty() {
            0
        } else {
            counts[counts.len() / 2]
        }
    };

    let msg = format!(
        "{pct:.1}% of dates have delta 0.2\u{2013}0.8 coverage; ~{median_strikes} strikes/day"
    );

    if pct >= 80.0 {
        CheckResult::pass(name, msg)
    } else {
        CheckResult::warn(name, msg)
    }
}

// ─── Prices checks ─────────────────────────────────────────────────────────

fn check_prices(df: &DataFrame) -> Vec<CheckResult> {
    vec![
        check_prices_schema(df),
        check_prices_nulls_outliers(df),
        check_prices_gaps(df),
    ]
}

/// 1. Schema validation for prices data.
fn check_prices_schema(df: &DataFrame) -> CheckResult {
    let name = "Schema";
    let mut issues = Vec::new();

    for &col_name in PRICES_EXPECTED_COLUMNS {
        if df.column(col_name).is_err() {
            issues.push(format!("missing '{col_name}'"));
        }
    }

    // Check date type
    if let Ok(col) = df.column(PRICES_DATE_COLUMN) {
        match col.dtype() {
            DataType::Date => {}
            other => issues.push(format!("{PRICES_DATE_COLUMN} is {other}, expected Date")),
        }
    }

    // Check numeric columns
    for &col_name in &["open", "high", "low", "close", "adjclose"] {
        if let Ok(col) = df.column(col_name) {
            if !col.dtype().is_numeric() {
                issues.push(format!("'{col_name}' is {}, expected numeric", col.dtype()));
            }
        }
    }

    if issues.is_empty() {
        let ncols = df.get_column_names().len();
        CheckResult::pass(name, format!("Schema valid ({ncols} columns)"))
    } else {
        CheckResult::warn(name, issues.join("; "))
    }
}

/// 2. Null and outlier detection in prices data.
fn check_prices_nulls_outliers(df: &DataFrame) -> CheckResult {
    let name = "Nulls/Outliers";
    let mut issues = Vec::new();

    // Null checks
    for &col_name in &[PRICES_DATE_COLUMN, "close"] {
        if let Ok(col) = df.column(col_name) {
            let nulls = col.null_count();
            if nulls > 0 {
                issues.push(format!("{nulls} nulls in {col_name}"));
            }
        }
    }

    // Zero or negative prices using native chunked array iteration (avoids df.clone())
    let total = df.height();
    for &col_name in &["open", "high", "low", "close", "adjclose"] {
        if let Ok(c) = df.column(col_name) {
            if let Ok(ca) = c.f64() {
                let bad: usize = ca
                    .into_iter()
                    .filter(|v| matches!(v, Some(x) if *x <= 0.0))
                    .count();
                if bad > 0 {
                    let pct = (bad as f64 / total as f64) * 100.0;
                    issues.push(format!(
                        "{bad} zero/negative values in {col_name} ({pct:.1}%)"
                    ));
                }
            }
        }
    }

    if issues.is_empty() {
        CheckResult::pass(name, "No issues".to_string())
    } else {
        CheckResult::warn(name, issues.join("; "))
    }
}

/// 3. Gap detection in prices: missing weekdays, report gaps > 3 consecutive trading days.
fn check_prices_gaps(df: &DataFrame) -> CheckResult {
    let name = "Gaps";

    let dates = extract_date_set(df, PRICES_DATE_COLUMN);
    if dates.is_empty() {
        return CheckResult::warn(name, "No dates found".to_string());
    }

    let first = *dates.iter().next().unwrap();
    let last = *dates.iter().next_back().unwrap();

    // Generate all weekdays in range
    let mut expected_weekdays = BTreeSet::new();
    let mut d = first;
    while d <= last {
        let wd = d.weekday();
        if wd != chrono::Weekday::Sat && wd != chrono::Weekday::Sun {
            expected_weekdays.insert(d);
        }
        d += chrono::Duration::days(1);
    }

    let missing: Vec<NaiveDate> = expected_weekdays.difference(&dates).copied().collect();

    // Find consecutive gaps > 3 trading days
    let mut long_gaps = Vec::new();
    let mut i = 0;
    while i < missing.len() {
        let gap_start = missing[i];
        let mut gap_end = gap_start;
        let mut count = 1;
        while i + 1 < missing.len() {
            let next = missing[i + 1];
            // Bridge Fri→Mon (3 calendar days), otherwise require consecutive weekdays (1 day)
            let max_gap = if gap_end.weekday() == chrono::Weekday::Fri {
                3
            } else {
                1
            };
            if (next - gap_end).num_days() <= max_gap {
                gap_end = next;
                count += 1;
                i += 1;
            } else {
                break;
            }
        }
        if count > 3 {
            long_gaps.push(format!("{gap_start} to {gap_end} ({count} days)"));
        }
        i += 1;
    }

    if long_gaps.is_empty() {
        CheckResult::pass(
            name,
            format!(
                "No gaps > 3 trading days ({} total missing weekdays)",
                missing.len()
            ),
        )
    } else {
        CheckResult::warn(
            name,
            format!(
                "{} gap(s) > 3 trading days: {}",
                long_gaps.len(),
                long_gaps.join("; ")
            ),
        )
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract all unique dates from a column into a sorted set.
///
/// Casts the date column to its physical `Int32` representation and iterates
/// natively, avoiding per-row dynamic dispatch via `col.get(i)`.
fn extract_date_set(df: &DataFrame, col_name: &str) -> BTreeSet<NaiveDate> {
    let mut dates = BTreeSet::new();
    let Ok(col) = df.column(col_name) else {
        return dates;
    };
    let Ok(phys) = col.cast(&DataType::Int32) else {
        return dates;
    };
    let Ok(ca) = phys.i32() else {
        return dates;
    };
    for days in ca.into_iter().flatten() {
        if let Some(d) =
            NaiveDate::from_num_days_from_ce_opt(days + crate::utils::EXCEL_DATE_EPOCH_OFFSET)
        {
            dates.insert(d);
        }
    }
    dates
}
