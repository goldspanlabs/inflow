//! Inflow: standalone CLI for downloading market data to optopsy cache.

mod cache;
mod cli;
mod commands;
mod config;
mod error;
mod pipeline;
mod providers;
mod utils;

use clap::Parser;
use cli::{Args, Command};
use config::Config;
use error::InflowError;
use std::process::exit;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("inflow=info")),
        )
        .init();

    // Load configuration
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            exit(e.exit_code());
        }
    };

    // Parse CLI arguments
    let args = Args::parse();

    // Execute command
    let result = match args.command {
        Command::Download { target } => match commands::execute_download(&config, target).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        },

        Command::Status => {
            let cache = cache::CacheStore::new(config.data_root.clone());
            match commands::execute_status(&cache).await {
                Ok(()) => Ok(()),
                Err(e) => Err(InflowError::Other(e)),
            }
        }

        Command::Config => {
            commands::execute_config(&config);
            Ok(())
        }

        Command::Check { symbols } => {
            let cache = cache::CacheStore::new(config.data_root.clone());
            match commands::execute_check(&cache, &symbols).await {
                Ok(()) => Ok(()),
                Err(e) => Err(InflowError::Other(e)),
            }
        }

        Command::List { search } => commands::execute_list(&config, search.as_deref()).await,
    };

    // Handle result
    if let Err(e) = result {
        eprintln!("{e}");
        exit(e.exit_code());
    }
}
