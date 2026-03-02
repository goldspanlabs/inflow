//! Command-line interface for inflow.

use chrono::NaiveDate;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "inflow")]
#[command(about = "Download and cache market data for optopsy", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Download options chains or prices
    Download {
        #[command(subcommand)]
        target: DownloadTarget,
    },

    /// Show cache status
    Status,

    /// Show resolved configuration
    Config,
}

#[derive(Subcommand, Debug)]
pub enum DownloadTarget {
    /// Download options chains from EODHD
    Options {
        /// Symbols to download (e.g., SPY, QQQ)
        symbols: Vec<String>,

        /// Start date (YYYY-MM-DD) for download window
        #[arg(long, value_parser = parse_naive_date)]
        from: Option<NaiveDate>,

        /// Number of concurrent downloads (default: 4)
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },

    /// Download prices from Yahoo Finance
    Prices {
        /// Symbols to download (e.g., SPY, QQQ)
        symbols: Vec<String>,

        /// Period for historical data: 1mo, 3mo, 6mo, 1y, 5y, max
        #[arg(long, default_value = "1y")]
        period: String,

        /// Number of concurrent downloads (default: 4)
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },

    /// Download both options and prices
    All {
        /// Symbols to download (e.g., SPY, QQQ)
        symbols: Vec<String>,

        /// Start date (YYYY-MM-DD) for options download window
        #[arg(long, value_parser = parse_naive_date)]
        from: Option<NaiveDate>,

        /// Period for historical prices: 1mo, 3mo, 6mo, 1y, 5y, max
        #[arg(long, default_value = "1y")]
        period: String,

        /// Number of concurrent downloads (default: 4)
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },
}

/// Parse a date string in YYYY-MM-DD format.
fn parse_naive_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("Invalid date: '{}'. Expected format: YYYY-MM-DD", s))
}
