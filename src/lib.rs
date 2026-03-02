//! Inflow library - market data downloader
//!
//! This library exposes the internal modules for use in integration tests and external consumers.

pub mod cache;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod pipeline;
pub mod providers;
pub mod utils;

pub use config::Config;
