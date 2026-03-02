//! Config command implementation.

use crate::Config;
use console::style;

/// Execute the config command.
pub fn execute(config: &Config) {
    println!("\n{}\n", style("Inflow Configuration:").bold().green());

    println!(
        "  {} {}",
        style("DATA_ROOT:").bold(),
        config.data_root.display()
    );

    let api_key_status = if config.eodhd_api_key.is_some() {
        style("✓ configured").green()
    } else {
        style("✗ not set").red()
    };
    println!(
        "  {} {}",
        style("EODHD_API_KEY:").bold(),
        api_key_status
    );

    println!(
        "  {} {} req/sec",
        style("EODHD_RATE_LIMIT:").bold(),
        config.eodhd_rate_limit
    );

    println!();
}
