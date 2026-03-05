//! Producer worker for concurrent downloads.

use crate::cache::CacheStore;
use crate::pipeline::types::{DownloadParams, DownloadResult, WindowChunk};
use crate::providers::DataProvider;
use indicatif::MultiProgress;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

/// Run a producer worker for a single symbol.
///
/// Acquires the semaphore, calls the provider's download method, and returns the result.
#[allow(clippy::too_many_arguments)]
pub async fn run_symbol_worker(
    symbol: String,
    provider: Arc<dyn DataProvider>,
    params: DownloadParams,
    cache: Arc<CacheStore>,
    semaphore: Arc<Semaphore>,
    tx: mpsc::Sender<WindowChunk>,
    shutdown: CancellationToken,
    mp: Arc<MultiProgress>,
) -> DownloadResult {
    // Acquire semaphore slot
    let Ok(_permit) = semaphore.acquire().await else {
        return DownloadResult::success(symbol, provider.name().to_string(), provider.category().to_string(), 0, 0, None)
            .with_errors(vec!["Semaphore acquisition failed".to_string()]);
    };

    match provider
        .download(&symbol, &params, &cache, tx, shutdown, &mp)
        .await
    {
        Ok(result) => result,
        Err(e) => DownloadResult::success(symbol, provider.name().to_string(), provider.category().to_string(), 0, 0, None)
            .with_errors(vec![e.to_string()]),
    }
}
