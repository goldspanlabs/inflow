//! Table formatting utilities for CLI output.

use comfy_table::Table;

/// Build a table for download results.
pub fn download_results_table(results: &[(String, String, usize, usize, String, String)]) -> Table {
    let mut table = Table::new();
    table.set_header(vec![
        "Symbol",
        "Provider",
        "New Rows",
        "Total Rows",
        "Date Range",
        "Status",
    ]);

    for (symbol, provider, new_rows, total_rows, date_range, status) in results {
        table.add_row(vec![
            symbol.clone(),
            provider.clone(),
            new_rows.to_string(),
            total_rows.to_string(),
            date_range.clone(),
            status.clone(),
        ]);
    }

    table
}

/// Build a table for cache status.
pub fn cache_status_table(
    rows: &[(String, usize, f64, String)],
) -> Table {
    let mut table = Table::new();
    table.set_header(vec!["Symbol", "Rows", "Size (MB)", "Date Range"]);

    for (symbol, row_count, size_mb, date_range) in rows {
        table.add_row(vec![
            symbol.clone(),
            row_count.to_string(),
            format!("{:.2}", size_mb),
            date_range.clone(),
        ]);
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_results_table_creation() {
        let results = vec![(
            "SPY".to_string(),
            "EODHD".to_string(),
            100,
            1000,
            "2024-01-01 → 2024-03-01".to_string(),
            "✓".to_string(),
        )];

        let table = download_results_table(&results);
        let table_str = format!("{}", table);
        assert!(table_str.contains("SPY"));
        assert!(table_str.contains("EODHD"));
        assert!(table_str.contains("100"));
    }

    #[test]
    fn test_cache_status_table_creation() {
        let rows = vec![(
            "SPY".to_string(),
            100,
            1.5,
            "2024-01-01 → 2024-03-01".to_string(),
        )];

        let table = cache_status_table(&rows);
        let table_str = format!("{}", table);
        assert!(table_str.contains("SPY"));
        assert!(table_str.contains("100"));
        assert!(table_str.contains("1.50"));
    }
}
