//! Appending one confirmed note, and nothing else.
//!
//! # Why this is beside `Book` rather than inside it
//!
//! `Book` is the read adapter, and every call on it takes a [`RecordScope`] — a type obtainable
//! only from a verified [`ServiceInstance`], which is what a turn is built from. Adding a write
//! method there would put mutation one autocomplete away from every serving-path call site, and
//! would mean the type a turn holds has a write method on it at all.
//!
//! So this is a separate client with a separate credential purpose, reachable only from the host
//! command path. It is not a new architectural layer: it shares `Book`'s crate, its
//! [`CredentialSource`], its `Secret`, its timeout policy and its failure vocabulary. What it does
//! not share is a caller.
//!
//! # What it can express
//!
//! One request: append this activity, to this account, in the organization this credential
//! resolves to. There is no update, no delete, no list, and no read — which is why it needs no
//! scope type of its own: the server resolves the organization from the credential, and
//! [`Append::expected_org`] is a guard the server compares rather than a selector it obeys.
//!
//! [`RecordScope`]: ironworks_domain::RecordScope
//! [`ServiceInstance`]: ironworks_domain::ServiceInstance

use crate::credential::{CredentialSource, Secret};
use ironworks_domain::record::{AccountId, Date};
use ironworks_domain::{ContributorId, CredentialPurpose, CredentialRef, OrgId};

/// What one confirmed promotion asks the store to record.
///
/// Every field is immutable at the server: a replay carrying the same values succeeds, and one
/// carrying different values under the same `activity_id` is refused rather than absorbed.
#[derive(Debug, Clone)]
pub struct Append<'a> {
    /// Deterministic, and inside the store's shared-note namespace. Two attempts to promote the
    /// same proposal produce the same id, which is what makes a retry a replay.
    pub activity_id: &'a str,
    pub account: &'a AccountId,
    /// The day the note is recorded against.
    ///
    /// A [`Date`] and not a `&str` whose grammar was stated in this comment. The store refuses a
    /// malformed one with `invalid_occurred_at` — a round trip and a refusal code to learn what a
    /// ten-byte shape check already knows.
    pub occurred_at: &'a Date,
    /// The person's exact bytes.
    pub body: &'a str,
    pub contributor: &'a ContributorId,
    /// What the caller believes this credential authorises.
    ///
    /// A **confused-deputy guard, never a selector.** The server resolves the organization from
    /// the token and compares this to it; when they disagree it refuses. Sending it is how the
    /// caller says what it thought it was writing into, so a credential that has been re-pointed
    /// is caught instead of silently writing somewhere else.
    pub expected_org: &'a OrgId,
}

/// What the store established about one append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Appended {
    /// The row did not exist and now does.
    Created { activity_id: String, org: OrgId },
    /// The row already existed and every immutable field matched. Nothing was written.
    ///
    /// Indistinguishable from `Created` in its effect on the world, which is the point: a retry
    /// after an ambiguous transport failure lands here.
    Replayed { activity_id: String, org: OrgId },
}

impl Appended {
    pub fn activity_id(&self) -> &str {
        match self {
            Self::Created { activity_id, .. } | Self::Replayed { activity_id, .. } => activity_id,
        }
    }
    pub fn org(&self) -> &OrgId {
        match self {
            Self::Created { org, .. } | Self::Replayed { org, .. } => org,
        }
    }
}

