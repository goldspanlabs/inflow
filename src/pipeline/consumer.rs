//! Consumer writer task for merging data into cache.

use crate::cache::CacheStore;
use crate::pipeline::types::WindowChunk;
use anyhow::{Context, Result};
use polars::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Options deduplication columns (must match optopy-mcp).
const DEDUP_COLS: &[&str] = &[
    "quote_date",
    "expiration",
    "strike",
    "option_type",
    "expiration_type",
];

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
                accumulated.entry(symbol).or_insert_with(Vec::new).push(df);
            }
            WindowChunk::PricesComplete { symbol, df } => {
                // Prices are overwritten (single chunk per symbol from Yahoo)
                match write_prices(&cache, &symbol, df).await {
                    Ok(_) => {}
                    Err(e) => errors.push(format!("Failed to write prices for {symbol}: {e}")),
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
    let existing_df = cache.read_parquet(&path).await?.and_then(|lf| lf.collect().ok());

    // Merge chunks together
    let mut merged_df = if let Some(existing) = existing_df {
        let mut all_dfs = vec![existing.lazy()];
        for chunk in chunks {
            all_dfs.push(chunk.lazy());
        }

        concat(all_dfs, UnionArgs {
            rechunk: true,
            to_supertypes: true,
            diagonal: true,
            ..Default::default()
        })?
        .collect()?
    } else {
        if chunks.is_empty() {
            return Ok(());
        }

        let all_dfs: Vec<_> = chunks.into_iter().map(|df| df.lazy()).collect();
        if all_dfs.is_empty() {
            return Ok(());
        }

        concat(all_dfs, UnionArgs {
            rechunk: true,
            to_supertypes: true,
            diagonal: true,
            ..Default::default()
        })?
        .collect()?
    };

    // Deduplicate
    let available: Vec<String> = DEDUP_COLS
        .iter()
        .filter(|c| merged_df.schema().contains(c))
        .map(|c| c.to_string())
        .collect();

    if !available.is_empty() {
        merged_df = merged_df.unique::<String, String>(Some(&available), UniqueKeepStrategy::Last, None)?;
    }

    // Sort by quote_date if present
    if merged_df.schema().contains("quote_date") {
        merged_df = merged_df
            .lazy()
            .sort(["quote_date"], SortMultipleOptions::default())
            .collect()?;
    }

    cache.atomic_write(&path, &mut merged_df).await?;
    Ok(())
}
