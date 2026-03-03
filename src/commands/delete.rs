//! Delete cached data for a symbol.

use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::cache::CacheStore;

pub async fn execute(cache: &CacheStore, symbols: &[String]) -> Result<()> {
    for symbol in symbols {
        delete_symbol(cache, symbol)?;
    }
    Ok(())
}

fn delete_symbol(cache: &CacheStore, symbol: &str) -> Result<()> {
    let options_path = cache.options_path(symbol)?;
    let prices_path = cache.prices_path(symbol)?;

    let has_options = options_path.exists();
    let has_prices = prices_path.exists();

    if !has_options && !has_prices {
        println!(
            "{} No cached data found for {}",
            style("⚠").yellow(),
            style(symbol.to_uppercase()).bold()
        );
        return Ok(());
    }

    // Build choices based on what exists
    let mut choices = Vec::new();
    if has_options && has_prices {
        choices.push("Options only");
        choices.push("Prices only");
        choices.push("Both options and prices");
    } else if has_options {
        choices.push("Options");
    } else {
        choices.push("Prices");
    }
    choices.push("Cancel");

    let prompt = format!(
        "Delete cached data for {}?",
        style(symbol.to_uppercase()).bold()
    );

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(&prompt)
        .items(&choices)
        .default(0)
        .interact()?;

    let selected = choices[selection];

    if selected == "Cancel" {
        println!("  Skipped {}", symbol.to_uppercase());
        return Ok(());
    }

    let delete_options = selected == "Options"
        || selected == "Options only"
        || selected == "Both options and prices";
    let delete_prices =
        selected == "Prices" || selected == "Prices only" || selected == "Both options and prices";

    if delete_options && has_options {
        std::fs::remove_file(&options_path)?;
        println!(
            "  {} Deleted options cache for {}",
            style("✓").green(),
            symbol.to_uppercase()
        );
    }

    if delete_prices && has_prices {
        std::fs::remove_file(&prices_path)?;
        println!(
            "  {} Deleted prices cache for {}",
            style("✓").green(),
            symbol.to_uppercase()
        );
    }

    Ok(())
}
