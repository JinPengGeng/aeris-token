#[derive(Debug, thiserror::Error)]
pub enum DataLayerError {
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("postgres error: {0}")]
    Postgres(String),

    #[error("redis error: {0}")]
    Redis(String),

    #[error("sql error: {0}")]
    Sql(String),

    /// Structured SQLx failure carrying the original driver error as the
    /// `source()` of the error chain, so upper layers can programmatically
    /// distinguish constraint conflicts from connection loss via
    /// `sqlx::Error::as_database_error()`.
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("operation timed out: {0}")]
    TimedOut(String),

    #[error("unexpected database value: {0}")]
    UnexpectedValue(String),
}

impl Clone for DataLayerError {
    /// Cloning is lossy for the structured [`DataLayerError::Sqlx`] variant:
    /// `sqlx::Error` is not cloneable, so clones flatten it back to a plain
    /// [`DataLayerError::Sql`] message and drop the `source()` chain. Code
    /// that needs the structured source must use the original error.
    fn clone(&self) -> Self {
        match self {
            Self::InvalidConfiguration(message) => Self::InvalidConfiguration(message.clone()),
            Self::InvalidInput(message) => Self::InvalidInput(message.clone()),
            Self::Postgres(message) => Self::Postgres(message.clone()),
            Self::Redis(message) => Self::Redis(message.clone()),
            Self::Sql(message) => Self::Sql(message.clone()),
            Self::Sqlx(error) => Self::Sql(error.to_string()),
            Self::TimedOut(message) => Self::TimedOut(message.clone()),
            Self::UnexpectedValue(message) => Self::UnexpectedValue(message.clone()),
        }
    }
}

impl DataLayerError {
    pub fn postgres(error: impl std::fmt::Display) -> Self {
        Self::Postgres(error.to_string())
    }

    pub fn redis(error: impl std::fmt::Display) -> Self {
        Self::Redis(error.to_string())
    }

    pub fn sql(error: impl std::fmt::Display) -> Self {
        Self::Sql(error.to_string())
    }

    /// Wrap a driver error while preserving it as the structured `source()` of
    /// the error chain.
    pub fn sqlx(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }

    /// Returns `true` when the underlying database error is a foreign-key
    /// constraint violation (SQLSTATE 23503), which callers typically treat
    /// as permanent rather than retryable.
    pub fn is_foreign_key_violation(&self) -> bool {
        match self {
            Self::Sqlx(error) => error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .is_some_and(|code| code == "23503"),
            Self::Postgres(message) | Self::Sql(message) => {
                message.contains("SQLSTATE 23503")
                    || message.contains("violates foreign key constraint")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DataLayerError;

    #[test]
    fn sqlx_variant_preserves_source_chain() {
        use std::error::Error as _;

        let error = DataLayerError::from(sqlx::Error::PoolTimedOut);
        assert!(error.source().is_some(), "Sqlx variant must expose a source");
        let source = std::error::Error::source(&error).expect("source should be present");
        assert!(source.downcast_ref::<sqlx::Error>().is_some());
    }

    #[test]
    fn string_variants_still_carry_no_source() {
        let error = DataLayerError::Sql("syntax".to_string());
        assert!(std::error::Error::source(&error).is_none());
    }
}
