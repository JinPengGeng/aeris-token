use async_trait::async_trait;
use sha2::{Digest, Sha256};

const MAX_SCOPE_COMPONENT_LEN: usize = 191;
const MAX_OWNER_LEN: usize = 255;
const MAX_COMPLETION_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HalfOpenProbeScope {
    pub provider_key_id: String,
    pub api_format: String,
}

impl HalfOpenProbeScope {
    pub fn new(
        provider_key_id: impl Into<String>,
        api_format: impl Into<String>,
    ) -> Result<Self, crate::DataLayerError> {
        let scope = Self {
            provider_key_id: provider_key_id.into(),
            api_format: api_format.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_component(
            &self.provider_key_id,
            "half-open probe provider_key_id",
            MAX_SCOPE_COMPONENT_LEN,
        )?;
        validate_component(
            &self.api_format,
            "half-open probe api_format",
            MAX_SCOPE_COMPONENT_LEN,
        )
    }

    /// Stable, opaque coordination key. Length framing prevents ambiguous input pairs.
    pub fn coordination_key(&self) -> Result<String, crate::DataLayerError> {
        self.validate()?;
        let mut digest = Sha256::new();
        digest.update(b"aether:half-open-probe:v1\0");
        digest.update((self.provider_key_id.len() as u64).to_be_bytes());
        digest.update(self.provider_key_id.as_bytes());
        digest.update((self.api_format.len() as u64).to_be_bytes());
        digest.update(self.api_format.as_bytes());
        Ok(format!("half-open-probe:v1:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HalfOpenProbeOutcome {
    Succeeded,
    Failed,
}

impl HalfOpenProbeOutcome {
    pub const fn as_database(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unsupported half-open probe outcome {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HalfOpenProbeCompletion {
    pub completion_id: String,
    pub scope: HalfOpenProbeScope,
    pub owner: String,
    pub fencing_token: u64,
    pub completed_at_unix_ms: u64,
    pub outcome: HalfOpenProbeOutcome,
}

impl HalfOpenProbeCompletion {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_component(
            &self.completion_id,
            "half-open probe completion_id",
            MAX_COMPLETION_ID_LEN,
        )?;
        self.scope.validate()?;
        validate_component(&self.owner, "half-open probe owner", MAX_OWNER_LEN)?;
        if self.fencing_token == 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "half-open probe fencing_token must be positive".to_string(),
            ));
        }
        if self.completed_at_unix_ms == 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "half-open probe completed_at_unix_ms must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredHalfOpenProbeCompletion {
    pub completion_id: String,
    pub scope: HalfOpenProbeScope,
    pub owner: String,
    pub fencing_token: u64,
    pub completed_at_unix_ms: u64,
    pub outcome: HalfOpenProbeOutcome,
}

impl From<HalfOpenProbeCompletion> for StoredHalfOpenProbeCompletion {
    fn from(value: HalfOpenProbeCompletion) -> Self {
        Self {
            completion_id: value.completion_id,
            scope: value.scope,
            owner: value.owner,
            fencing_token: value.fencing_token,
            completed_at_unix_ms: value.completed_at_unix_ms,
            outcome: value.outcome,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HalfOpenProbeCompletionWrite {
    Applied(StoredHalfOpenProbeCompletion),
    RejectedStale { current_fencing_token: u64 },
}

#[async_trait]
pub trait HalfOpenProbeCompletionRepository: Send + Sync {
    /// Persist only when `completion.fencing_token` is strictly greater than the stored fence.
    async fn complete_if_newer(
        &self,
        completion: HalfOpenProbeCompletion,
    ) -> Result<HalfOpenProbeCompletionWrite, crate::DataLayerError>;
}

fn validate_component(
    value: &str,
    field: &str,
    max_len: usize,
) -> Result<(), crate::DataLayerError> {
    if value.trim().is_empty() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    if value.len() > max_len {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} cannot exceed {max_len} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordination_key_is_stable_and_scope_framed() {
        let first = HalfOpenProbeScope::new("ab", "c").expect("valid scope");
        let same = HalfOpenProbeScope::new("ab", "c").expect("valid scope");
        let ambiguous_without_framing = HalfOpenProbeScope::new("a", "bc").expect("valid scope");

        assert_eq!(
            first.coordination_key().expect("key"),
            same.coordination_key().expect("key")
        );
        assert_ne!(
            first.coordination_key().expect("key"),
            ambiguous_without_framing.coordination_key().expect("key")
        );
    }

    #[test]
    fn completion_rejects_zero_fence_and_timestamp() {
        let mut completion = HalfOpenProbeCompletion {
            completion_id: "completion-1".to_string(),
            scope: HalfOpenProbeScope::new("key-1", "openai").expect("valid scope"),
            owner: "node-1".to_string(),
            fencing_token: 0,
            completed_at_unix_ms: 1,
            outcome: HalfOpenProbeOutcome::Succeeded,
        };
        assert!(completion.validate().is_err());
        completion.fencing_token = 1;
        completion.completed_at_unix_ms = 0;
        assert!(completion.validate().is_err());
    }
}
