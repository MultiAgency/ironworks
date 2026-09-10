//! Human-authorized Account Service operations that never enter a model turn.

use crate::{CredentialSource, Secret};
use ironworks_domain::{CredentialPurpose, CredentialRef, OrgId};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct BookExport {
    bytes: Vec<u8>,
    org: OrgId,
    digest: String,
    records: usize,
}

impl BookExport {
    /// Validate a preserved export against the same complete, scoped contract as a live response.
    pub fn validate(
        bytes: Vec<u8>,
        expected_org: &OrgId,
    ) -> Result<Self, RecordAdministrationFailure> {
        validate_export(bytes, expected_org)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn org(&self) -> &OrgId {
        &self.org
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn is_empty(&self) -> bool {
        self.records == 0
    }
}

pub struct Correction<'a> {
    pub activity_id: &'a str,
    pub supersedes: &'a str,
    pub body: &'a str,
    pub reason: &'a str,
}

pub struct Retraction<'a> {
    pub activity_id: &'a str,
    pub target_activity: &'a str,
    pub reason: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteReceipt {
    pub org: OrgId,
    pub deletion_id: String,
    pub deleted_at: String,
    pub remaining_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordAdministrationFailure {
    #[error("no credential is available for {handle}")]
    NoCredential { handle: String },
    #[error("{handle} is a {got:?} credential; this operation requires {expected:?}")]
    WrongPurpose { handle: String, got: CredentialPurpose, expected: CredentialPurpose },
    #[error("the Account Service could not be reached: {detail}")]
    Unreachable { detail: String },
    #[error("the Account Service refused this credential")]
    Unauthorized,
    #[error("the Account Service refused the request: HTTP {status}")]
    Refused { status: u16 },
    #[error("the Account Service answer is malformed: {detail}")]
    Malformed { detail: String },
    #[error("SCOPE DRIFT: expected {expected}, Account Service answered for {answered}")]
    ScopeDrift { expected: OrgId, answered: OrgId },
    #[error("the export is not marked complete")]
    Incomplete,
    #[error("the export digest cannot be reproduced")]
    DigestMismatch,
    #[error("the export is not deterministically ordered at {collection}")]
    Nondeterministic { collection: String },
}

pub struct RecordAdministration<C: CredentialSource> {
    agent: ureq::Agent,
    base: String,
    credentials: C,
}

/// One request this crate knows how to make of the Account Service.
///
/// # Why this is a type
///
/// `request` took `(method: &str, path: &str, secret, payload: Option<String>)` and matched the
/// verb against the body, which left arms it could not serve — `("GET", Some(body))`, or any
/// typo'd verb — and answered them with
/// `Malformed { detail: "internal unsupported Account Service request" }`. That is a programming
/// error routed through a **public failure enum whose every other variant is about the wire**, so
/// a caller could not tell "the store answered badly" from "we built the request wrong".
///
/// Pairing the verb with its body here makes both arms unrepresentable, and that use of
/// `Malformed` is gone.
enum Request<'a> {
    Get(&'a str),
    Delete(&'a str),
    Post { path: &'a str, body: String },
}

impl Request<'_> {
    fn path(&self) -> &str {
        match self {
            Self::Get(path) | Self::Delete(path) => path,
            Self::Post { path, .. } => path,
        }
    }
}

impl<C: CredentialSource> RecordAdministration<C> {
    pub fn new(base: impl Into<String>, credentials: C) -> Self {
        Self {
            agent: crate::agent(),
            base: base.into().trim_end_matches('/').to_owned(),
            credentials,
        }
    }

    pub fn export(
        &self,
        credential: &CredentialRef,
        expected_org: &OrgId,
    ) -> Result<BookExport, RecordAdministrationFailure> {
        self.require_purpose(credential, CredentialPurpose::RecordExport)?;
        let secret = self.secret(credential)?;
        let (status, body) = self.request(Request::Get("/export"), &secret)?;
        self.require_success(status)?;
        BookExport::validate(body.into_bytes(), expected_org)
    }

    pub fn append_correction(
        &self,
        credential: &CredentialRef,
        correction: &Correction<'_>,
    ) -> Result<(), RecordAdministrationFailure> {
        self.require_purpose(credential, CredentialPurpose::RecordCorrect)?;
        require_human_text(correction.reason, "correction reason")?;
        let payload = serde_json::json!({
            "activity_id": correction.activity_id,
            "supersedes": correction.supersedes,
            "body": correction.body,
            "reason": correction.reason,
        });
        self.mutation(credential, "/append_correction", payload)
    }

    pub fn append_retraction(
        &self,
        credential: &CredentialRef,
        retraction: &Retraction<'_>,
    ) -> Result<(), RecordAdministrationFailure> {
        self.require_purpose(credential, CredentialPurpose::RecordRetract)?;
        require_human_text(retraction.reason, "retraction reason")?;
        let payload = serde_json::json!({
            "activity_id": retraction.activity_id,
            "target_activity": retraction.target_activity,
            "reason": retraction.reason,
        });
        self.mutation(credential, "/append_retraction", payload)
    }

    pub fn delete_org_records(
        &self,
        credential: &CredentialRef,
        expected_org: &OrgId,
    ) -> Result<DeleteReceipt, RecordAdministrationFailure> {
        self.require_purpose(credential, CredentialPurpose::RecordDelete)?;
        let secret = self.secret(credential)?;
        let (status, body) = self.request(Request::Delete("/org-records"), &secret)?;
        self.require_success(status)?;
        validate_delete_receipt(&body, expected_org)
    }

    fn mutation(
        &self,
        credential: &CredentialRef,
        path: &str,
        payload: Value,
    ) -> Result<(), RecordAdministrationFailure> {
        let secret = self.secret(credential)?;
        let (status, _) =
            self.request(Request::Post { path, body: payload.to_string() }, &secret)?;
        self.require_success(status)
    }

    fn request(
        &self,
        request: Request<'_>,
        secret: &Secret,
    ) -> Result<(u16, String), RecordAdministrationFailure> {
        let url = format!("{}{}", self.base, request.path());
        let response = match request {
            Request::Get(_) => {
                self.agent.get(url).header("X-Service-Token", secret.expose()).call()
            }
            Request::Delete(_) => {
                self.agent.delete(url).header("X-Service-Token", secret.expose()).call()
            }
            Request::Post { body, .. } => self
                .agent
                .post(url)
                .header("X-Service-Token", secret.expose())
                .header("content-type", "application/json")
                .send(&body),
        };
        let mut response = response
            .map_err(|e| RecordAdministrationFailure::Unreachable { detail: e.to_string() })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| RecordAdministrationFailure::Unreachable { detail: e.to_string() })?;
        Ok((status, body))
    }

    fn require_success(&self, status: u16) -> Result<(), RecordAdministrationFailure> {
        match status {
            200..=299 => Ok(()),
            401 | 403 => Err(RecordAdministrationFailure::Unauthorized),
            status => Err(RecordAdministrationFailure::Refused { status }),
        }
    }

    fn require_purpose(
        &self,
        credential: &CredentialRef,
        expected: CredentialPurpose,
    ) -> Result<(), RecordAdministrationFailure> {
        if credential.purpose() == expected {
            Ok(())
        } else {
            Err(RecordAdministrationFailure::WrongPurpose {
                handle: credential.handle().to_owned(),
                got: credential.purpose(),
                expected,
            })
        }
    }

    fn secret(&self, credential: &CredentialRef) -> Result<Secret, RecordAdministrationFailure> {
        self.credentials.resolve(credential).ok_or_else(|| {
            RecordAdministrationFailure::NoCredential { handle: credential.handle().to_owned() }
        })
    }
}

fn validate_delete_receipt(
    body: &str,
    expected_org: &OrgId,
) -> Result<DeleteReceipt, RecordAdministrationFailure> {
    let wire: WireDelete = serde_json::from_str(body)
        .map_err(|e| RecordAdministrationFailure::Malformed { detail: e.to_string() })?;
    let org = OrgId::parse(wire.org)
        .map_err(|e| RecordAdministrationFailure::Malformed { detail: e.to_string() })?;
    if &org != expected_org {
        return Err(RecordAdministrationFailure::ScopeDrift {
            expected: expected_org.clone(),
            answered: org,
        });
    }
    if wire.deletion_id.trim().is_empty() || wire.deleted_at.trim().is_empty() {
        return Err(RecordAdministrationFailure::Malformed {
            detail: "deletion receipt is missing its id or time".to_owned(),
        });
    }
    if wire.remaining_records != 0 {
        return Err(RecordAdministrationFailure::Malformed {
            detail: format!(
                "deletion receipt reports {} remaining record(s)",
                wire.remaining_records
            ),
        });
    }
    Ok(DeleteReceipt {
        org,
        deletion_id: wire.deletion_id,
        deleted_at: wire.deleted_at,
        remaining_records: wire.remaining_records,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDelete {
    org: String,
    deletion_id: String,
    deleted_at: String,
    remaining_records: u64,
}

fn validate_export(
    bytes: Vec<u8>,
    expected_org: &OrgId,
) -> Result<BookExport, RecordAdministrationFailure> {
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| RecordAdministrationFailure::Malformed { detail: e.to_string() })?;
    let object = value.as_object_mut().ok_or_else(|| RecordAdministrationFailure::Malformed {
        detail: "export is not an object".to_owned(),
    })?;
    if object.get("format").and_then(Value::as_str) != Some("ironworks-book-v1") {
        return Err(RecordAdministrationFailure::Malformed {
            detail: "export format is not ironworks-book-v1".to_owned(),
        });
    }
    if object.get("complete").and_then(Value::as_bool) != Some(true) {
        return Err(RecordAdministrationFailure::Incomplete);
    }
    let answered = OrgId::parse(object.get("org").and_then(Value::as_str).unwrap_or_default())
        .map_err(|e| RecordAdministrationFailure::Malformed { detail: e.to_string() })?;
    if &answered != expected_org {
        return Err(RecordAdministrationFailure::ScopeDrift {
            expected: expected_org.clone(),
            answered,
        });
    }
    let mut records = 0;
    for (collection, key) in
        [("accounts", "account_id"), ("contacts", "contact_id"), ("activities", "activity_id")]
    {
        let rows = object.get(collection).and_then(Value::as_array).ok_or_else(|| {
            RecordAdministrationFailure::Malformed { detail: format!("missing {collection}") }
        })?;
        records += rows.len();
        let mut previous = None;
        for row in rows {
            let row = row.as_object().ok_or_else(|| RecordAdministrationFailure::Malformed {
                detail: format!("{collection} contains a non-object"),
            })?;
            let id = row.get(key).and_then(Value::as_str).unwrap_or_default();
            if id.is_empty() || previous.is_some_and(|prior| prior >= id) {
                return Err(RecordAdministrationFailure::Nondeterministic {
                    collection: collection.to_owned(),
                });
            }
            if !row.contains_key("provenance") {
                return Err(RecordAdministrationFailure::Malformed {
                    detail: format!("{collection} row has no provenance"),
                });
            }
            previous = Some(id);
        }
    }
    let claimed = object
        .remove("full_content_digest")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| RecordAdministrationFailure::Malformed {
            detail: "export has no full_content_digest".to_owned(),
        })?;
    // `?`, not `unwrap`. The value came off the wire, and this is the only non-test `unwrap` in
    // the crate: a shape `serde_json` will not re-serialise is a malformed answer, which is a
    // variant this function already has.
    let canonical = serde_json::to_vec(&value)
        .map_err(|e| RecordAdministrationFailure::Malformed { detail: e.to_string() })?;
    let reproduced = format!("{:x}", Sha256::digest(canonical));
    if claimed != reproduced {
        return Err(RecordAdministrationFailure::DigestMismatch);
    }
    Ok(BookExport { bytes, org: expected_org.clone(), digest: claimed, records })
}

fn require_human_text(text: &str, what: &str) -> Result<(), RecordAdministrationFailure> {
    if text.trim().is_empty() {
        Err(RecordAdministrationFailure::Malformed { detail: format!("{what} is empty") })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(org: &str, complete: bool, ids: &[&str]) -> Vec<u8> {
        let mut value = serde_json::json!({
            "format": "ironworks-book-v1",
            "org": org,
            "complete": complete,
            "exported_at": "2026-09-06T12:00:00+00:00",
            "accounts": ids.iter().map(|id| serde_json::json!({"account_id":id,"provenance":{"source":"test"}})).collect::<Vec<_>>(),
            "contacts": [],
            "activities": []
        });
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&value).unwrap()));
        value
            .as_object_mut()
            .unwrap()
            .insert("full_content_digest".to_owned(), Value::String(digest));
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn a_complete_ordered_scoped_export_reproduces_its_digest() {
        let org = OrgId::parse("acme").unwrap();
        let got = validate_export(export("acme", true, &["a", "b"]), &org).unwrap();
        assert_eq!(got.org(), &org);
        assert_eq!(got.digest().len(), 64);
    }

    #[test]
    fn export_refuses_both_scope_directions_incompleteness_order_and_digest_drift() {
        let acme = OrgId::parse("acme").unwrap();
        let globex = OrgId::parse("globex").unwrap();
        assert!(matches!(
            validate_export(export("globex", true, &["a"]), &acme),
            Err(RecordAdministrationFailure::ScopeDrift { .. })
        ));
        assert!(matches!(
            validate_export(export("acme", true, &["a"]), &globex),
            Err(RecordAdministrationFailure::ScopeDrift { .. })
        ));
        assert_eq!(
            validate_export(export("acme", false, &["a"]), &acme).unwrap_err(),
            RecordAdministrationFailure::Incomplete
        );
        assert!(matches!(
            validate_export(export("acme", true, &["b", "a"]), &acme),
            Err(RecordAdministrationFailure::Nondeterministic { .. })
        ));
        let mut corrupt = export("acme", true, &["a"]);
        let position = corrupt.iter().position(|byte| *byte == b'a').unwrap();
        corrupt[position] = b'z';
        assert!(validate_export(corrupt, &acme).is_err());
    }

    #[test]
    fn deletion_receipt_requires_the_scoped_org_and_zero_remaining_records() {
        let acme = OrgId::parse("acme").unwrap();
        let complete = r#"{"org":"acme","deletion_id":"delete-1","deleted_at":"2026-09-06T12:00:00Z","remaining_records":0}"#;
        assert_eq!(validate_delete_receipt(complete, &acme).unwrap().remaining_records, 0);

        let incomplete = r#"{"org":"acme","deletion_id":"delete-1","deleted_at":"2026-09-06T12:00:00Z","remaining_records":1}"#;
        assert!(matches!(
            validate_delete_receipt(incomplete, &acme),
            Err(RecordAdministrationFailure::Malformed { .. })
        ));

        let wrong_org = r#"{"org":"globex","deletion_id":"delete-1","deleted_at":"2026-09-06T12:00:00Z","remaining_records":0}"#;
        assert!(matches!(
            validate_delete_receipt(wrong_org, &acme),
            Err(RecordAdministrationFailure::ScopeDrift { .. })
        ));
    }
}
