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
            to,
            concurrency,
        } => {
            let opts_providers = filter_providers_by_category(&providers, "options");

            if opts_providers.is_empty() {
                return Err(InflowError::Config(
                    "No options providers enabled. Set EODHD_API_KEY to enable EODHD.".to_string(),
                ));
            }

            validate_date_range(from, to)?;

            let params = DownloadParams {
                period: "1y".to_string(),
                from_date: from,
                to_date: to,
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
                period,
                from_date: None,
                to_date: None,
            };

            (prices_providers, symbols, params, concurrency)
        }

        DownloadTarget::All {
            symbols,
            from,
            to,
            period,
            concurrency,
        } => {
            validate_date_range(from, to)?;

            let params = DownloadParams {
                period,
                from_date: from,
                to_date: to,
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

    let results = pipeline
        .run()
        .await
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Pipeline execution failed: {e}")))?;

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

/// Validate `--from` / `--to` date range flags.
fn validate_date_range(
    from: Option<chrono::NaiveDate>,
    to: Option<chrono::NaiveDate>,
) -> Result<(), InflowError> {
    if to.is_some() && from.is_none() {
        return Err(InflowError::Config(
            "--to requires --from to be specified".to_string(),
        ));
    }
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(InflowError::Config(format!(
                "--from ({f}) must be on or before --to ({t})"
            )));
        }
    }
    Ok(())
}

fn print_results(results: &[DownloadResult]) {
    let table_data: Vec<_> = results
        .iter()
        .map(|result| {
            let date_range = result
                .date_range
                .map(|(min, max)| format!("{min} → {max}"))
                .unwrap_or_default();

            let status = if !result.errors.is_empty() {
                format!("✗ ({})", result.errors.join("; "))
            } else if !result.warnings.is_empty() {
                format!("⚠ ({})", result.warnings.join("; "))
            } else {
                "✓".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn validate_both_none() {
        assert!(validate_date_range(None, None).is_ok());
    }

    #[test]
    fn validate_from_only() {
        let from = NaiveDate::from_ymd_opt(2024, 1, 1);
        assert!(validate_date_range(from, None).is_ok());
    }

    #[test]
    fn validate_to_without_from() {
        let to = NaiveDate::from_ymd_opt(2024, 6, 1);
        assert!(validate_date_range(None, to).is_err());
    }

    #[test]
    fn validate_from_before_to() {
        let from = NaiveDate::from_ymd_opt(2024, 1, 1);
        let to = NaiveDate::from_ymd_opt(2024, 6, 1);
        assert!(validate_date_range(from, to).is_ok());
    }

    #[test]
    fn validate_from_after_to() {
        let from = NaiveDate::from_ymd_opt(2024, 6, 1);
        let to = NaiveDate::from_ymd_opt(2024, 1, 1);
        assert!(validate_date_range(from, to).is_err());
    }

    #[test]
    fn validate_from_equals_to() {
        let d = NaiveDate::from_ymd_opt(2024, 3, 15);
        assert!(validate_date_range(d, d).is_ok());
    }
}
