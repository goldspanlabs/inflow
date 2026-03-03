//! Interactive list command — browse available underlying symbols and trigger a download.

use crate::cli::DownloadTarget;
use crate::config::Config;
use crate::error::InflowError;
use crate::providers::eodhd::types::ApiResponse;
use dialoguer::{theme::ColorfulTheme, Select};

const DATA_TYPE_OPTIONS: [&str; 3] = ["Options", "Prices", "Both"];
const DEFAULT_CONCURRENCY: usize = 4;
const DEFAULT_PERIOD: &str = "5y";

/// Fetch the list of underlying symbols available via the EODHD marketplace.
async fn fetch_underlying_symbols(api_key: &str) -> Result<Vec<String>, InflowError> {
    let url = format!(
        "https://eodhd.com/api/mp/unicornbay/options/underlying-symbols?api_token={api_key}"
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Failed to reach EODHD API: {e}")))?;

    if !response.status().is_success() {
        return Err(InflowError::Other(anyhow::anyhow!(
            "EODHD API returned status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        )));
    }

    let api_resp: ApiResponse = response
        .json()
        .await
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Failed to parse symbol list: {e}")))?;

    let symbols: Vec<String> = api_resp
        .data
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    Ok(symbols)
}

/// Execute the `list` command.
///
/// Fetches available underlying symbols from the EODHD marketplace, presents an
/// interactive paginated list for the user to browse using arrow keys, then prompts
/// for the data type to download before invoking the [`download`](super::download) command.
pub async fn execute(config: &Config) -> Result<(), InflowError> {
    let api_key = config.eodhd_api_key.as_deref().ok_or_else(|| {
        InflowError::Config(
            "EODHD_API_KEY is required for the list command. \
             Set it in your environment or ~/.env file."
                .to_string(),
        )
    })?;

    println!("Fetching available underlying symbols from EODHD…");
    let symbols = fetch_underlying_symbols(api_key).await?;

    if symbols.is_empty() {
        println!("No underlying symbols returned by the EODHD API.");
        return Ok(());
    }

    // --- symbol selection ---
    let symbol_idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select an underlying symbol (↑/↓ to navigate, Enter to confirm)")
        .items(&symbols)
        .default(0)
        .interact()
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Selection cancelled: {e}")))?;

    let selected_symbol = symbols[symbol_idx].clone();

    // --- data-type selection ---
    let data_idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What data would you like to download?")
        .items(&DATA_TYPE_OPTIONS)
        .default(0)
        .interact()
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Selection cancelled: {e}")))?;

    // --- build DownloadTarget and delegate to the download command ---
    let target = match data_idx {
        0 => DownloadTarget::Options {
            symbols: vec![selected_symbol],
            from: None,
            to: None,
            concurrency: DEFAULT_CONCURRENCY,
        },
        1 => DownloadTarget::Prices {
            symbols: vec![selected_symbol],
            period: DEFAULT_PERIOD.to_string(),
            concurrency: DEFAULT_CONCURRENCY,
        },
        _ => DownloadTarget::All {
            symbols: vec![selected_symbol],
            from: None,
            to: None,
            period: DEFAULT_PERIOD.to_string(),
            concurrency: DEFAULT_CONCURRENCY,
        },
    };

    super::download::execute(config, target).await.map(|_| ())
}
