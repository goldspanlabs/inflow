//! Status command implementation.

use crate::cache::{scan_file, CacheStore};
use crate::utils::cache_status_table;
use anyhow::Result;

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
    let date_col = if category == "options" {
        "quote_date"
    } else {
        "date"
    };

    let mut table_data = Vec::new();

    for symbol in symbols {
        if let Ok(path) = cache.get_path(category, symbol) {
            if let Ok(info) = scan_file(&path, date_col).await {
                let size_mb = info.size_bytes as f64 / 1_000_000.0;
                let date_range = match (info.date_min, info.date_max) {
                    (Some(min), Some(max)) => format!("{} → {}", min, max),
                    _ => String::new(),
                };

                table_data.push((
                    symbol.clone(),
                    info.row_count,
                    size_mb,
                    date_range,
                ));
            }
        }
    }

    let table = cache_status_table(&table_data);
    println!("{table}");
    Ok(())
}
