//! The narrow operator control for IronClaw's requested-model policy.
//!
//! This is not evidence of which model a provider actually ran, of TEE execution, or that a
//! member cannot alter policy. It compares the operator-visible requested-model catalogue to the
//! repository's model pin. Experiment 0037 measured the member refusal separately at the pinned
//! revision; this control does not generalize that measurement.

use crate::OperatorToken;
use ironworks_domain::{CredentialRef, ModelPin};

const CATALOGUE_PATH: &str = "/api/webchat/v2/llm/models";
const POLICY_PATH: &str = "/api/webchat/v2/llm/model-policy";

/// A secret-safe comparison with the repository's expected policy.
///
/// Current upstream strings are intentionally not retained. An unexpected response is remote
/// input and must not become a diagnostic channel for arbitrary values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPolicyComparison {
    pub selection_enabled: bool,
    pub workspace_default_matches: bool,
    pub allowed_models_exact: bool,
    pub allowed_model_count: usize,
}

impl ModelPolicyComparison {
    pub fn is_exact(self) -> bool {
        self.selection_enabled && self.workspace_default_matches && self.allowed_models_exact
    }
}

/// A failure to measure or apply policy. Messages never include response bodies or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPolicyError {
    NoCredential,
    Unreachable,
    CredentialRejected {
        operation: &'static str,
    },
    UnexpectedStatus {
        operation: &'static str,
        status: u16,
    },
    MalformedResponse,
    /// The final GET completed, but did not prove the requested singleton policy.
    ReadBackMismatch(ModelPolicyComparison),
}

/// The explicit operator-side model-policy control for one IronClaw instance.
pub struct ModelPolicyControl {
    agent: ureq::Agent,
    base: String,
    token: Option<OperatorToken>,
}

impl ModelPolicyControl {
    /// Resolve the one operator credential once. It is deliberately not represented as a
    /// `RuntimeMember` credential: those are different authorities even though both are bearer
    /// tokens at the HTTP boundary.
    pub fn new(base: impl Into<String>, credential: &CredentialRef) -> Self {
        Self {
            agent: crate::operator_agent(),
            base: base.into().trim_end_matches('/').to_owned(),
            token: OperatorToken::from_env(credential),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_token_for_test(&mut self, token: OperatorToken) {
        self.token = Some(token);
    }

    /// Read the current operator-visible catalogue and compare it exactly to `expected`.
    pub fn check(&self, expected: &ModelPin) -> Result<ModelPolicyComparison, ModelPolicyError> {
        let body = self.get_catalogue()?;
        compare(&body, expected)
    }

    /// Replace the policy with exactly `expected`, with mandatory read-back.
    ///
    /// `Ok` is possible only after the final GET proves exact agreement. This method cannot claim
    /// effective provider selection or TEE execution.
    pub fn apply(&self, expected: &ModelPin) -> Result<(), ModelPolicyError> {
        self.put_policy(expected)?;
        let after = compare(&self.get_catalogue()?, expected)?;
        if !after.is_exact() {
            return Err(ModelPolicyError::ReadBackMismatch(after));
        }
        Ok(())
    }

    fn authorization(&self) -> Result<String, ModelPolicyError> {
        self.token.as_ref().map(OperatorToken::header).ok_or(ModelPolicyError::NoCredential)
    }

    fn get_catalogue(&self) -> Result<String, ModelPolicyError> {
        let authorization = self.authorization()?;
        let mut response = self
            .agent
            .get(format!("{}{}", self.base, CATALOGUE_PATH))
            .header("authorization", authorization)
            .call()
            .map_err(|_| ModelPolicyError::Unreachable)?;
        let status = response.status().as_u16();
        if matches!(status, 401 | 403) {
            return Err(ModelPolicyError::CredentialRejected { operation: "GET" });
        }
        if !(200..300).contains(&status) {
            return Err(ModelPolicyError::UnexpectedStatus { operation: "GET", status });
        }
        response.body_mut().read_to_string().map_err(|_| ModelPolicyError::MalformedResponse)
    }

    fn put_policy(&self, expected: &ModelPin) -> Result<(), ModelPolicyError> {
        let authorization = self.authorization()?;
        let body = serde_json::json!({
            "workspace_default": expected.as_str(),
            "allowed_models": [expected.as_str()],
        });
        let response = self
            .agent
            .put(format!("{}{}", self.base, POLICY_PATH))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .send(body.to_string())
            .map_err(|_| ModelPolicyError::Unreachable)?;
        let status = response.status().as_u16();
        if matches!(status, 401 | 403) {
            return Err(ModelPolicyError::CredentialRejected { operation: "PUT" });
        }
        if !(200..300).contains(&status) {
            return Err(ModelPolicyError::UnexpectedStatus { operation: "PUT", status });
        }
        Ok(())
    }
}

fn compare(body: &str, expected: &ModelPin) -> Result<ModelPolicyComparison, ModelPolicyError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| ModelPolicyError::MalformedResponse)?;
    let selection_enabled = value
        .get("selection_enabled")
        .and_then(serde_json::Value::as_bool)
        .ok_or(ModelPolicyError::MalformedResponse)?;
    let workspace_default = match value.get("workspace_default") {
        Some(serde_json::Value::String(value)) => Some(value.as_str()),
        Some(serde_json::Value::Null) | None => None,
        _ => return Err(ModelPolicyError::MalformedResponse),
    };
    let models = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or(ModelPolicyError::MalformedResponse)?;
    if !models.iter().all(serde_json::Value::is_string) {
        return Err(ModelPolicyError::MalformedResponse);
    }
    let allowed_models_exact = models.len() == 1
        && models.first().and_then(serde_json::Value::as_str) == Some(expected.as_str());
    Ok(ModelPolicyComparison {
        selection_enabled,
        workspace_default_matches: workspace_default == Some(expected.as_str()),
        allowed_models_exact,
        allowed_model_count: models.len(),
    })
}
