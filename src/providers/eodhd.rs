//! EODHD data provider for historical US equity options chains.
//!
//! Ports the key functionality from `optopy-mcp/src/data/eodhd.rs` with adaptations for the
//! DataProvider trait and pipeline architecture.

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{Duration, NaiveDate, Utc};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use polars::prelude::*;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

// Constants (from optopy-mcp)
const BASE_URL: &str = "https://eodhd.com/api/mp/unicornbay";
const PAGE_LIMIT: u32 = 1000;
const MAX_OFFSET: u32 = 10_000;
const TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: u32 = 5;
const MIN_REQUEST_INTERVAL_MS: u64 = 100;
const RATE_LIMIT_SLOW_THRESHOLD: u32 = 50;
const MIN_WINDOW_DAYS: i64 = 1;
const HISTORY_DAYS: i64 = 730; // ~2 years

const FIELDS: &str = "\
    underlying_symbol,type,exp_date,expiration_type,tradetime,strike,\
    bid,ask,last,open,high,low,\
    volume,open_interest,\
    delta,gamma,theta,vega,rho,volatility,\
    midpoint,moneyness,theoretical,dte";

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
    "strike", "bid", "ask", "last", "open", "high", "low", "volume", "open_interest", "delta",
    "gamma", "theta", "vega", "rho", "implied_volatility", "midpoint", "moneyness",
    "theoretical", "dte",
];

const DEDUP_COLS: &[&str] = &[
    "quote_date",
    "expiration",
    "strike",
    "option_type",
    "expiration_type",
];

// -----------------------------------------------------------------------
// API response types
// -----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiResponse {
    meta: Option<ApiMeta>,
    data: Option<serde_json::Value>,
    links: Option<ApiLinks>,
}

