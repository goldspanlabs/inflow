//! EODHD API response types.

use serde::Deserialize;

/// Top-level API response wrapper.
#[derive(Debug, Deserialize)]
pub struct ApiResponse {
    /// Metadata about the response (field names in compact mode).
    pub meta: Option<ApiMeta>,
    /// The actual data rows.
    pub data: Option<serde_json::Value>,
    /// Pagination links.
    pub links: Option<ApiLinks>,
}

/// Metadata about response fields (used in compact mode).
#[derive(Debug, Deserialize)]
pub struct ApiMeta {
    /// List of field names for compact format data.
    pub fields: Option<Vec<String>>,
}

/// Pagination information from API.
#[derive(Debug, Deserialize)]
pub struct ApiLinks {
    /// URL for next page, if available.
    pub next: Option<String>,
}
