//! Error types for inflow.

use thiserror::Error;

/// `inflow` error type.
#[derive(Error, Debug)]
pub enum InflowError {
    /// Configuration error (exit code 2).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Partial download failure — some symbols succeeded, others failed (exit code 1).
    #[error("Partial failure: {0}")]
    PartialFailure(String),

    /// All other errors (exit code 1).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl InflowError {
    /// Return the exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::PartialFailure(_) | Self::Other(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error_exit_code() {
        let err = InflowError::Config("bad config".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn test_partial_failure_exit_code() {
        let err = InflowError::PartialFailure("some failed".into());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_other_error_exit_code() {
        let err = InflowError::Other(anyhow::anyhow!("something went wrong"));
        assert_eq!(err.exit_code(), 1);
    }
}
