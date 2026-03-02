//! Parquet cache storage and atomic writes.

use anyhow::{bail, Context, Result};
use polars::prelude::*;
use std::path::{Path, PathBuf};

use anyhow;

/// Cache store for reading and writing Parquet files.
#[derive(Debug, Clone)]
pub struct CacheStore {
    /// Root cache directory (e.g., `~/.optopsy/cache`).
    root: PathBuf,
}

impl CacheStore {
    /// Create a new cache store.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Get the options cache path for a symbol.
    ///
    /// Returns `{root}/options/{SYMBOL}.parquet` where SYMBOL is uppercased.
    pub fn options_path(&self, symbol: &str) -> Result<PathBuf> {
        let safe = self.validate_symbol(symbol)?;
        Ok(self.root.join("options").join(format!("{safe}.parquet")))
    }

    /// Get the prices cache path for a symbol.
    ///
    /// Returns `{root}/prices/{SYMBOL}.parquet` where SYMBOL is uppercased.
    pub fn prices_path(&self, symbol: &str) -> Result<PathBuf> {
        let safe = self.validate_symbol(symbol)?;
        Ok(self.root.join("prices").join(format!("{safe}.parquet")))
    }

    /// Get the cache path for a symbol in a specific category.
    ///
    /// Returns the appropriate path based on category (options or prices).
    /// This is a convenience method to avoid duplicating path selection logic.
    pub fn get_path(&self, category: &str, symbol: &str) -> Result<PathBuf> {
        match category {
            "options" => self.options_path(symbol),
            "prices" => self.prices_path(symbol),
            _ => Err(anyhow::anyhow!("Unknown category: {}", category)),
        }
    }

    /// Atomically write a DataFrame to the given path.
    ///
    /// Writes to a temporary `.parquet.tmp` file and renames atomically to avoid
    /// corruption from interrupted I/O.
    pub async fn atomic_write(&self, path: &Path, df: &mut DataFrame) -> Result<()> {
        let path_owned = path.to_path_buf();
        let mut df_clone = df.clone();

        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path_owned.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create cache dir: {}", parent.display()))?;
            }

            let tmp_path = path_owned.with_extension("parquet.tmp");
            let file = std::fs::File::create(&tmp_path)
                .with_context(|| format!("Failed to create temp file: {}", tmp_path.display()))?;

            ParquetWriter::new(file)
                .finish(&mut df_clone)
                .context("Failed to write parquet")?;

            std::fs::rename(&tmp_path, &path_owned).with_context(|| {
                format!(
                    "Failed to rename {} → {}",
                    tmp_path.display(),
                    path_owned.display()
                )
            })?;

            Ok::<(), anyhow::Error>(())
        })
        .await?
    }

    /// Read a Parquet file as a lazy frame.
    ///
    /// Returns `None` if the file does not exist.
    pub async fn read_parquet(&self, path: &Path) -> Result<Option<LazyFrame>> {
        let path_owned = path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            if !path_owned.exists() {
                return Ok(None);
            }

            let path_str = path_owned.to_string_lossy().to_string();
            let lf = LazyFrame::scan_parquet(
                path_str.as_str().into(),
                ScanArgsParquet::default(),
            )
            .context("Failed to scan parquet")?;

            Ok(Some(lf))
        })
        .await?
    }

    /// List all symbol files in a category (options or prices).
    ///
    /// Returns symbols in sorted order, without `.parquet` extension.
    pub fn list_symbols(&self, category: &str) -> Result<Vec<String>> {
        validate_path_segment(category)?;

        let category_dir = self.root.join(category);
        if !category_dir.exists() {
            return Ok(vec![]);
        }

        let mut symbols = Vec::new();
        for entry in std::fs::read_dir(&category_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "parquet") {
                if let Some(stem) = path.file_stem() {
                    symbols.push(stem.to_string_lossy().to_string());
                }
            }
        }
        symbols.sort();
        Ok(symbols)
    }

    /// Validate a symbol and return the uppercase version.
    fn validate_symbol(&self, symbol: &str) -> Result<String> {
        if symbol.is_empty() {
            bail!("symbol must not be empty");
        }
        // Allow alphanumeric, dots, and dashes (common in tickers like BRK.A, BF.B, etc.)
        for c in symbol.chars() {
            if !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_' {
                bail!("symbol contains invalid character: {}", c);
            }
        }
        Ok(symbol.to_uppercase())
    }
}

/// Ensure a path segment (category name) contains only safe characters.
fn validate_path_segment(segment: &str) -> Result<()> {
    if segment.is_empty() {
        bail!("path segment must not be empty");
    }
    if std::path::Path::new(segment)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("path segment contains illegal characters: {segment}");
    }
    if segment.contains('/') || segment.contains('\\') {
        bail!("path segment must not contain separators: {segment}");
    }
    Ok(())
}
