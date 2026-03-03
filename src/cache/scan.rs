//! Cache file scanning and inspection.

use crate::utils::anyvalue_to_naive_date;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use polars::prelude::*;
use std::path::Path;

/// Information about a cached Parquet file.
#[derive(Debug, Clone)]
pub struct CacheFileInfo {
    /// File size in bytes.
    pub size_bytes: u64,

    /// Number of rows in the file.
    pub row_count: usize,

    /// Earliest date in the `date` or `quote_date` column (if available).
    pub date_min: Option<NaiveDate>,

    /// Latest date in the `date` or `quote_date` column (if available).
    pub date_max: Option<NaiveDate>,
}

/// Scan a Parquet file and extract metadata.
///
/// Reads the Parquet file to get row count and date range (from `quote_date` or `date` column).
pub async fn scan_file(path: &Path, date_col: &str) -> Result<CacheFileInfo> {
    // Get file size
    let size_bytes = tokio::fs::metadata(path)
        .await
        .context("Failed to read file metadata")?
        .len();

    // Read Parquet to get row count and date range
    let path_owned = path.to_path_buf();
    let date_col_owned = date_col.to_string();

    let (row_count, date_min, date_max) = tokio::task::spawn_blocking(move || {
        let lf = match LazyFrame::scan_parquet(
            path_owned.to_string_lossy().as_ref().into(),
            ScanArgsParquet::default(),
        ) {
            Ok(lf) => lf,
            Err(e) => {
                tracing::warn!("Failed to scan parquet {}: {e}", path_owned.display());
                return (0, None, None);
            }
        };

        // Collect once and reuse for both row count and date extraction
        let df = match lf.collect() {
            Ok(df) => df,
            Err(e) => {
                tracing::warn!("Failed to collect parquet {}: {e}", path_owned.display());
                return (0, None, None);
            }
        };

        let row_count = df.height();

        let col_name = if df.schema().contains(&date_col_owned) {
            &date_col_owned
        } else if df.schema().contains("date") {
            "date"
        } else if df.schema().contains("quote_date") {
            "quote_date"
        } else {
            return (row_count, None, None);
        };

        let date_min = df
            .column(col_name)
            .ok()
            .and_then(|col| col.min_reduce().ok())
            .and_then(|s| anyvalue_to_naive_date(s.value()));

        let date_max = df
            .column(col_name)
            .ok()
            .and_then(|col| col.max_reduce().ok())
            .and_then(|s| anyvalue_to_naive_date(s.value()));

        (row_count, date_min, date_max)
    })
    .await
    .context("Scan task panicked")?;

    Ok(CacheFileInfo {
        size_bytes,
        row_count,
        date_min,
        date_max,
    })
}
