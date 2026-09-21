use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Enumeration: execution error kind.
pub enum ExecutionErrorKind {
/// Variant: connect timeout.
    ConnectTimeout,
/// Variant: first byte timeout.
    FirstByteTimeout,
/// Variant: read timeout.
    ReadTimeout,
/// Variant: upstream4xx.
    Upstream4xx,
/// Variant: upstream5xx.
    Upstream5xx,
/// Variant: tls error.
    TlsError,
/// Variant: proxy error.
    ProxyError,
/// Variant: protocol error.
    ProtocolError,
/// Variant: cancelled.
    Cancelled,
/// Variant: internal.
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Enumeration: execution phase.
pub enum ExecutionPhase {
/// Variant: connect.
    Connect,
/// Variant: handshake.
    Handshake,
/// Variant: write.
    Write,
/// Variant: first byte.
    FirstByte,
/// Variant: stream read.
    StreamRead,
/// Variant: decode.
    Decode,
/// Variant: finalize.
    Finalize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Data type: execution error.
pub struct ExecutionError {
/// Field: kind.
    pub kind: ExecutionErrorKind,
/// Field: phase.
    pub phase: ExecutionPhase,
/// Field: message.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
/// Field: upstream status.
    pub upstream_status: Option<u16>,
    #[serde(default)]
/// Field: retryable.
    pub retryable: bool,
    #[serde(default)]
/// Field: failover recommended.
    pub failover_recommended: bool,
}
