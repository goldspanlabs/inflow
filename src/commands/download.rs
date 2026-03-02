//! Download command implementation.

use crate::cache::CacheStore;
use crate::cli::DownloadTarget;
use crate::error::InflowError;
use crate::pipeline::{DownloadParams, DownloadResult, Pipeline};
use crate::providers::{build_providers, filter_providers_by_category};
use crate::utils::download_results_table;
use crate::Config;
use anyhow::Result;
use std::sync::Arc;

/// Execute the download command.
///
/// Builds a [`Pipeline`] from the configured providers and runs it.
/// Returns [`InflowError::PartialFailure`] if some symbols fail,
/// or [`InflowError::Config`] if no providers are configured for the requested category.
pub async fn execute(
    config: &Config,
    target: DownloadTarget,
) -> Result<Vec<DownloadResult>, InflowError> {
    let cache = Arc::new(CacheStore::new(config.data_root.clone()));
    let providers = build_providers(config);

    if providers.is_empty() {
        return Err(InflowError::Config(
            "No providers enabled. Set EODHD_API_KEY to enable EODHD provider.".to_string(),
        ));
    }

    let (filtered_providers, symbols, params, concurrency) = match target {
        DownloadTarget::Options {
            symbols,
            from,
            concurrency,
        } => {
            let opts_providers = filter_providers_by_category(&providers, "options");

            if opts_providers.is_empty() {
                return Err(InflowError::Config(
                    "No options providers enabled. Set EODHD_API_KEY to enable EODHD.".to_string(),
                ));
            }

            let params = DownloadParams {
                from_date: from,
                period: "1y".to_string(),
            };

            (opts_providers, symbols, params, concurrency)
        }

        DownloadTarget::Prices {
            symbols,
            period,
            concurrency,
        } => {
            let prices_providers = filter_providers_by_category(&providers, "prices");

            if prices_providers.is_empty() {
                return Err(InflowError::Config(
                    "No prices providers available.".to_string(),
                ));
            }

            let params = DownloadParams {
                from_date: None,
                period,
            };

            (prices_providers, symbols, params, concurrency)
        }

        DownloadTarget::All {
            symbols,
            from,
            period,
            concurrency,
        } => {
            let params = DownloadParams {
                from_date: from,
                period,
            };

            (providers, symbols, params, concurrency)
        }
    };

    let pipeline = Pipeline {
        providers: filtered_providers,
        cache,
        symbols,
        params,
        concurrency,
    };

    let results = pipeline.run().await.map_err(|e| {
        InflowError::Other(anyhow::anyhow!("Pipeline execution failed: {}", e))
    })?;

    // Print results table
    print_results(&results);

    // Check for partial failures
    let failed_count = results.iter().filter(|r| !r.is_success()).count();
    if failed_count > 0 && failed_count < results.len() {
        return Err(InflowError::PartialFailure(format!(
            "{} of {} symbols failed",
            failed_count,
            results.len()
        )));
    } else if failed_count == results.len() {
        return Err(InflowError::PartialFailure(
            "All downloads failed".to_string(),
        ));
    }

    Ok(results)
}

fn print_results(results: &[DownloadResult]) {
    let table_data: Vec<_> = results
        .iter()
        .map(|result| {
            let date_range = result
                .date_range
                .map(|(min, max)| format!("{} → {}", min, max))
                .unwrap_or_default();

            let status = if result.is_success() {
                "✓".to_string()
            } else {
                format!("✗ ({})", result.errors.join("; "))
            };

            (
                result.symbol.clone(),
                result.provider.clone(),
                result.new_rows,
                result.total_rows,
                date_range,
                status,
            )
        })
        .collect();

    let table = download_results_table(&table_data);
    println!("\n{table}\n");
}
