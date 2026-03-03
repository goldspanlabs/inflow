//! Interactive list command — browse available underlying symbols and trigger a download.

use crate::cli::DownloadTarget;
use crate::config::Config;
use crate::error::InflowError;
use crate::providers::eodhd::types::ApiResponse;
use dialoguer::{theme::ColorfulTheme, Input, MultiSelect, Select};

use crate::cli::{DEFAULT_CONCURRENCY, DEFAULT_PERIOD};

const DATA_TYPE_OPTIONS: [&str; 3] = ["Options", "Prices", "Both"];
const PAGE_SIZE: usize = 25;

/// Fetch the list of underlying symbols available via the EODHD marketplace.
async fn fetch_underlying_symbols(api_key: &str) -> Result<Vec<String>, InflowError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            crate::providers::eodhd::http::TIMEOUT_SECS,
        ))
        .build()
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Failed to build HTTP client: {e}")))?;

    let response = client
        .get("https://eodhd.com/api/mp/unicornbay/options/underlying-symbols")
        .query(&[("api_token", api_key)])
        .send()
        .await
        .map_err(|_| InflowError::Other(anyhow::anyhow!("Failed to reach EODHD API")))?;

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
pub async fn execute(config: &Config, search: Option<&str>) -> Result<(), InflowError> {
    let api_key = config.eodhd_api_key.as_deref().ok_or_else(|| {
        InflowError::Config(
            "EODHD_API_KEY is required for the list command. \
             Set it in your environment or ~/.env file."
                .to_string(),
        )
    })?;

    println!("Fetching available underlying symbols from EODHD…");
    let all_symbols = fetch_underlying_symbols(api_key).await?;

    if all_symbols.is_empty() {
        println!("No underlying symbols returned by the EODHD API.");
        return Ok(());
    }

    let mut current_filter: Option<String> = search.map(ToString::to_string);

    // --- symbol selection loop (allows re-searching) ---
    let selected_symbols: Vec<String> = loop {
        let filtered: Vec<&String> = if let Some(ref query) = current_filter {
            let query_upper = query.to_uppercase();
            all_symbols
                .iter()
                .filter(|s| s.to_uppercase().contains(&query_upper))
                .collect()
        } else {
            all_symbols.iter().collect()
        };

        if filtered.is_empty() {
            println!("No symbols matched the search filter.");
            current_filter = None;
            continue;
        }

        if let Some(ref query) = current_filter {
            println!("Showing {} result(s) for \"{query}\"", filtered.len());
        }

        let items: Vec<String> = filtered.iter().map(|s| (*s).clone()).collect();

        let selections = MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt("Space to toggle, Enter to confirm. Enter with no selection to search")
            .items(&items)
            .max_length(PAGE_SIZE)
            .interact()
            .map_err(|e| InflowError::Other(anyhow::anyhow!("Symbol selection failed: {e}")))?;

        if selections.is_empty() {
            let query: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Enter search term (empty to show all)")
                .allow_empty(true)
                .interact_text()
                .map_err(|e| InflowError::Other(anyhow::anyhow!("Search input failed: {e}")))?;
            current_filter = if query.is_empty() { None } else { Some(query) };
            continue;
        }

        break selections.into_iter().map(|i| items[i].clone()).collect();
    };

    println!(
        "Selected {} symbol(s): {}",
        selected_symbols.len(),
        selected_symbols.join(", ")
    );

    // --- data-type selection ---
    let data_idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What data would you like to download?")
        .items(&DATA_TYPE_OPTIONS)
        .default(0)
        .interact()
        .map_err(|e| InflowError::Other(anyhow::anyhow!("Data type selection failed: {e}")))?;

    // --- build DownloadTarget and delegate to the download command ---
    let target = match data_idx {
        0 => DownloadTarget::Options {
            symbols: selected_symbols,
            from: None,
            to: None,
            concurrency: DEFAULT_CONCURRENCY,
        },
        1 => DownloadTarget::Prices {
            symbols: selected_symbols,
            period: DEFAULT_PERIOD.to_string(),
            concurrency: DEFAULT_CONCURRENCY,
        },
        _ => DownloadTarget::All {
            symbols: selected_symbols,
            from: None,
            to: None,
            period: DEFAULT_PERIOD.to_string(),
            concurrency: DEFAULT_CONCURRENCY,
        },
    };

    super::download::execute(config, target).await.map(|_| ())
}
