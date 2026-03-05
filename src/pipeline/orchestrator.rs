//! Pipeline orchestration using a producer–consumer model.
//!
//! [`Pipeline::run`] spawns one producer task per (provider, symbol) pair,
//! limited by a [`Semaphore`] for concurrency control. All chunks flow via
//! an mpsc channel to a single consumer ([`run_writer`]) that accumulates
//! options windows and writes prices immediately.

use crate::cache::CacheStore;
use crate::pipeline::consumer::run_writer;
use crate::pipeline::producer::run_symbol_worker;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::providers::DataProvider;
use crate::utils::{collect_blocking, extract_date_range, OPTIONS_DATE_COLUMN, PRICES_DATE_COLUMN};
use anyhow::Result;
use indicatif::MultiProgress;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

/// Download pipeline configuration.
pub struct Pipeline {
    /// List of enabled providers.
    pub providers: Vec<Arc<dyn DataProvider>>,

    /// Cache store.
    pub cache: Arc<CacheStore>,

    /// Symbols to download.
    pub symbols: Vec<String>,

    /// Download parameters.
    pub params: DownloadParams,

    /// Concurrency limit for downloads.
    pub concurrency: usize,
}

impl Pipeline {
    /// Run the pipeline.
    ///
    /// Spawns producer workers for each provider-symbol pair, runs the consumer
    /// to merge and write data, and returns aggregate results.
    pub async fn run(self) -> Result<Vec<DownloadResult>> {
        let shutdown = CancellationToken::new();
        let mp = Arc::new(MultiProgress::new());

        // Channel for chunks (buffered to prevent blocking)
        let (tx, rx) = mpsc::channel::<WindowChunk>(self.concurrency * 4);

        // Semaphore to limit concurrent downloads
        let semaphore = Arc::new(Semaphore::new(self.concurrency));

        // Spawn the consumer task
        let consumer_cache = Arc::clone(&self.cache);
        let consumer_handle = tokio::spawn(async move { run_writer(consumer_cache, rx).await });

        // Spawn producer tasks
        let mut producer_handles = Vec::new();

        for provider in &self.providers {
            for symbol in &self.symbols {
                let symbol_clone = symbol.clone();
                let provider_clone = Arc::clone(provider);
                let params_clone = self.params.clone();
                let cache_clone = Arc::clone(&self.cache);
                let semaphore_clone = Arc::clone(&semaphore);
                let tx_clone = tx.clone();
                let shutdown_clone = shutdown.clone();

                let mp_clone = Arc::clone(&mp);
                let handle = tokio::spawn(async move {
                    run_symbol_worker(
                        symbol_clone,
                        provider_clone,
                        params_clone,
                        cache_clone,
                        semaphore_clone,
                        tx_clone,
                        shutdown_clone,
                        mp_clone,
                    )
                    .await
                });

                producer_handles.push(handle);
            }
        }

        // Drop the original sender so consumer knows when producers are done
        drop(tx);

        // Wait for all producers to complete
        let mut results = Vec::new();
        for handle in producer_handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }

        // Wait for consumer to finish processing remaining chunks
        let writer_errors = match consumer_handle.await {
            Ok(errs) => errs,
            Err(e) => {
                tracing::error!("Consumer task panicked: {e}");
                anyhow::bail!("Consumer task panicked: {e}");
            }
        };
        if !writer_errors.is_empty() {
            tracing::warn!("Writer errors: {:?}", writer_errors);
        }

        // Now that the consumer has finished writing, populate total_rows and date_range
        for result in &mut results {
            let (date_col, path) = if result.category == "options" {
                (OPTIONS_DATE_COLUMN, self.cache.options_path(&result.symbol))
            } else {
                (PRICES_DATE_COLUMN, self.cache.prices_path(&result.symbol))
            };
            if let Ok(path) = path {
                if let Ok(Some(lf)) = self.cache.read_parquet(&path).await {
                    if let Ok(df) = collect_blocking(lf).await {
                        result.total_rows = df.height();
                        result.date_range = extract_date_range(&df, date_col);
                    }
                }
            }
        }

        Ok(results)
    }
}
