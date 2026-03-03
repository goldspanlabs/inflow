//! Consumer writer task for merging data into cache.

use crate::cache::CacheStore;
use crate::pipeline::types::WindowChunk;
use crate::utils::{OPTIONS_DATE_COLUMN, OPTIONS_DEDUP_COLS};
use anyhow::{Context, Result};
use polars::prelude::*;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Run the writer consumer task.
///
/// Receives chunks from all providers, merges them with existing cache, and writes atomically.
/// Returns a list of any errors encountered (non-fatal).
pub async fn run_writer(
    cache: Arc<CacheStore>,
    mut rx: mpsc::Receiver<WindowChunk>,
) -> Vec<String> {
    let mut accumulated: HashMap<String, Vec<DataFrame>> = HashMap::new();
    let mut errors = Vec::new();

    while let Some(chunk) = rx.recv().await {
        match chunk {
            WindowChunk::OptionsWindow { symbol, df } => {
                accumulated.entry(symbol).or_default().push(df);
            }
            WindowChunk::PricesComplete { symbol, df } => {
                // Prices are overwritten (single chunk per symbol from Yahoo)
                if let Err(e) = write_prices(&cache, &symbol, df).await {
                    errors.push(format!("Failed to write prices for {symbol}: {e}"));
                }
            }
        }
    }

    // Process accumulated options data
    for (symbol, chunks) in accumulated {
        if let Err(e) = write_options(&cache, &symbol, chunks).await {
            errors.push(format!("Failed to write options for {symbol}: {e}"));
        }
    }

    errors
}

/// Write prices data (simple overwrite).
async fn write_prices(cache: &CacheStore, symbol: &str, df: DataFrame) -> Result<()> {
    let path = cache.prices_path(symbol)?;
    let mut df = df;
    cache.atomic_write(&path, &mut df).await?;
    Ok(())
}

/// Write options data (merge + deduplicate).
async fn write_options(cache: &CacheStore, symbol: &str, chunks: Vec<DataFrame>) -> Result<()> {
    let path = cache.options_path(symbol)?;

    // Read existing cache
    let cached = cache.read_parquet(&path).await?;
    let existing_df = if let Some(lf) = cached {
        tokio::task::spawn_blocking(move || lf.collect().ok())
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Merge, deduplicate, and sort (all blocking Polars work)
    let mut merged_df = tokio::task::spawn_blocking(move || -> Result<DataFrame> {
        let mut df = merge_options_dataframes(existing_df, chunks)?;
        deduplicate_options(&mut df)?;
        sort_by_quote_date(&mut df)?;
        Ok(df)
    })
    .await
    .context("Spawn blocking panicked")??;

    cache.atomic_write(&path, &mut merged_df).await?;
    Ok(())
}

/// Merge existing options `DataFrame` with new chunks.
///
/// If existing data is present, concatenates it with all chunks.
/// Otherwise, concatenates chunks together.
fn merge_options_dataframes(
    existing: Option<DataFrame>,
    chunks: Vec<DataFrame>,
) -> Result<DataFrame> {
    let all_dfs: Vec<LazyFrame> = existing
        .into_iter()
        .chain(chunks)
        .map(DataFrame::lazy)
        .collect();

    if all_dfs.is_empty() {
        return Ok(DataFrame::empty());
    }

    concat(
        all_dfs,
        UnionArgs {
            rechunk: true,
            to_supertypes: true,
            diagonal: true,
            ..Default::default()
        },
    )?
    .collect()
    .context("Failed to collect merged dataframe")
}

/// Deduplicate options `DataFrame` on key columns.
///
/// Keeps the last occurrence of each unique row based on
/// [`OPTIONS_DEDUP_COLS`](crate::utils::OPTIONS_DEDUP_COLS).
fn deduplicate_options(df: &mut DataFrame) -> Result<()> {
    let available: Vec<String> = OPTIONS_DEDUP_COLS
        .iter()
        .filter(|c| df.schema().contains(c))
        .map(ToString::to_string)
        .collect();

    if !available.is_empty() {
        *df = df.unique::<String, String>(Some(&available), UniqueKeepStrategy::Last, None)?;
    }

    Ok(())
}

/// Sort options `DataFrame` by `quote_date` if the column exists.
fn sort_by_quote_date(df: &mut DataFrame) -> Result<()> {
    if df.schema().contains(OPTIONS_DATE_COLUMN) {
        let temp = mem::take(df);
        *df = temp
            .lazy()
            .sort([OPTIONS_DATE_COLUMN], SortMultipleOptions::default())
            .collect()
            .context("Failed to sort dataframe by quote_date")?;
    }

    Ok(())
}