/// Why an append did not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppendFailure {
    /// The request never provably left, or its outcome could not be read.
    ///
    /// **The only variant a caller may retry**, and it may retry only the byte-identical request,
    /// because the server is idempotent on exactly that.
    #[error("the record store could not be reached: {detail}")]
    Unreachable { detail: String },
    #[error("the record store refused this append credential")]
    Unauthorized,
    /// A row with this id exists carrying different immutable content.
    ///
    /// **Permanent.** Two different notes cannot both be the promotion of one proposal, and
    /// overwriting either is the one thing an immutable record must never do.
    #[error(
        "CONFLICT: activity {activity_id} already exists with a different {differing_field} — \
         confirmed evidence is never overwritten"
    )]
    Conflict { activity_id: String, differing_field: String },
    /// The credential resolves an organization the caller did not expect.
    #[error(
        "SCOPE DRIFT: the append credential authorises {answered}, not the {asked} this \
         proposal was made in — refusing to write into another organization"
    )]
    ScopeDrift { asked: OrgId, answered: OrgId },
    #[error("the record store has no account {account} in {org}")]
    UnknownAccount { org: OrgId, account: String },
    #[error("the record store refused the request: {code}")]
    Refused { code: String },
    #[error("the record store answered in a shape this cannot read: {detail}")]
    Malformed { detail: String },
    #[error("no credential is available for {handle}")]
    NoCredential { handle: String },
    /// The handle offered was not an append credential.
    ///
    /// Unreachable through the registry, which stamps the purpose at parse. Kept because this is
    /// the boundary where a read handle would otherwise become a write one.
    #[error("{handle} is a {got:?} credential; appending requires a RecordAppend credential")]
    WrongPurpose { handle: String, got: CredentialPurpose },
}

impl AppendFailure {
    /// Whether the identical request may be sent again.
    ///
    /// Exactly one variant says yes. Every refusal the store *made* is a decision, and repeating
    /// a decided request would either change nothing or overwrite something.
    pub fn may_retry_identical(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }
}

/// The append client. Holds an endpoint and a way to resolve credentials — never a credential.
pub struct AppendClient<C: CredentialSource> {
    agent: ureq::Agent,
    base: String,
    credentials: C,
}

impl<C: CredentialSource> std::fmt::Debug for AppendClient<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppendClient").field("base", &self.base).finish_non_exhaustive()
    }
}

