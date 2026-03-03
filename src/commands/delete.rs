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

/// Build the interactive menu choices based on which cache files exist.
fn build_delete_choices(has_options: bool, has_prices: bool) -> Vec<&'static str> {
    let mut choices = Vec::new();
    if has_options && has_prices {
        choices.push("Options only");
        choices.push("Prices only");
        choices.push("Both options and prices");
    } else if has_options {
        choices.push("Options");
    } else if has_prices {
        choices.push("Prices");
    }
    choices.push("Cancel");
    choices
}

/// Determine which caches to delete based on the user's menu selection.
fn resolve_deletions(choice: &str) -> (bool, bool) {
    let delete_options =
        choice == "Options" || choice == "Options only" || choice == "Both options and prices";
    let delete_prices =
        choice == "Prices" || choice == "Prices only" || choice == "Both options and prices";
    (delete_options, delete_prices)
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

    let choices = build_delete_choices(has_options, has_prices);

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

    let (delete_options, delete_prices) = resolve_deletions(selected);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_choices_both_exist() {
        let choices = build_delete_choices(true, true);
        assert_eq!(
            choices,
            vec![
                "Options only",
                "Prices only",
                "Both options and prices",
                "Cancel"
            ]
        );
    }

    #[test]
    fn build_choices_options_only() {
        let choices = build_delete_choices(true, false);
        assert_eq!(choices, vec!["Options", "Cancel"]);
    }

    #[test]
    fn build_choices_prices_only() {
        let choices = build_delete_choices(false, true);
        assert_eq!(choices, vec!["Prices", "Cancel"]);
    }

    /// Verifies that every choice produced by `build_delete_choices`
    /// round-trips through `resolve_deletions` correctly.
    #[test]
    fn choices_round_trip_through_resolve() {
        let choices = build_delete_choices(true, true);
        // Non-cancel choices should resolve to at least one deletion
        for &choice in choices.iter().filter(|c| **c != "Cancel") {
            let (del_opts, del_prices) = resolve_deletions(choice);
            assert!(del_opts || del_prices, "{choice} should delete something");
        }
        // Cancel should delete nothing
        assert_eq!(resolve_deletions("Cancel"), (false, false));
    }
}
