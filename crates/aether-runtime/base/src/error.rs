#[derive(Debug, thiserror::Error)]
/// RuntimeBootstrapError.
pub enum RuntimeBootstrapError {
    #[error("failed to initialize tracing: {0}")]
    /// Variant: Tracing.
    Tracing(String),
}
