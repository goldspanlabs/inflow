//! Window-based pagination and recursive fetching for EODHD API.

use crate::pipeline::types::WindowChunk;
use crate::utils::{parse_compact_rows, parse_standard_rows};
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

/// Pagination helper for EODHD API requests.
pub struct Paginator {
    pub http: HttpClient,
}

impl Paginator {
    /// Create a new paginator.
    pub fn new(api_key: String) -> Self {
        Self {
            http: HttpClient::new(api_key),
        }
    }

    /// Generate ~30-day windows, newest first.
    pub fn monthly_windows(start: NaiveDate, end: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
        let mut windows = Vec::new();
        let mut cur = end;
        while cur > start {
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
        let mut url = format!("{BASE_URL}/options/eod");
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

    /// Fetch a single date window, subdividing if the offset cap is hit.
    ///
    /// Returns `(total_rows_fetched_so_far, error)`.
    /// Uses `Box::pin` for the recursive calls to avoid infinite future sizes.
    pub fn fetch_window_recursive<'a>(
        &'a self,
        symbol: &'a str,
        option_type: &'a str,
        win_from: NaiveDate,
        win_to: NaiveDate,
        mut rows_fetched: usize,
        tx: &'a mpsc::Sender<WindowChunk>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (usize, Option<String>)> + Send + 'a>>
    {
        use crate::providers::eodhd::parsing::normalize_rows;

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
                // Undo partial count — subdivision will re-fetch this range
                rows_fetched -= window_rows;

                tracing::warn!(
                    "Offset cap hit for {symbol} {option_type} ({win_from} to {win_to}), \
                     subdividing into smaller windows"
                );

                let mid = win_from + Duration::days(span_days / 2);

                // First half
                let (fetched, first_err) = self
                    .fetch_window_recursive(symbol, option_type, win_from, mid, rows_fetched, tx)
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
    pub async fn fetch_all_for_type(
        &self,
        symbol: &str,
        option_type: &str,
        _resume_from: Option<NaiveDate>,
        tx: &mpsc::Sender<WindowChunk>,
        pb: &ProgressBar,
    ) -> (usize, Option<String>) {
        let today = Utc::now().date_naive();
        let start = today - Duration::days(HISTORY_DAYS);
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
}
