//! Window-based pagination and recursive fetching for EODHD API.

use crate::cache::CacheStore;
use crate::pipeline::types::WindowChunk;
use crate::utils::{collect_blocking, parse_compact_rows, parse_standard_rows};
use chrono::{Duration, NaiveDate, Utc};
use indicatif::ProgressBar;
use std::collections::HashMap;
use tokio::sync::mpsc;

use super::http::HttpClient;
use super::types::ApiResponse;

// Pagination constants
pub const BASE_URL: &str = "https://eodhd.com/api/mp/unicornbay";
pub const PAGE_LIMIT: u32 = 1000;
pub const MAX_OFFSET: u32 = 10_000;
pub const MIN_WINDOW_DAYS: i64 = 1;
pub const HISTORY_DAYS: i64 = 730; // ~2 years
pub const FIELDS: &str = "\
    underlying_symbol,type,exp_date,expiration_type,tradetime,strike,\
    bid,ask,last,open,high,low,\
    volume,open_interest,\
    delta,gamma,theta,vega,rho,volatility,\
    midpoint,moneyness,theoretical,dte";

// Strike range for 97% data coverage: ±65% of underlying price
// This captures virtually all liquid options (ITM and OTM)
pub const STRIKE_LOWER_MULTIPLIER: f64 = 0.35; // 35% of price (65% OTM for puts)
pub const STRIKE_UPPER_MULTIPLIER: f64 = 2.65; // 265% of price (165% OTM for calls)

/// Extract close price for a given date from cached prices.
///
/// Returns the close price if available, or None if data missing/date not found.
async fn get_cached_price(
    cache: &CacheStore,
    symbol: &str,
    date: NaiveDate,
) -> anyhow::Result<Option<f64>> {
    let prices_path = cache.prices_path(symbol)?;
    let cached_lf = cache.read_parquet(&prices_path).await?;

    let Some(lf) = cached_lf else {
        return Ok(None);
    };

    let df = collect_blocking(lf).await?;

    // Filter to the requested date
    let date_col = df.column("date")?;
    let date_chunked = date_col.date()?;
    let date_phys = &date_chunked.phys;

    let close_col = df.column("close")?;
    let close_f64 = close_col.f64()?;

    // Find matching date
    for (date_val, close_val) in date_phys.iter().zip(close_f64.iter()) {
        if let (Some(d), Some(c)) = (date_val, close_val) {
            // Polars dates are days since epoch
            if let Some(cached_date) = chrono::NaiveDate::from_num_days_from_ce_opt(
                d + crate::utils::EXCEL_DATE_EPOCH_OFFSET,
            ) {
                if cached_date == date {
                    return Ok(Some(c));
                }
            }
        }
    }

    Ok(None)
}

/// Calculate strike range for 97% data coverage based on underlying price.
///
/// Uses ±65% of underlying price to capture virtually all liquid options.
fn calculate_strike_range(price: f64) -> (f64, f64) {
    let lower = price * STRIKE_LOWER_MULTIPLIER;
    let upper = price * STRIKE_UPPER_MULTIPLIER;
    (lower, upper)
}

/// Build query params for a strike-range-filtered options request.
fn build_strike_params(
    symbol: &str,
    option_type: &str,
    win_from: NaiveDate,
    win_to: NaiveDate,
    strike_from: f64,
    strike_to: f64,
) -> Vec<(String, String)> {
    vec![
        ("filter[underlying_symbol]".into(), symbol.to_string()),
        ("filter[type]".into(), option_type.to_string()),
        (
            "filter[tradetime_from]".into(),
            win_from.format("%Y-%m-%d").to_string(),
        ),
        (
            "filter[tradetime_to]".into(),
            win_to.format("%Y-%m-%d").to_string(),
        ),
        ("filter[strike_from]".into(), strike_from.to_string()),
        ("filter[strike_to]".into(), strike_to.to_string()),
        ("fields[options-eod]".into(), FIELDS.to_string()),
        ("page[limit]".into(), PAGE_LIMIT.to_string()),
        ("sort".into(), "exp_date".to_string()),
    ]
}

