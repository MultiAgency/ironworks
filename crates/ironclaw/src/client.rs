//! The IronClaw client.

use crate::classify::classify;
use crate::model::{Accepted, ErrorEnvelope, IdempotencyKey, Outcome, ResponseId, ResponseObject};
use crate::{Cause, Deadline, Runtime, TurnFailure, model};
use std::time::Duration;

/// How many times a **provably unsent** request may be re-sent.
///
/// Safe by construction: a request that never left cannot have executed. An ambiguous failure is
/// never retried here or anywhere above.
const UNSENT_RETRIES: u32 = 3;

/// A credential. Redacted in `Debug` and it implements no `Display`, and the only way out is
/// `Credential::authorization_header` — so leaking it takes deliberate work rather than a stray
/// `{:?}` in a log line.
#[derive(Clone)]
pub struct Credential(String);

impl Credential {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The one sanctioned way to *use* this value: as an `Authorization` header.
    ///
    /// `expose` stays private, and this is the reason it can. The two request builders below both
    /// wrote `format!("Bearer {}", …expose())` inline, so the secret left its newtype twice at
    /// call sites — while `crates/records/src/credential.rs::Secret`,
    /// `crates/channel-http/src/lib.rs::AdapterToken` and `crates/reconcile/src/lib.rs::OperatorToken`
    /// all keep exactly this method for exactly this reason. A caller receives a header it can
    /// only send, never the credential to store, log or compare.
    fn authorization_header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential(redacted)")
    }
}

/// A client for one tenant's identity against one runtime.
pub struct Ironclaw {
    agent: ureq::Agent,
    base: String,
    credential: Credential,
}

impl std::fmt::Debug for Ironclaw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ironclaw").field("base", &self.base).finish_non_exhaustive()
    }
}