impl<C: CredentialSource> AppendClient<C> {
    pub fn new(base: impl Into<String>, credentials: C) -> Self {
        Self {
            agent: crate::agent(),
            base: base.into().trim_end_matches('/').to_owned(),
            credentials,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.base
    }

    /// Send one append. Idempotent at the server on the identical body.
    pub fn append(
        &self,
        credential: &CredentialRef,
        append: &Append<'_>,
    ) -> Result<Appended, AppendFailure> {
        if credential.purpose() != CredentialPurpose::RecordAppend {
            return Err(AppendFailure::WrongPurpose {
                handle: credential.handle().to_owned(),
                got: credential.purpose(),
            });
        }
        let secret: Secret = self.credentials.resolve(credential).ok_or_else(|| {
            AppendFailure::NoCredential { handle: credential.handle().to_owned() }
        })?;
        let payload = serde_json::json!({
            "activity_id": append.activity_id,
            "account_id": append.account.as_str(),
            "occurred_at": append.occurred_at.as_str(),
            "body": append.body,
            "contributor": append.contributor.as_str(),
            "expected_org": append.expected_org.as_str(),
        })
        .to_string();

        let mut response = self
            .agent
            .post(format!("{}/append_activity", self.base))
            .header("X-Service-Token", secret.expose())
            .header("content-type", "application/json")
            .send(&payload)
            .map_err(|e| AppendFailure::Unreachable { detail: e.to_string() })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            // The status arrived and the body did not: the write may well have landed, so this
            // is uncertainty and the caller may replay the identical request.
            .map_err(|e| AppendFailure::Unreachable { detail: e.to_string() })?;
        Self::read(status, &body, append)
    }

    /// The whole of this client's judgement, kept pure so every branch is testable without a
    /// socket.
    fn read(status: u16, body: &str, append: &Append<'_>) -> Result<Appended, AppendFailure> {
        if status == 401 || status == 403 {
            return Err(AppendFailure::Unauthorized);
        }
        // ── who owns "I could not read this" ─────────────────────────────────────────
        //
        // On a **success code** the answer is always ambiguity, never a decision. A `200` or
        // `201` means the store believes it wrote — so a body this cannot read leaves a request
        // that may well have landed, and the only safe classification is the one that permits
        // replaying the byte-identical request. `Malformed` is permanent and would strand a
        // committed row behind an unreadable acknowledgement.
        //
        // Below a success code the reasoning inverts: a 4xx is a decision the store made and
        // reporting it as retryable would repeat a refused request forever, so an unreadable
        // 4xx stays `Malformed`. A 5xx keeps its existing treatment — the store failed
        // mid-request and may have written — which is the same rule as the success case.
        let unreadable = |detail: String| {
            if (200..300).contains(&status) || (500..600).contains(&status) {
                AppendFailure::Unreachable { detail: format!("HTTP {status}: {detail}") }
            } else {
                AppendFailure::Malformed { detail: format!("HTTP {status}: {detail}") }
            }
        };
        let parsed: serde_json::Value =
            serde_json::from_str(body).map_err(|e| unreadable(e.to_string()))?;
        let field = |k: &str| parsed.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let org = || OrgId::parse(field("org")).map_err(|e| unreadable(e.to_string()));
        match status {
            201 | 200 => {
                // An organization this cannot parse — absent, empty, or not an org id — is part
                // of an unreadable acknowledgement and goes through `unreadable` above, so it is
                // retryable. A VALID one naming somewhere else is a different fact: the store
                // told us plainly where it wrote, and that is drift, not ambiguity.
                let org = org()?;
                if &org != append.expected_org {
                    // Belt and braces: the server compares this too, and a store that answered
                    // for another org while reporting success is one this must not believe.
                    return Err(AppendFailure::ScopeDrift {
                        asked: append.expected_org.clone(),
                        answered: org,
                    });
                }
                // A success has to be a success ABOUT THE REQUEST. An acknowledgement naming a
                // different activity, or a `status` that disagrees with the HTTP code, is one
                // this cannot turn into a truthful `Appended` — and it is **ambiguous**, not a
                // refusal, because the write may well have landed. So it reads as unreachable,
                // which is the one class that permits replaying the byte-identical request; the
                // store's own idempotency then settles what actually happened.
                let acknowledged = field("activity_id");
                let claimed = field("status");
                let expected_status = if status == 201 { "created" } else { "replayed" };
                if acknowledged != append.activity_id || claimed != expected_status {
                    return Err(AppendFailure::Unreachable {
                        detail: format!(
                            "HTTP {status} acknowledged activity {acknowledged:?} as \
                             {claimed:?}, but the request was {:?} and {status} means \
                             {expected_status:?} — the outcome is unreadable, so the identical \
                             request stays replayable",
                            append.activity_id
                        ),
                    });
                }
                let activity_id = acknowledged.to_owned();
                Ok(if status == 201 {
                    Appended::Created { activity_id, org }
                } else {
                    Appended::Replayed { activity_id, org }
                })
            }
            404 if field("error") == "unknown_account" => Err(AppendFailure::UnknownAccount {
                org: org()?,
                account: append.account.as_str().to_owned(),
            }),
            409 if field("error") == "org_mismatch" => Err(AppendFailure::ScopeDrift {
                asked: append.expected_org.clone(),
                answered: org()?,
            }),
            409 => Err(AppendFailure::Conflict {
                activity_id: append.activity_id.to_owned(),
                differing_field: match field("differing_field") {
                    "" => "field".to_owned(),
                    named => named.to_owned(),
                },
            }),
            400 => Err(AppendFailure::Refused { code: field("error").to_owned() }),
            // A 429 is the one 4xx that is not a decision about the request: the store is asking
            // for the same request later, which is exactly what `Unreachable` licenses.
            429 => Err(AppendFailure::Unreachable {
                detail: format!("HTTP 429: {}", body.chars().take(200).collect::<String>()),
            }),
            // Every other 4xx is a DECISION, even one this build has no branch for. A 404 from a
            // mistyped endpoint, a 405, a 410, a 422 — none of them becomes true by being sent
            // again, and classifying them as unreachable made the caller replay a refused request
            // on every `/confirm`, forever. `Refused` names the store's own code where it gave
            // one and the status where it did not.
            s if (400..500).contains(&s) => Err(AppendFailure::Refused {
                code: match field("error") {
                    "" => format!("HTTP {s}"),
                    named => named.to_owned(),
                },
            }),
            // Everything else, 5xx included: the store did not decide, so neither does this.
            _ => Err(AppendFailure::Unreachable {
                detail: format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()),
            }),
        }
    }
}

#[cfg(test)]
mod tests;