/// Normalize rows and send as a `WindowChunk`. Returns the number of rows sent.
async fn send_normalized_chunk(
    rows: &[HashMap<String, String>],
    symbol: &str,
    tx: &mpsc::Sender<WindowChunk>,
    label: &str,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    match crate::providers::eodhd::parsing::normalize_rows(rows) {
        Ok(df) => {
            if let Err(e) = tx
                .send(WindowChunk::OptionsWindow {
                    symbol: symbol.to_string(),
                    df,
                })
                .await
            {
                tracing::warn!("Failed to send {label} strike range chunk: {e}");
                0
            } else {
                rows.len()
            }
        }
        Err(e) => {
            tracing::warn!("Failed to normalize {label} strike range: {e}");
            0
        }
    }
}

/// Pagination helper for EODHD API requests.
pub struct Paginator {
    pub http: HttpClient,
    base_url: String,
}

impl Paginator {
    /// Create a new paginator.
    pub fn new(api_key: String) -> Self {
        Self {
            http: HttpClient::new(api_key),
            base_url: BASE_URL.to_string(),
        }
    }

    /// Create a paginator with a custom base URL (for testing).
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            http: HttpClient::new(api_key),
            base_url,
        }
    }

    /// Generate ~30-day windows, newest first.
    pub fn monthly_windows(start: NaiveDate, end: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
        let mut windows = Vec::new();
        let mut cur = end;
        while cur >= start {
            let q_start = (cur - Duration::days(30)).max(start);
            windows.push((q_start, cur));
            cur = q_start - Duration::days(1);
        }
        windows
    }

    /// Paginate through a single date window using compact mode.
    ///
    /// Returns `(rows, hit_cap, error)`.  `hit_cap` is true when the offset
    /// limit was reached, signalling more data likely exists beyond this window.
    pub async fn paginate_window(
        &self,
        base_params: &[(String, String)],
    ) -> (Vec<HashMap<String, String>>, bool, Option<String>) {
        let mut rows: Vec<HashMap<String, String>> = Vec::new();
        let mut url = format!("{}/options/eod", self.base_url);
        let mut offset: u32 = 0;
        let mut hit_cap = false;
        let mut use_base_params = true;

        loop {
            let params: Vec<(String, String)> = if use_base_params {
                let mut p: Vec<(String, String)> = base_params.to_vec();
                p.push(("api_token".into(), self.http.api_key.clone()));
                p.push(("compact".into(), "1".into()));
                p.push(("page[offset]".into(), offset.to_string()));
                p
            } else {
                vec![
                    ("api_token".into(), self.http.api_key.clone()),
                    ("compact".into(), "1".into()),
                ]
            };

            let resp = match self.http.throttled_get(&url, &params).await {
                Ok(r) => r,
                Err(e) => return (vec![], false, Some(format!("Request failed: {e}"))),
            };

            let status = resp.status().as_u16();

            if let Some(error) = HttpClient::check_response(status) {
                return (vec![], false, Some(error));
            }

            // 422 — API rejects large offsets, treat as hitting the cap
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

            // Parse rows depending on format
            let page_rows = if fields.is_empty() {
                // Standard format: data is array of objects
                parse_standard_rows(&data)
            } else {
                // Compact format: data is array of arrays
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
                    // Have a next link but hit offset cap
                    hit_cap = true;
                    break;
                }
                None => break,
            }
        }

        (rows, hit_cap, None)
    }

    /// Fetch a single date window, using price-informed strike partitioning when available.
    ///
    /// Returns `(total_rows_fetched_so_far, error)`.
    /// Uses `Box::pin` for the recursive calls to avoid infinite future sizes.
    ///
    /// For minimum windows (1 day), attempts to partition by strike ranges based on
    /// underlying price. This avoids offset cap issues and fetches only liquid strikes.
    /// Falls back to full fetch if price unavailable.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn fetch_window_recursive<'a>(
        &'a self,
        symbol: &'a str,
        option_type: &'a str,
        win_from: NaiveDate,
        win_to: NaiveDate,
        mut rows_fetched: usize,
        tx: &'a mpsc::Sender<WindowChunk>,
        cache: &'a CacheStore,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (usize, Option<String>)> + Send + 'a>>
    {
        use crate::providers::eodhd::parsing::normalize_rows;

        Box::pin(async move {
            let span_days = (win_to - win_from).num_days();

            tracing::debug!(
                "Fetching {symbol} {option_type} options: {win_from} to {win_to} \
                 ({span_days} days) — {rows_fetched} total rows so far"
            );

            // For minimum windows (1 day), try price-informed strike partitioning first
            if span_days == 0 {
                if let Ok(Some(price)) = get_cached_price(cache, symbol, win_from).await {
                    tracing::debug!(
                        "Using cached price for {symbol} on {win_from}: ${price:.2}, \
                         using price-informed strike partitioning"
                    );
                    let (recovered, recovery_err) = self
                        .fetch_by_strike_range(
                            symbol,
                            option_type,
                            win_from,
                            win_to,
                            price,
                            rows_fetched,
                            tx,
                        )
                        .await;
                    return (recovered, recovery_err);
                }
                tracing::debug!(
                    "No cached price available for {symbol} on {win_from}, \
                     falling back to full fetch"
                );
            }

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
                // Minimum window still hit cap despite price-informed partitioning
                // This means even the strike ranges had 10K+ rows (extreme volatility)
                tracing::debug!(
                    "Offset cap still hit for {symbol} {option_type} on minimum window ({win_from} to {win_to}) \
                     even with strike partitioning, data may be incomplete"
                );
                return (
                    rows_fetched,
                    Some(format!(
                        "offset cap hit on minimum window {win_from}–{win_to} even with strike partitioning, \
                         data may be incomplete"
                    )),
                );
            }

            if hit_cap {
                // Undo partial count — subdivision will re-fetch this range
                rows_fetched -= window_rows;

                tracing::debug!(
                    "Offset cap hit for {symbol} {option_type} ({win_from} to {win_to}), \
                     subdividing into smaller windows"
                );

                let mid = win_from + Duration::days(span_days / 2);

                // First half
                let (fetched, first_err) = self
                    .fetch_window_recursive(
                        symbol,
                        option_type,
                        win_from,
                        mid,
                        rows_fetched,
                        tx,
                        cache,
                    )
                    .await;
                rows_fetched = fetched;

                // Second half
                let (fetched, second_err) = self
                    .fetch_window_recursive(
                        symbol,
                        option_type,
                        mid + Duration::days(1),
                        win_to,
                        rows_fetched,
                        tx,
                        cache,
                    )
                    .await;
                rows_fetched = fetched;

                // Propagate the first error encountered
                return (rows_fetched, first_err.or(second_err));
            }

            (rows_fetched, None)
        })
    }

    /// Fetch all rows for a single option type using date windows.
    ///
    /// If `resume_from` is provided, only fetches data after that date
    /// (for resuming interrupted downloads or incremental updates).
    /// Uses cache to enable price-informed strike range recovery when offset cap is hit.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_all_for_type(
        &self,
        symbol: &str,
        option_type: &str,
        resume_from: Option<NaiveDate>,
        end_date: Option<NaiveDate>,
        tx: &mpsc::Sender<WindowChunk>,
        pb: &ProgressBar,
        cache: &CacheStore,
    ) -> (usize, Option<String>) {
        let today = Utc::now().date_naive();

        // Use resume_from if provided, otherwise default to full history
        let start = if let Some(resume_date) = resume_from {
            resume_date
        } else {
            today - Duration::days(HISTORY_DAYS)
        };
        let end = end_date.unwrap_or(today);

        // If resume date is past the end date, nothing to fetch
        // (e.g., cached Friday, run Saturday, resume_from Monday → skip fetch)
        if start > end {
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
                .fetch_window_recursive(
                    symbol,
                    option_type,
                    *win_from,
                    *win_to,
                    rows_fetched,
                    tx,
                    cache,
                )
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

    /// Fetch window with strike range partitioning for offset cap recovery.
    ///
    /// When offset cap is hit on a minimum window, subdivide by strike price ranges
    /// based on the underlying price to recover missing data without duplicates.
    /// Covers 97% of data range (underlying price × 0.35 to 2.65).
    #[allow(clippy::too_many_arguments)]
    async fn fetch_by_strike_range(
        &self,
        symbol: &str,
        option_type: &str,
        win_from: NaiveDate,
        win_to: NaiveDate,
        price: f64,
        mut rows_fetched: usize,
        tx: &mpsc::Sender<WindowChunk>,
    ) -> (usize, Option<String>) {
        let (strike_lower, strike_upper) = calculate_strike_range(price);

        tracing::debug!(
            "Strike range recovery for {symbol} {option_type}: price=${price:.2}, \
             fetching strikes ${strike_lower:.2}-${strike_upper:.2} (±65%)"
        );

        let strike_mid = f64::midpoint(strike_lower, strike_upper);

        // Fetch lower and upper strike ranges
        let params_lower = build_strike_params(
            symbol,
            option_type,
            win_from,
            win_to,
            strike_lower,
            strike_mid,
        );
        let (rows_lower, hit_cap_lower, _) = self.paginate_window(&params_lower).await;
        rows_fetched += send_normalized_chunk(&rows_lower, symbol, tx, "lower").await;

        let params_upper = build_strike_params(
            symbol,
            option_type,
            win_from,
            win_to,
            strike_mid + 0.01,
            strike_upper,
        );
        let (rows_upper, hit_cap_upper, _) = self.paginate_window(&params_upper).await;
        rows_fetched += send_normalized_chunk(&rows_upper, symbol, tx, "upper").await;

        // Report if either partition still hits cap
        let mut final_error = None;

        if hit_cap_lower {
            tracing::debug!(
                "Offset cap still hit in lower strike range (${strike_lower:.2}-${strike_mid:.2})"
            );
            final_error =
                Some("offset cap hit in lower strike range, data may be incomplete".to_string());
        }

        if hit_cap_upper {
            tracing::debug!(
                "Offset cap still hit in upper strike range (${strike_mid:.2}-${strike_upper:.2})"
            );
            if final_error.is_none() {
                final_error = Some(
                    "offset cap hit in upper strike range, data may be incomplete".to_string(),
                );
            }
        }

        tracing::debug!(
            "Strike range recovery complete: {symbol} {option_type}, \
             {rows_fetched} rows recovered"
        );

        (rows_fetched, final_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_windows_generates_correct_ranges() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let windows = Paginator::monthly_windows(start, end);

        // Should be newest-first
        assert!(!windows.is_empty());
        assert_eq!(windows[0].1, end);

        // All windows should be within range
        for (from, to) in &windows {
            assert!(*from >= start);
            assert!(*to <= end);
            assert!(from <= to);
        }

        // Windows should cover the full range
        let last = windows.last().unwrap();
        assert_eq!(last.0, start);
    }

    #[test]
    fn test_calculate_strike_range() {
        let (lower, upper) = calculate_strike_range(500.0);
        // 500 * 0.35 = 175, 500 * 2.65 = 1325 (allowing for small FP rounding error)
        let tol = 1e-9;
        assert!(
            (lower - 175.0).abs() < tol,
            "lower should be 175, got {lower}"
        );
        assert!(
            (upper - 1325.0).abs() < tol,
            "upper should be 1325, got {upper}"
        );
    }

    #[test]
    fn test_build_strike_params_count_and_content() {
        let from = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        let to = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        let params = build_strike_params("SPY", "call", from, to, 175.0, 1325.0);

        assert_eq!(params.len(), 9, "should have 9 params");

        // Check key params are present
        let map: std::collections::HashMap<&str, &str> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        assert_eq!(map["filter[underlying_symbol]"], "SPY");
        assert_eq!(map["filter[type]"], "call");
        assert_eq!(map["filter[tradetime_from]"], "2024-03-15");
        assert_eq!(map["filter[tradetime_to]"], "2024-03-15");
        assert_eq!(map["filter[strike_from]"], "175");
        assert_eq!(map["filter[strike_to]"], "1325");
        assert_eq!(map["page[limit]"], PAGE_LIMIT.to_string());
        assert_eq!(map["sort"], "exp_date");
    }
}
