//! Status command implementation.

use crate::cache::{scan_file, CacheStore};
use anyhow::Result;
use comfy_table::Table;
use std::path::Path;

/// Execute the status command.
pub async fn execute(cache: &CacheStore) -> Result<()> {
    // Scan options and prices directories
    let options_symbols = cache.list_symbols("options").unwrap_or_default();
    let prices_symbols = cache.list_symbols("prices").unwrap_or_default();

    if options_symbols.is_empty() && prices_symbols.is_empty() {
        println!("\nCache is empty. Run 'inflow download' to populate it.\n");
        return Ok(());
    }

    // Options table
    if !options_symbols.is_empty() {
        println!("\n📊 Options Cache:\n");
        print_category_table("options", &options_symbols, cache).await?;
    }

    // Prices table
    if !prices_symbols.is_empty() {
        println!("\n💹 Prices Cache:\n");
        print_category_table("prices", &prices_symbols, cache).await?;
    }

    println!();
    Ok(())
}

async fn print_category_table(
    category: &str,
    symbols: &[String],
    cache: &CacheStore,
) -> Result<()> {
    let mut table = Table::new();
    table.set_header(vec!["Symbol", "Rows", "Size (MB)", "Date Range"]);

    let date_col = if category == "options" {
        "quote_date"
    } else {
        "date"
    };

    for symbol in symbols {
        let path_result = if category == "options" {
            cache.options_path(symbol)
        } else {
            cache.prices_path(symbol)
        };

        if let Ok(path) = path_result {
            if let Ok(info) = scan_file(&path, date_col).await {
                let size_mb = info.size_bytes as f64 / 1_000_000.0;
                let date_range = match (info.date_min, info.date_max) {
                    (Some(min), Some(max)) => format!("{} → {}", min, max),
                    _ => String::new(),
                };

                table.add_row(vec![
                    symbol.clone(),
                    info.row_count.to_string(),
                    format!("{:.2}", size_mb),
                    date_range,
                ]);
            }
        }
    }

    println!("{table}");
    Ok(())
}