impl Ironclaw {
    /// Build a client.
    ///
    /// The timeout configuration is load-bearing, not tuning. Every timeout is **per phase** so
    /// that `ureq::Error::Timeout` names where it happened, which is the only way to tell a
    /// provably-unsent failure from an ambiguous one. A `timeout_global` here would collapse
    /// both into one indistinguishable error and force every connect failure to block an update
    /// for operator reconciliation. See ADR 0004; do not "simplify" this.
    pub fn new(base: impl Into<String>, credential: Credential) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(None)
            .timeout_per_call(None)
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(10)))
            // **Both must exceed the runtime's own 30s wait, or we never hear its answer.**
            // `create` blocks server-side under `tokio::time::timeout(DEFAULT_RESPONSES_WAIT_
            // TIMEOUT, wait_for_response_completion(..))` and maps elapse to `503
            // service_unavailable` (pinned source, `responses_workflow.rs:327`). With a body on
            // the request, ureq accounts that wait to the SEND phase, not `RecvResponse` — so a
            // 30s send budget raced the server's 30s wait and we reported `SendTimeout` for a
            // write that had completed in 0.3ms, on a turn the runtime had definitively refused.
            //
            // Measured both ways on the same question: at 30s, "timed out writing the request";
            // at 90s, "the runtime refused it: service_unavailable ... [server_error]" — same
            // 30.6s, same `Unknown` classification, same recovery. Only the diagnostic changed,
            // from false to true. Recovery is unaffected either way because both classify as
            // `Unknown`: a 503 arrives after submission, so the turn may still be running.
            //
            // These are not a turn budget. They bound one HTTP phase each (ADR 0004), and the
            // long turn is recovered by replaying the idempotency key, never by waiting longer.
            .timeout_send_request(Some(Duration::from_secs(90)))
            .timeout_send_body(Some(Duration::from_secs(90)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            // A 409 is information and a 200 can carry `status: "failed"`, so HTTP status is
            // never the success signal here. The body is interpreted instead.
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            base: base.into().trim_end_matches('/').to_owned(),
            credential,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

/// One HTTP exchange, reduced to a status and a body.
struct Raw {
    status: u16,
    body: String,
}

fn read(mut resp: ureq::http::Response<ureq::Body>) -> Result<Raw, TurnFailure> {
    let status = resp.status().as_u16();
    match resp.body_mut().read_to_string() {
        Ok(body) => Ok(Raw { status, body }),
        // The response headers arrived, so the turn certainly ran. Failing to read the body
        // tells us nothing about that.
        Err(e) => Err(TurnFailure::Unknown(Cause::Undecodable(e.to_string()))),
    }
}

/// Interpret a non-2xx answer.
///
/// Two encodings, both measured: `401` is `text/plain` (`Invalid or missing auth token`), while
/// `400`, `404` and `409` are JSON-enveloped. A parser that assumes one shape breaks on the
/// most common failure there is.
fn interpret_error(raw: &Raw, sent: bool) -> TurnFailure {
    let wrap = |c: Cause| if sent { TurnFailure::Unknown(c) } else { TurnFailure::NotSent(c) };

    if raw.status == 401 || raw.status == 403 {
        // Rejected before the handler saw it, so nothing ran — whatever the body shape.
        return TurnFailure::NotSent(Cause::Unauthorized);
    }
    let envelope: Option<ErrorEnvelope> = serde_json::from_str(&raw.body).ok();
    let (code, message) = match &envelope {
        Some(e) => {
            // `param` names the offending field and `type` the error family. Both are the
            // difference between "the runtime refused it" and a message an operator can act
            // on, so they go into the text rather than being parsed and dropped.
            let mut message = e.error.message.clone().unwrap_or_default();
            if let Some(param) = &e.error.param {
                message.push_str(&format!(" (field: {param})"));
            }
            if let Some(kind) = &e.error.kind {
                message.push_str(&format!(" [{kind}]"));
            }
            (e.error.code.clone().unwrap_or_default(), message)
        }
        None => (String::new(), raw.body.trim().to_owned()),
    };
    if raw.status == 409 || code == "conflict" {
        // The key was replayed against a different body. A turn under this key already exists —
        // it is emphatically not unsent.
        return TurnFailure::Unknown(Cause::IdempotencyConflict);
    }
    if raw.status == 400 {
        // Rejected on validation; the handler never dispatched a turn.
        return TurnFailure::NotSent(Cause::Rejected { code, message });
    }
    if raw.status == 429 {
        // **`NotSent` whatever `sent` says.** A 429 is an admission rejection, so the turn was
        // never considered — and the previous fall-through made it `Unknown(Rejected)` on create,
        // which says "a turn MAY have run; it must not be repeated". That is the expensive
        // direction of the error `0050` found in the other one: it strands a turn nothing ran,
        // and a loop of short invocations against a 30-request budget (§G-6) meets it routinely.
        return TurnFailure::NotSent(Cause::RateLimited);
    }
    wrap(Cause::Rejected { code, message })
}

impl Runtime for Ironclaw {
    fn create(
        &self,
        turn: &ironworks_domain::ModelTurn,
        key: &IdempotencyKey,
        deadline: &Deadline,
    ) -> Result<Accepted, TurnFailure> {
        // The same function the journal fingerprints, so the digest describes what is sent.
        let body = crate::model::request_bytes(turn)
            .map_err(|e| TurnFailure::NotSent(Cause::Undecodable(e.to_string())))?;
        let body = String::from_utf8(body)
            .map_err(|e| TurnFailure::NotSent(Cause::Undecodable(e.to_string())))?;

        let mut attempt = 0;
        loop {
            if deadline.expired() {
                return Err(TurnFailure::NotSent(Cause::BudgetExhausted));
            }
            let sent = self
                .agent
                .post(self.url("/api/v1/responses"))
                .header("Authorization", &self.credential.authorization_header())
                .header("Content-Type", "application/json")
                .header("Idempotency-Key", key.as_str())
                .send(body.as_str());

            let failure = match sent {
                Ok(resp) => {
                    let raw = read(resp)?;
                    if (200..300).contains(&raw.status) {
                        let doc: ResponseObject = serde_json::from_str(&raw.body)
                            .map_err(|e| TurnFailure::Unknown(Cause::Undecodable(e.to_string())))?;
                        let Some(id) = doc.id.clone().and_then(|s| ResponseId::parse(s).ok())
                        else {
                            // A 2xx with no id: the turn may have run and we have no handle to
                            // it. This is the worst outcome the protocol can produce.
                            return Err(TurnFailure::Unknown(Cause::Undecodable(
                                "the runtime accepted a turn but named no response id".into(),
                            )));
                        };
                        let reported_model =
                            ironworks_domain::ReportedModel::read(doc.model.as_deref());
                        return Ok(Accepted {
                            id,
                            outcome: model::to_outcome(doc),
                            reported_model,
                        });
                    }
                    interpret_error(&raw, true)
                }
                Err(e) => classify(&e),
            };

            // Only a provably-unsent failure may be re-sent. Everything else returns.
            attempt += 1;
            if !failure.provably_unsent() || attempt > UNSENT_RETRIES {
                return Err(failure);
            }
        }
    }

    fn fetch(&self, id: &ResponseId, deadline: &Deadline) -> Result<Outcome, TurnFailure> {
        if deadline.expired() {
            return Err(TurnFailure::Unknown(Cause::BudgetExhausted));
        }
        let resp = self
            .agent
            .get(self.url(&format!("/api/v1/responses/{id}")))
            .header("Authorization", &self.credential.authorization_header())
            .call()
            .map_err(|e| {
                // A read is idempotent, so "unsent" carries no risk here — but the caller only
                // ever acts on `provably_unsent` for retries, so the honest classification is
                // still the right one.
                classify(&e)
            })?;
        let raw = read(resp)?;
        if !(200..300).contains(&raw.status) {
            return Err(interpret_error(&raw, false));
        }
        let doc: ResponseObject = serde_json::from_str(&raw.body)
            .map_err(|e| TurnFailure::Unknown(Cause::Undecodable(e.to_string())))?;
        Ok(model::to_outcome(doc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(status: u16, body: &str) -> Raw {
        Raw { status, body: body.into() }
    }

    #[test]
    fn a_credential_does_not_leak_through_debug() {
        let c = Credential::new("sk-super-secret");
        assert_eq!(format!("{c:?}"), "Credential(redacted)");
        assert!(!format!("{c:?}").contains("secret"));
    }

    #[test]
    fn a_client_does_not_leak_its_credential_either() {
        let c = Ironclaw::new("http://127.0.0.1:1", Credential::new("sk-secret"));
        assert!(!format!("{c:?}").contains("secret"));
    }

    #[test]
    fn a_plain_text_401_is_understood_and_is_never_ambiguous() {
        // Measured: 401 is text/plain, not JSON. Nothing ran, so it is safe to say so.
        let f = interpret_error(&raw(401, "Invalid or missing auth token"), true);
        assert_eq!(f, TurnFailure::NotSent(Cause::Unauthorized));
    }

    #[test]
    fn a_409_conflict_is_ambiguous_because_a_turn_already_exists_under_that_key() {
        let body = r#"{"error":{"message":"The request conflicts with the current resource state.",
            "type":"conflict_error","param":"idempotency_key","code":"conflict"}}"#;
        let f = interpret_error(&raw(409, body), true);
        assert_eq!(f, TurnFailure::Unknown(Cause::IdempotencyConflict));
        assert!(!f.provably_unsent(), "a conflict must never be retried");
    }

    #[test]
    fn a_400_is_provably_unsent_because_validation_precedes_dispatch() {
        let body = r#"{"error":{"message":"The request is invalid.",
            "type":"invalid_request_error","param":"body","code":"invalid_request"}}"#;
        let f = interpret_error(&raw(400, body), true);
        assert!(f.provably_unsent());
        match f.cause() {
            Cause::Rejected { code, .. } => assert_eq!(code, "invalid_request"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_429_is_provably_unsent_and_blocks_a_measurement() {
        // Admission rejection: the turn was never considered, so it is safe to retry under the
        // same key. This used to fall through to `Unknown(Rejected)` on create — "a turn MAY have
        // run" — which strands a turn that nothing ran, on the one failure a loop of short
        // invocations meets routinely (§G-6: 30 requests / 60 seconds, per user).
        let body = r#"{"error":{"message":"Too many requests.",
            "type":"rate_limit_error","code":"rate_limited"}}"#;
        for sent in [true, false] {
            let f = interpret_error(&raw(429, body), sent);
            assert_eq!(f, TurnFailure::NotSent(Cause::RateLimited), "sent={sent}");
            assert!(f.provably_unsent(), "a rate-limited turn is safe to retry");
            // BLOCKED, not FAIL: nothing was established about the request.
            assert!(f.cause().blocks_measurement());
        }
    }

    #[test]
    fn a_non_json_error_body_still_yields_a_usable_cause() {
        let f = interpret_error(&raw(503, "Database unavailable"), true);
        assert!(!f.provably_unsent());
        assert!(f.to_string().contains("Database unavailable"));
    }

    #[test]
    fn a_url_is_built_without_a_double_slash() {
        let c = Ironclaw::new("http://h:1/", Credential::new("t"));
        assert_eq!(c.url("/api/v1/responses"), "http://h:1/api/v1/responses");
    }

    /// The one live-ish check that needs no server: a refused connection must classify as
    /// provably unsent, so the caller may safely retry rather than blocking the update.
    #[test]
    fn a_refused_connection_is_provably_unsent() {
        // Port 1 on loopback refuses immediately on every platform we target.
        let c = Ironclaw::new("http://127.0.0.1:1", Credential::new("t"));
        let d = Deadline::starting_now(Duration::from_secs(5));
        let id = ResponseId::parse("resp_x").unwrap();
        let err = c.fetch(&id, &d).unwrap_err();
        assert!(err.provably_unsent(), "got {err:?}");
    }
}
