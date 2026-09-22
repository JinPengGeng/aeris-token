use crate::tracing::LogFormat;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LogDestination.
pub enum LogDestination {
    /// Variant: Stdout.
    Stdout,
    /// Variant: File.
    File,
    /// Variant: Both.
    Both,
}

impl LogDestination {
    /// Executes `needs_file_sink`.
    pub const fn needs_file_sink(self) -> bool {
        matches!(self, Self::File | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// LogRotation.
pub enum LogRotation {
    /// Variant: Hourly.
    Hourly,
    /// Variant: Daily.
    Daily,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// FileLoggingConfig.
pub struct FileLoggingConfig {
    /// Field: dir.
    pub dir: PathBuf,
    /// Field: rotation.
    pub rotation: LogRotation,
    /// Field: retention_days.
    pub retention_days: u64,
    /// Field: max_files.
    pub max_files: usize,
}

impl FileLoggingConfig {
    /// Executes `new`.
    pub fn new(
        dir: impl Into<PathBuf>,
        rotation: LogRotation,
        retention_days: u64,
        max_files: usize,
    ) -> Self {
        Self {
            dir: dir.into(),
            rotation,
            retention_days,
            max_files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// ServiceObservabilityConfig.
pub struct ServiceObservabilityConfig {
    /// Field: log_format.
    pub log_format: LogFormat,
    /// Field: metrics_namespace.
    pub metrics_namespace: &'static str,
    /// Field: log_destination.
    pub log_destination: LogDestination,
    /// Field: file_logging.
    pub file_logging: Option<FileLoggingConfig>,
    /// Field: node_role.
    pub node_role: Option<String>,
    /// Field: instance_id.
    pub instance_id: Option<String>,
}

impl ServiceObservabilityConfig {
    /// Executes `new`.
    pub const fn new(log_format: LogFormat, metrics_namespace: &'static str) -> Self {
        Self {
            log_format,
            metrics_namespace,
            log_destination: LogDestination::Stdout,
            file_logging: None,
            node_role: None,
            instance_id: None,
        }
    }

    /// Executes `with_log_destination`.
    pub const fn with_log_destination(mut self, log_destination: LogDestination) -> Self {
        self.log_destination = log_destination;
        self
    }

    /// Executes `with_file_logging`.
    pub fn with_file_logging(mut self, file_logging: FileLoggingConfig) -> Self {
        self.file_logging = Some(file_logging);
        self
    }

    /// Executes `with_node_role`.
    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.node_role = Some(node_role.into());
        self
    }

    /// Executes `with_instance_id`.
    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.instance_id = Some(instance_id.into());
        self
    }
}