#[derive(Debug, Deserialize)]
struct ApiMeta {
    fields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ApiLinks {
    next: Option<String>,
}

// -----------------------------------------------------------------------
// Provider
// -----------------------------------------------------------------------

pub struct EodhdProvider {
    client: Client,
    api_key: String,
    last_request_time: Mutex<Instant>,
    request_count: AtomicU32,
}

impl EodhdProvider {
    /// Create a new EODHD provider.
    pub fn new(api_key: String, _rate_limit_per_sec: u32) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            api_key,
            last_request_time: Mutex::new(Instant::now()),
            request_count: AtomicU32::new(0),
        }
    }

    // -- HTTP ---------------------------------------------------------------

    /// Rate-limited GET with retry on transient errors and backoff.
    async fn throttled_get(
        &self,
        url: &str,
        params: &[(String, String)],
    ) -> Result<reqwest::Response> {
        for attempt in 0..=MAX_RETRIES {
            // Enforce minimum interval between requests
            {
                let mut last = self.last_request_time.lock().await;
                let elapsed = last.elapsed();
                let min_interval = std::time::Duration::from_millis(MIN_REQUEST_INTERVAL_MS);
                if let Some(remaining) = min_interval.checked_sub(elapsed) {
                    sleep(remaining).await;
                }
                *last = Instant::now();
            }

            let resp = match self.client.get(url).query(params).send().await {
                Ok(r) => r,
                Err(e) => {
                    if attempt == MAX_RETRIES {
                        return Err(e.into());
                    }
                    let wait = 2u64.pow(attempt);
                    tracing::warn!(
                        "EODHD request error, retrying in {wait}s (attempt {}/{}): {e}",
                        attempt + 1,
                        MAX_RETRIES + 1
                    );
                    sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
            };

            self.request_count.fetch_add(1, Ordering::Relaxed);

            let status = resp.status().as_u16();

            // 5xx — exponential backoff
            if status >= 500 {
                if attempt == MAX_RETRIES {
                    return Ok(resp);
                }
                let wait = 2u64.pow(attempt + 1);
                tracing::warn!(
                    "EODHD {status} server error, backing off {wait}s (attempt {}/{})",
                    attempt + 1,
                    MAX_RETRIES + 1
                );
                sleep(std::time::Duration::from_secs(wait)).await;
                continue;
            }

            // 429 — exponential backoff
            if status == 429 {
                if attempt == MAX_RETRIES {
                    return Ok(resp);
                }
                let wait = 2u64.pow(attempt + 1);
                tracing::warn!(
                    "EODHD 429 rate limit, backing off {wait}s (attempt {}/{})",
                    attempt + 1,
                    MAX_RETRIES + 1
                );
                sleep(std::time::Duration::from_secs(wait)).await;
                continue;
            }

            // Adaptive throttle based on remaining rate limit
            if let Some(remaining) = resp.headers().get("X-RateLimit-Remaining") {
                if let Ok(remaining_str) = remaining.to_str() {
                    if let Ok(remaining_int) = remaining_str.parse::<u32>() {
                        if remaining_int < RATE_LIMIT_SLOW_THRESHOLD {
                            tracing::info!(
                                "EODHD rate limit remaining: {remaining_int}, throttling"
                            );
                            sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }

            return Ok(resp);
        }
        bail!("Max retries exceeded")
    }

    /// Return a human-readable error for known EODHD status codes.
    fn check_response(status: u16) -> Option<String> {
        match status {
            401 => Some("EODHD API key is invalid or expired.".into()),
            403 => Some("EODHD API access denied. Check your subscription plan.".into()),
            429 => Some("EODHD rate limit exceeded. Try again later.".into()),
            s if s >= 500 => Some(format!(
                "EODHD server error ({s}). The API may be temporarily unavailable."
            )),
            _ => None,
        }
    }

    // -- pagination ---------------------------------------------------------

    /// Paginate through a single date window.
    async fn paginate_window(
        &self,
        base_params: &[(String, String)],
    ) -> (Vec<HashMap<String, String>>, bool, Option<String>) {
        let mut rows: Vec<HashMap<String, String>> = Vec::new();
        let mut url = format!("{BASE_URL}/options/eod");
        let mut offset: u32 = 0;
        let mut hit_cap = false;
        let mut use_base_params = true;

        loop {
            let params: Vec<(String, String)> = if use_base_params {
                let mut p: Vec<(String, String)> = base_params.to_vec();
                p.push(("api_token".into(), self.api_key.clone()));
                p.push(("compact".into(), "1".into()));
                p.push(("page[offset]".into(), offset.to_string()));
                p
            } else {
                vec![
                    ("api_token".into(), self.api_key.clone()),
                    ("compact".into(), "1".into()),
                ]
            };

            let resp = match self.throttled_get(&url, &params).await {
                Ok(r) => r,
                Err(e) => return (vec![], false, Some(format!("Request failed: {e}"))),
            };

            let status = resp.status().as_u16();

            if let Some(error) = Self::check_response(status) {
                return (vec![], false, Some(error));
            }

            if status == 422 {
                hit_cap = true;
                break;
            }

            if !resp.status().is_success() {
                return (vec![], false, Some(format!("Unexpected status: {status}")));
            }

            let body: ApiResponse = match resp.json().await {
                Ok(b) => b,
                Err(e) => return (vec![], false, Some(format!("JSON parse error: {e}"))),
            };

            let fields = body
                .meta
                .as_ref()
                .and_then(|m| m.fields.as_ref())
                .cloned()
                .unwrap_or_default();

            let Some(data) = body.data else {
                break;
            };

            let page_rows = if fields.is_empty() {
                parse_standard_rows(&data)
            } else {
                parse_compact_rows(&fields, &data)
            };

            if page_rows.is_empty() {
                break;
            }

            rows.extend(page_rows);
            offset += PAGE_LIMIT;
            let next_url = body.links.as_ref().and_then(|l| l.next.clone());

            match next_url {
                Some(next) if offset < MAX_OFFSET => {
                    url = next;
                    use_base_params = false;
                }
                Some(_) => {
                    hit_cap = true;
                    break;
                }
                None => break,
            }
        }

        (rows, hit_cap, None)
    }

    // -- window management --------------------------------------------------

    /// Generate ~30-day windows, newest first.
    fn monthly_windows(start: NaiveDate, end: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
        let mut windows = Vec::new();
        let mut cur = end;
        while cur > start {
            let q_start = (cur - Duration::days(30)).max(start);
            windows.push((q_start, cur));
            cur = q_start - Duration::days(1);
        }
        windows
    }

    /// Fetch a single date window, subdividing if offset cap is hit.
    fn fetch_window_recursive<'a>(
        &'a self,
        symbol: &'a str,
        option_type: &'a str,
        win_from: NaiveDate,
        win_to: NaiveDate,
        mut rows_fetched: usize,
        tx: &'a mpsc::Sender<WindowChunk>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (usize, Option<String>)> + Send + 'a>>
    {
        Box::pin(async move {
            let span_days = (win_to - win_from).num_days();

            tracing::info!(
                "Fetching {symbol} {option_type} options: {win_from} to {win_to} \
                 ({span_days} days) — {rows_fetched} total rows so far"
            );

            let from_str = win_from.format("%Y-%m-%d").to_string();
            let to_str = win_to.format("%Y-%m-%d").to_string();

            let base_params: Vec<(String, String)> = vec![
                ("filter[underlying_symbol]".into(), symbol.to_string()),
                ("filter[type]".into(), option_type.to_string()),
                ("filter[tradetime_from]".into(), from_str),
                ("filter[tradetime_to]".into(), to_str),
                ("fields[options-eod]".into(), FIELDS.to_string()),
                ("page[limit]".into(), PAGE_LIMIT.to_string()),
                ("sort".into(), "exp_date".to_string()),
            ];

            let (rows, hit_cap, error) = self.paginate_window(&base_params).await;

            let window_rows = rows.len();
            if !rows.is_empty() {
                match normalize_rows(&rows) {
                    Ok(df) => {
                        let send_result: Result<(), _> = tx
                            .send(WindowChunk::OptionsWindow {
                                symbol: symbol.to_string(),
                                df,
                            })
                            .await;
                        if send_result.is_ok() {
                            rows_fetched += window_rows;
                        } else {
                            tracing::warn!("Failed to send window chunk (receiver dropped)");
                        }
                    }
                    Err(e) => tracing::warn!("Failed to normalize window data: {e}"),
                }
            }

            if let Some(ref err_msg) = error {
                tracing::warn!("Error {win_from}–{win_to} ({option_type}): {err_msg} — skipping");
                return (rows_fetched, error);
            }

            if hit_cap && span_days <= MIN_WINDOW_DAYS {
                tracing::warn!(
                    "Offset cap hit for {symbol} {option_type} on minimum window \
                     ({win_from} to {win_to}); data may be truncated"
                );
                return (
                    rows_fetched,
                    Some(format!(
                        "offset cap hit on minimum window {win_from}–{win_to}, data may be incomplete"
                    )),
                );
            }

            if hit_cap {
                rows_fetched -= window_rows;

                tracing::warn!(
                    "Offset cap hit for {symbol} {option_type} ({win_from} to {win_to}), \
                     subdividing into smaller windows"
                );

                let mid = win_from + Duration::days(span_days / 2);

                let (fetched, first_err) = self
                    .fetch_window_recursive(symbol, option_type, win_from, mid, rows_fetched, tx)
                    .await;
                rows_fetched = fetched;

                let (fetched, second_err) = self
                    .fetch_window_recursive(
                        symbol,
                        option_type,
                        mid + Duration::days(1),
                        win_to,
                        rows_fetched,
                        tx,
                    )
                    .await;
                rows_fetched = fetched;

                return (rows_fetched, first_err.or(second_err));
            }

            (rows_fetched, None)
        })
    }

    /// Fetch all rows for a single option type.
    async fn fetch_all_for_type(
        &self,
        symbol: &str,
        option_type: &str,
        resume_from: Option<NaiveDate>,
        tx: &mpsc::Sender<WindowChunk>,
        pb: &ProgressBar,
    ) -> (usize, Option<String>) {
        let today = Utc::now().date_naive();
        let start = resume_from.unwrap_or_else(|| today - Duration::days(HISTORY_DAYS));
        let end = today;

        if start >= end {
            pb.finish_with_message("up to date");
            return (0, None);
        }

        let windows = Self::monthly_windows(start, end);
        pb.set_length(windows.len() as u64);
        let mut rows_fetched: usize = 0;
        let mut last_error: Option<String> = None;

        for (win_from, win_to) in &windows {
            pb.set_message(format!("{win_from} → {win_to}"));

            let (fetched, error) = self
                .fetch_window_recursive(symbol, option_type, *win_from, *win_to, rows_fetched, tx)
                .await;
            rows_fetched = fetched;
            if error.is_some() {
                last_error = error;
            }
            pb.inc(1);
        }

        pb.finish_with_message(format!("{rows_fetched} rows"));
        (rows_fetched, last_error)
    }
}

#[async_trait]
impl crate::providers::DataProvider for EodhdProvider {
    fn name(&self) -> &str {
        "EODHD"
    }

    fn category(&self) -> &str {
        "options"
    }

    async fn download(
        &self,
        symbol: &str,
        _params: &DownloadParams,
        cache: &CacheStore,
        tx: mpsc::Sender<WindowChunk>,
        _shutdown: CancellationToken,
    ) -> Result<DownloadResult> {
        let symbol = symbol.to_uppercase();
        let request_count_before = self.request_count.load(Ordering::Relaxed);

        let mp = MultiProgress::new();
        let bar_style = ProgressStyle::default_bar()
            .template("  {prefix:.bold} [{bar:30.cyan/dim}] {pos}/{len} windows  {msg}")
            .expect("valid template")
            .progress_chars("=> ");

        let mut errors: Vec<String> = Vec::new();
        let mut new_rows_total: usize = 0;

        for option_type in &["call", "put"] {
            let pb = mp.add(ProgressBar::new(0));
            pb.set_style(bar_style.clone());
            pb.set_prefix(format!("{symbol} {option_type}s"));

            let (new_rows, error) = self
                .fetch_all_for_type(&symbol, option_type, None, &tx, &pb)
                .await;

            if let Some(err) = error {
                pb.abandon_with_message(format!("error: {err}"));
                errors.push(format!("{option_type}: {err}"));
            }

            new_rows_total += new_rows;
        }

        // Read cache to get totals
        let options_path = cache.options_path(&symbol)?;
        let cached_lf = cache.read_parquet(&options_path).await?;

        let (total_rows, date_range) = if let Some(lf) = cached_lf {
            if let Ok(df) = lf.collect() {
                let rows = df.height();
                let date_range = extract_date_range(&df);
                (rows, date_range)
            } else {
                (0, None)
            }
        } else {
            (0, None)
        };

        let api_requests = self.request_count.load(Ordering::Relaxed) - request_count_before;
        tracing::info!("EODHD: {symbol} completed ({api_requests} API requests)");

        Ok(DownloadResult::success(
            symbol,
            self.name().to_string(),
            new_rows_total,
            total_rows,
            date_range,
        )
        .with_errors(errors))
    }
}

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn parse_compact_rows(fields: &[String], data: &serde_json::Value) -> Vec<HashMap<String, String>> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|row| {
            let vals = row.as_array()?;
            let mut map = HashMap::new();
            for (i, field) in fields.iter().enumerate() {
                if let Some(val) = vals.get(i) {
                    let s = match val {
                        serde_json::Value::Null => continue,
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        other => other.to_string(),
                    };
                    map.insert(field.clone(), s);
                }
            }
            Some(map)
        })
        .collect()
}

fn parse_standard_rows(data: &serde_json::Value) -> Vec<HashMap<String, String>> {
    let Some(arr) = data.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|row| {
            let obj = row
                .as_object()
                .and_then(|o| o.get("attributes"))
                .and_then(|a| a.as_object())
                .or_else(|| row.as_object())?;
            let mut map = HashMap::new();
            for (k, v) in obj {
                let s = match v {
                    serde_json::Value::Null => continue,
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                map.insert(k.clone(), s);
            }
            Some(map)
        })
        .collect()
}

fn normalize_rows(rows: &[HashMap<String, String>]) -> Result<DataFrame> {
    if rows.is_empty() {
        return Ok(DataFrame::empty());
    }

    let column_map: HashMap<&str, &str> = COLUMN_MAP.iter().copied().collect();

    let mut seen = std::collections::HashSet::new();
    let mut api_fields: Vec<String> = Vec::new();
    for row in rows {
        for key in row.keys() {
            if seen.insert(key.clone()) {
                api_fields.push(key.clone());
            }
        }
    }

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

fn extract_date_range(df: &DataFrame) -> Option<(NaiveDate, NaiveDate)> {
    let col = df.column("quote_date").ok()?;

    let format_scalar = |s: &Scalar| -> Option<NaiveDate> {
        match s.value() {
            AnyValue::Date(days) => {
                NaiveDate::from_num_days_from_ce_opt(days + 719_163)
            }
            _ => None,
        }
    };

    let min = col.min_reduce().ok().and_then(|s| format_scalar(&s))?;
    let max = col.max_reduce().ok().and_then(|s| format_scalar(&s))?;
    Some((min, max))
}
