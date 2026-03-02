//! HTTP request handling and rate limiting for EODHD API.

use anyhow::bail;
use reqwest::Client;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::sleep;

// Constants for HTTP handling
pub const TIMEOUT_SECS: u64 = 60;
pub const MAX_RETRIES: u32 = 5;
pub const MIN_REQUEST_INTERVAL_MS: u64 = 100;
pub const RATE_LIMIT_SLOW_THRESHOLD: u32 = 50;

/// HTTP client and rate limiting state for EODHD API requests.
pub struct HttpClient {
    pub client: Client,
    pub api_key: String,
    pub last_request_time: Mutex<Instant>,
    pub request_count: AtomicU32,
}

impl HttpClient {
    /// Create a new HTTP client with rate limiting.
    pub fn new(api_key: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to build HTTP client with timeout: {e}, using default");
                Client::new()
            });

        Self {
            client,
            api_key,
            last_request_time: Mutex::new(Instant::now()),
            request_count: AtomicU32::new(0),
        }
    }

    /// Rate-limited GET with retry on transient errors and backoff.
    pub async fn throttled_get(
        &self,
        url: &str,
        params: &[(String, String)],
    ) -> anyhow::Result<reqwest::Response> {
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
    pub fn check_response(status: u16) -> Option<String> {
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
}
