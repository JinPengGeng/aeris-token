use crate::observability::{FileLoggingConfig, LogDestination, ServiceObservabilityConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
/// ServiceRuntimeConfig.
pub struct ServiceRuntimeConfig {
    /// Field: service_name.
    pub service_name: &'static str,
    /// Field: default_log_filter.
    pub default_log_filter: &'static str,
    /// Field: observability.
    pub observability: ServiceObservabilityConfig,
}

impl ServiceRuntimeConfig {
    /// Executes `new`.
    pub const fn new(service_name: &'static str, default_log_filter: &'static str) -> Self {
        Self {
            service_name,
            default_log_filter,
            observability: ServiceObservabilityConfig::new(crate::LogFormat::Pretty, service_name),
        }
    }

    /// Executes `with_log_format`.
    pub const fn with_log_format(mut self, log_format: crate::LogFormat) -> Self {
        self.observability.log_format = log_format;
        self
    }

    /// Executes `with_log_destination`.
    pub const fn with_log_destination(mut self, log_destination: LogDestination) -> Self {
        self.observability.log_destination = log_destination;
        self
    }

    /// Executes `with_file_logging`.
    pub fn with_file_logging(mut self, file_logging: FileLoggingConfig) -> Self {
        self.observability.file_logging = Some(file_logging);
        self
    }

    /// Executes `with_node_role`.
    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.observability.node_role = Some(node_role.into());
        self
    }

    /// Executes `with_instance_id`.
    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.observability.instance_id = Some(instance_id.into());
        self
    }

    /// Executes `with_metrics_namespace`.
    pub const fn with_metrics_namespace(mut self, metrics_namespace: &'static str) -> Self {
        self.observability.metrics_namespace = metrics_namespace;
        self
    }
}
