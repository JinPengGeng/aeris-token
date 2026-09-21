use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    ExecutionError, ExecutionResponseObservation, ExecutionStreamTerminalSummary,
    ExecutionTelemetry,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Enumeration: stream frame type.
pub enum StreamFrameType {
    /// Variant: headers.
    Headers,
    /// Variant: data.
    Data,
    /// Variant: error.
    Error,
    /// Variant: telemetry.
    Telemetry,
    /// Variant: eof.
    Eof,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// Enumeration: stream frame payload.
pub enum StreamFramePayload {
    /// Variant: headers.
    Headers {
        /// Field: status code.
        status_code: u16,
        #[serde(default)]
        /// Field: headers.
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Field: response observation.
        response_observation: Option<ExecutionResponseObservation>,
    },
    /// Variant: data.
    Data {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Field: chunk b64.
        chunk_b64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Field: text.
        text: Option<String>,
    },
    /// Variant: error.
    Error {
        /// Field: error.
        error: ExecutionError,
    },
    /// Variant: telemetry.
    Telemetry {
        /// Field: telemetry.
        telemetry: ExecutionTelemetry,
    },
    /// Variant: eof.
    Eof {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Field: summary.
        summary: Option<ExecutionStreamTerminalSummary>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Data type: stream frame.
pub struct StreamFrame {
    #[serde(rename = "type")]
    /// Field: frame type.
    pub frame_type: StreamFrameType,
    /// Field: payload.
    pub payload: StreamFramePayload,
}

impl StreamFrame {
    /// Constructor / associated function: eof.
    pub fn eof() -> Self {
        Self::eof_with_summary(None)
    }

    /// Constructor / associated function: eof with summary.
    pub fn eof_with_summary(summary: Option<ExecutionStreamTerminalSummary>) -> Self {
        Self {
            frame_type: StreamFrameType::Eof,
            payload: StreamFramePayload::Eof { summary },
        }
    }
}
