//! Shared Polars utilities.

use anyhow::Result;
use polars::prelude::*;

/// Collect a `LazyFrame` on a blocking thread.
///
/// Polars collection is CPU-bound and can block the Tokio runtime.
/// This helper offloads the work to a blocking thread pool.
pub async fn collect_blocking(lf: LazyFrame) -> Result<DataFrame> {
    tokio::task::spawn_blocking(move || lf.collect().map_err(anyhow::Error::from)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_blocking_basic() {
        let series = Series::new(PlSmallStr::from("a"), &[1i32, 2, 3]);
        let df = DataFrame::new(3, vec![series.into_column()]).unwrap();
        let lf = df.lazy();

        let result = collect_blocking(lf).await.unwrap();
        assert_eq!(result.height(), 3);
        assert_eq!(result.get_column_names()[0].as_str(), "a");
    }

    #[tokio::test]
    async fn test_collect_blocking_empty() {
        let series = Series::new(PlSmallStr::from("x"), Vec::<i32>::new());
        let df = DataFrame::new(0, vec![series.into_column()]).unwrap();
        let lf = df.lazy();

        let result = collect_blocking(lf).await.unwrap();
        assert_eq!(result.height(), 0);
    }
}
