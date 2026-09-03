//! The shapes that cross the wire, and the domain shapes they map to.
//!
//! Wire types tolerate unknown fields: the runtime may add one at a pin bump, and refusing to
//! parse would take every tenant down over an addition. That is the opposite of the rule for
//! compiled-in configuration, where an unknown key is a typo and must fail loudly.

use ironworks_domain::ModelTurn;
pub use ironworks_domain::{IdempotencyKey, InvalidKey, ResponseId};
use serde::{Deserialize, Serialize};

/// The request body, exactly as it goes out.
///
/// Built only from a [`ModelTurn`], which only a verified service instance can produce — so the
/// serving path cannot be entered with an unverified scope.
///
/// Field order here **is** serialization order, which is what makes the body byte-stable and
/// crash recovery possible (ADR 0005). Do not reorder, and never introduce a `HashMap`.
#[derive(Debug, Serialize)]
pub(crate) struct CreateBody<'a> {
    pub model: &'a str,
    pub instructions: &'a str,
    pub input: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<&'a str>,
}

/// The exact bytes this crate will send for a turn.
///
/// Public because the journal fingerprints the request, and a digest taken over a *different*
/// serialisation would prove nothing about what actually went out. One function, one answer.
pub fn request_bytes(turn: &ModelTurn) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&CreateBody::from(turn))
}

impl<'a> From<&'a ModelTurn> for CreateBody<'a> {
    fn from(t: &'a ModelTurn) -> Self {
        Self {
            model: t.identity().model().as_str(),
            instructions: t.instructions().as_str(),
            input: t.input().as_str(),
            previous_response_id: t.previous().map(|p| p.response().as_str()),
        }
    }
}

/// A turn the runtime has accepted. It may already be finished.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    /// Persist this **before** doing anything else with it.
    pub id: ResponseId,
    pub outcome: Outcome,
}

/// One call the model made which the runtime handed back rather than executing.
///
/// IronClaw parks a run on a client-declared function tool: *"The model may call it; the host
/// never executes it."* The caller runs it and resumes by submitting a `function_call_output`
/// carrying the same [`ToolCall::call_id`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    /// Raw JSON as the model produced it. **Model-generated text**, so it is not journalled — a
    /// recovering process re-reads it from the runtime by response id.
    pub arguments: String,
}

/// Where a turn has got to.
///
/// `Failed` sits in the success channel deliberately: the turn definitively ran and produced no
/// answer, which is a terminal fact the caller acts on — not a transport problem it might retry.
/// Only [`super::TurnFailure`] means "we could not find out".
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Queued or in progress. Poll again.
    Running,
    Completed {
        text: String,
        usage: Option<Usage>,
    },
    /// The turn reported `completed` and is **waiting on us**: at least one `function_call` in its
    /// output has no `function_call_output` answering it.
    ///
    /// **This variant exists because the status does not distinguish it.** Measured at 1.4.0
    /// (`docs/experiments/0032-a-parked-run-says-completed.md`): a parked run reports
    /// `status: "completed"` with a `function_call` item carrying no `content`, so the previous
    /// mapping produced `Completed { text: "" }` — an empty answer, journalled as a finished turn,
    /// with a thread pointer committed and nothing recording that a call was outstanding. Polling
    /// will never change it; only submitting the outputs will.
    Parked {
        calls: Vec<ToolCall>,
        usage: Option<Usage>,
    },
    /// The turn ran and errored. The message is the runtime's sanitized one — it deliberately
    /// carries no provider detail, so it is safe to log but useless for diagnosis.
    Failed {
        code: String,
        message: String,
    },
    Cancelled,
    /// Stopped early. Any partial output is preserved rather than discarded.
    Incomplete {
        text: String,
    },
}

impl Outcome {
    /// Whether polling can still change this.
    ///
    /// [`Self::Parked`] is terminal: the run reported `completed` and no amount of `GET` will move
    /// it. What moves it is submitting the tool outputs, which is a different operation on a
    /// different path — so a poll loop must stop here rather than spin.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Running)
    }

    /// Whether this outcome holds an answer a person may be shown.
    ///
    /// `Parked` does not, and neither does a failure. Written as its own predicate because
    /// "terminal" and "answered" stopped being the same question the moment a parked run started
    /// reporting `completed`.
    pub fn has_answer(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Incomplete { .. })
    }
}

/// Token and cost accounting, best-effort.
///
/// Token counts are present and correct on both create and fetch at 1.4.0.
///
/// # `total_cost_usd` does not describe the pinned model, in either direction
///
/// It is an IronClaw extension computed from *its own* per-model table, and measured 2026-09-02
/// (`docs/experiments/0040-the-cost-field-misses-the-pin-twice.md`) that table misses
/// our pin twice over, for two unrelated reasons:
///
/// - **On create** it prices the model the request named. `qwen/…` is classified as a free local
///   model by a name heuristic — the table's `is_local_model` matches a leading `qwen`, alongside
///   `llama`, `mistral`, `deepseek` and others — so a turn on the pin prices at exactly **`"0"`**.
/// - **On fetch** the model has become the literal `"reborn"` (§G-2), which the table does not
///   know, so it falls back to its default entry and prices the same turn at **GPT-4o's rates**
///   ($2.50/Mtok in, $10.00/Mtok out).
///
/// A model the table *does* know prices correctly on create — measured with `model: "default"`,
/// which is neither local-prefixed nor a table entry and duly came back at the default rates. So
/// this is a fact about the pin's name, not about the create path.
///
/// The cache-read discount is keyed the same way (Anthropic 10×, OpenAI 2×, everything else none),
/// so `qwen/…` gets no discount — which is why `0001`'s "cached costs the same as fresh" holds
/// here. §G-7's *never reconcile spend from it* is the rule; this is why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_cost_usd: Option<String>,
}

// ── wire shapes ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ResponseObject {
    pub id: Option<String>,
    pub status: Option<WireStatus>,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    #[serde(default)]
    pub error: Option<WireError>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Incomplete,
}

/// One item of a response's `output`.
///
/// The runtime replays the **whole step history** here on a resumed run — the original call, the
/// output submitted for it, any call the runtime made on its own behalf, and finally the message —
/// so this type must carry enough to tell those apart. Measured in
/// `docs/experiments/0032-a-parked-run-says-completed.md`.
#[derive(Debug, Deserialize)]
pub(crate) struct OutputItem {
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// The model's arguments, as the JSON *string* the runtime sends. Kept unparsed: this crate
    /// does not know any tool's schema, and parsing here would be an opinion about one.
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub content: serde_json::Value,
}

impl OutputItem {
    fn is(&self, kind: &str) -> bool {
        self.kind.as_deref() == Some(kind)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireError {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Present on the 4xx envelope (`invalid_request_error`, `conflict_error`, …) and absent on
    /// the in-band error attached to a `status: "failed"` response.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub param: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub input_tokens_details: Option<WireTokenDetails>,
    #[serde(default)]
    pub cost: Option<WireCost>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WireCost {
    #[serde(default)]
    pub total_cost_usd: Option<String>,
}

/// The 4xx/5xx envelope. Only present when the body is JSON — a `401` is `text/plain`.
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorEnvelope {
    pub error: WireError,
}

impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Self {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cached_tokens: w.input_tokens_details.map(|d| d.cached_tokens).unwrap_or(0),
            total_cost_usd: w.cost.and_then(|c| c.total_cost_usd),
        }
    }
}

/// Flatten output items into the answer text.
///
/// Content arrives as an array of `{type, text}` parts. Anything without a `text` is skipped
/// rather than rendered — a tool call is not an answer, and stringifying it would put JSON in
/// front of a person.
/// A completed turn whose text is not an answer.
///
/// One signal is used and deliberately only one: a model's own control tokens (`<|im_start|>`,
/// `<|endoftext|>`, and the rest of that family) reaching output text. A runtime that emits them
/// has stopped producing prose, and no answer about a tenant's accounts contains them — so this
/// is a gate rather than a heuristic, which is what it has to be. A false positive here withholds
/// a good answer from a tenant, and withholding silently is the failure a looser test would buy.
///
/// Degenerate text carrying no control token is therefore **not** caught. Word-ratio and entropy
/// tests catch more of it and misjudge short, list-shaped and non-English answers, which is a
/// trade a delivery gate cannot make. This detects the one shape that is unambiguous.
pub fn malformed_answer(text: &str) -> Option<&'static str> {
    // A token is short by construction; bounding the span stops an unrelated `<|` and a later
    // `|>` from pairing across half a document.
    const SPAN: usize = 32;
    let mut from = 0;
    while let Some(i) = text[from..].find("<|") {
        let open = from + i + 2;
        let window = &text[open..text.len().min(open + SPAN)];
        if window.contains("|>") {
            return Some("a model control token");
        }
        from = open;
    }
    None
}

/// The calls in `output` that nothing has answered yet.
///
/// **Pairing, not position.** A resumed run replays every earlier call *and* its output, so "the
/// last item is a call" is wrong and "any call exists" is wronger — both would report a finished
/// turn as parked forever. A call is outstanding when no `function_call_output` carries its
/// `call_id`, which is the only reading that survives the replayed history.
///
/// Calls the runtime makes on its own behalf are paired in the same output (measured: it issues
/// `builtin__result_read` and its result together), so they never appear here — which is correct,
/// because nothing outside the runtime could answer one.
pub(crate) fn outstanding_calls(items: &[OutputItem]) -> Vec<ToolCall> {
    let answered: std::collections::BTreeSet<&str> = items
        .iter()
        .filter(|i| i.is("function_call_output"))
        .filter_map(|i| i.call_id.as_deref())
        .collect();
    items
        .iter()
        .filter(|i| i.is("function_call"))
        .filter_map(|i| {
            let call_id = i.call_id.as_deref()?;
            (!answered.contains(call_id)).then(|| ToolCall {
                call_id: call_id.to_owned(),
                name: i.name.clone().unwrap_or_default(),
                arguments: i.arguments.clone().unwrap_or_default(),
            })
        })
        .collect()
}

pub(crate) fn output_text(items: &[OutputItem]) -> String {
    let mut parts = Vec::new();
    for item in items {
        match &item.content {
            serde_json::Value::String(s) => parts.push(s.clone()),
            serde_json::Value::Array(list) => {
                for p in list {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                        parts.push(t.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    parts.join("\n").trim().to_owned()
}

pub(crate) fn to_outcome(doc: ResponseObject) -> Outcome {
    let text = output_text(&doc.output);
    let usage = doc.usage.map(Usage::from);
    match doc.status {
        Some(WireStatus::Queued | WireStatus::InProgress) | None => Outcome::Running,
        Some(WireStatus::Completed) => match outstanding_calls(&doc.output) {
            calls if calls.is_empty() => Outcome::Completed { text, usage },
            calls => Outcome::Parked { calls, usage },
        },
        Some(WireStatus::Cancelled) => Outcome::Cancelled,
        Some(WireStatus::Incomplete) => Outcome::Incomplete { text },
        Some(WireStatus::Failed) => Outcome::Failed {
            code: doc
                .error
                .as_ref()
                .and_then(|e| e.code.clone())
                .unwrap_or_else(|| "unknown".into()),
            message: doc
                .error
                .and_then(|e| e.message)
                .unwrap_or_else(|| "the runtime reported a failure with no message".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `ModelTurn`, built the only way one can be: through a verified service
    /// instance melting its own composition. Heavier than a literal, and deliberately so — this
    /// asserts on what the domain actually produces, not on a shape invented here.
    fn turn_for(previous: Option<&str>) -> ironworks_domain::ModelTurn {
        use ironworks_domain::*;
        use std::collections::BTreeMap;

        struct P(BTreeMap<String, String>);
        impl PartSource for P {
            fn read(&self, r: &PartRef) -> Option<&str> {
                self.0.get(r.as_str()).map(String::as_str)
            }
        }
        struct S;
        impl ScopeResolver for S {
            fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
                Ok(ScopeEvidence::from_record_store(
                    OrgId::parse("org").unwrap(),
                    "http://store.example",
                ))
            }
        }

        let parts = P(BTreeMap::from([
            ("p".to_string(), "PART".to_string()),
            ("t".to_string(), "TAIL".to_string()),
        ]));
        let definition = ServiceDefinition::new(
            ServiceId::parse("s").unwrap(),
            1,
            "r",
            Composition::new(vec![PartRef::new("p")], "# H", PartRef::new("t")).unwrap(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
        )
        .unwrap();
        let guidance = Guidance::parse(
            &format!(
                "<!-- tenant-guidance v1 slug: acme service: s -->\n\n{}",
                "content ".repeat(80)
            ),
            "acme",
            "s",
        )
        .unwrap();
        let instance = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition,
            guidance,
            RuntimeIdentity::parse("m").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "a"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "b")).unwrap(),
            ModelSelection::from_pin(&ModelPin::parse("M # c").unwrap()),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
            &parts,
        )
        .unwrap()
        .verify(&S)
        .unwrap();
        let prev = previous.map(|r| instance.thread_scope().pointer(ResponseId::parse(r).unwrap()));
        instance
            // No records and a complete book: the envelope renders to the question verbatim, so
            // the sentinel below still asserts what it always did — that the input lands in the
            // right JSON field, unaltered.
            .build_turn(
                &parts,
                &ironworks_domain::Ask {
                    question: "ENVELOPE",
                    records: &[],
                    records_state: &RecordsState::Complete,
                    retrieved_at: &ironworks_domain::record::Timestamp::parse(
                        "2026-08-27T12:00:00+00:00",
                    )
                    .unwrap(),
                    speaker: None,
                    fact_fields: None,
                },
                prev,
            )
            .unwrap()
    }

    /// Crash recovery replays an idempotency key against a rebuilt body, and a differing body is
    /// refused 409 (measured). Byte-stability is therefore a correctness property.
    #[test]
    fn the_request_body_serializes_deterministically_and_in_declaration_order() {
        let turn = turn_for(None);
        let body = serde_json::to_string(&CreateBody::from(&turn)).unwrap();
        for _ in 0..50 {
            assert_eq!(serde_json::to_string(&CreateBody::from(&turn)).unwrap(), body);
        }
        let m = body.find("\"model\"").unwrap();
        let i = body.find("\"instructions\"").unwrap();
        let n = body.find("\"input\"").unwrap();
        assert!(m < i && i < n, "declaration order is serialization order: {body}");
        assert!(body.starts_with(r#"{"model":"M","instructions":"PART"#));
        assert!(body.contains(r#""input":"ENVELOPE""#));
    }

    #[test]
    fn an_absent_thread_pointer_is_omitted_not_null() {
        // `"previous_response_id": null` and an absent key are different bodies, and only one
        // replays cleanly against a key minted before a crash.
        let none = serde_json::to_string(&CreateBody::from(&turn_for(None))).unwrap();
        assert!(!none.contains("previous_response_id"));
        let some = serde_json::to_string(&CreateBody::from(&turn_for(Some("resp_1")))).unwrap();
        assert!(some.ends_with(r#","previous_response_id":"resp_1"}"#), "{some}");
    }

    #[test]
    fn every_status_maps_to_an_outcome() {
        for (wire, terminal) in [
            (r#""queued""#, false),
            (r#""in_progress""#, false),
            (r#""completed""#, true),
            (r#""failed""#, true),
            (r#""cancelled""#, true),
            (r#""incomplete""#, true),
        ] {
            let raw = format!(r#"{{"id":"r","status":{wire},"output":[]}}"#);
            let doc: ResponseObject = serde_json::from_str(&raw).unwrap();
            assert_eq!(to_outcome(doc).is_terminal(), terminal, "{wire}");
        }
    }

    #[test]
    fn a_missing_status_reads_as_running_never_as_success() {
        let doc: ResponseObject = serde_json::from_str(r#"{"id":"r","output":[]}"#).unwrap();
        assert_eq!(to_outcome(doc), Outcome::Running);
    }

    #[test]
    fn a_control_token_in_the_output_is_not_an_answer() {
        // Verbatim from 0016-attention-frozen-graders.json run 1, which the coordination graders
        // scored as a reasoning failure because they can only ask what an answer says, never
        // whether it is one.
        let soup = "9.8339.|14132 #\", sp/ sp.2.\n:0,542.2. (.3492::2ER6: with:,| =4.23                     as and.com.20 *32ER.227:,/ER24 in.<|im_start|>.33 with{2. sp<|repo_name|>,02525";
        assert_eq!(malformed_answer(soup), Some("a model control token"));
    }

    #[test]
    fn an_ordinary_briefing_is_left_alone() {
        // Punctuation this service actually emits: em-dashes, pipes in tables, angle brackets.
        let real = "**Cedar Harbor Studio** — the pilot ends 2026-09-10 — FACT, 2026-08-11 \
                    agreement. | Account | Issue | and 3 < 5 > 1, no record shows a signed \
                    extension.";
        assert_eq!(malformed_answer(real), None);
    }

    #[test]
    fn an_unpaired_marker_does_not_pair_across_the_answer() {
        // `<|` early and `|>` far later are not a token, and treating them as one would withhold
        // a good answer — the direction this gate must not fail in.
        let far = format!("a <| b {} c |> d", "x".repeat(200));
        assert_eq!(malformed_answer(&far), None);
    }

    #[test]
    fn a_token_at_the_very_end_is_still_found() {
        assert_eq!(
            malformed_answer("the answer ends <|endoftext|>"),
            Some("a model control token")
        );
    }

    #[test]
    fn answer_text_comes_out_of_the_content_parts() {
        let raw = r#"{"id":"r","status":"completed",
            "output":[{"content":[{"text":"ping","type":"output_text"}]}]}"#;
        let doc: ResponseObject = serde_json::from_str(raw).unwrap();
        match to_outcome(doc) {
            Outcome::Completed { text, .. } => assert_eq!(text, "ping"),
            other => panic!("{other:?}"),
        }
    }

    /// The exact bytes a parked run returned, from
    /// `docs/experiments/0032-a-parked-run-says-completed.md`. Copied rather than paraphrased: a
    /// hand-written shape would assert against what we believe the runtime sends, which is the
    /// belief that was wrong in the first place.
    const MEASURED_PARKED: &str = r#"{"id":"resp_d5b1cbde157484f88610b021f334fe2","object":"response",
        "created_at":1788321081,"status":"completed","model":"qwen/qwen3.5-397b-a17b",
        "output":[{"type":"function_call","id":"fc_call_f92f187e5d584262bbf204d7","status":"completed",
                   "call_id":"call_f92f187e5d584262bbf204d7","name":"get_balance",
                   "arguments":"{\"account_id\":\"ACME-001\"}"}]}"#;

    /// The resumed run, same source: the whole step history replayed, ending in a message.
    const MEASURED_RESUMED: &str = r#"{"id":"resp_c297793cd0be4b67a9df250784689df2","status":"completed",
        "output":[
          {"type":"function_call","call_id":"call_f92f","name":"get_balance","arguments":"{}"},
          {"type":"function_call_output","call_id":"call_f92f","output":{"trust":"untrusted_tool_output"}},
          {"type":"function_call","call_id":"call_a51f","name":"builtin__result_read","arguments":"{}"},
          {"type":"function_call_output","call_id":"call_a51f","output":{"trust":"untrusted_tool_output"}},
          {"type":"message","role":"assistant",
           "content":[{"text":"The current ledger balance is $41,250.00 USD.","type":"output_text"}]}]}"#;

    #[test]
    fn a_parked_run_is_not_read_as_a_completed_one() {
        // The defect this variant exists to remove: `status: completed`, no message, and the old
        // mapping produced `Completed { text: "" }` — an empty answer with a call outstanding.
        let doc: ResponseObject = serde_json::from_str(MEASURED_PARKED).unwrap();
        match to_outcome(doc) {
            Outcome::Parked { calls, .. } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "get_balance");
                assert_eq!(calls[0].call_id, "call_f92f187e5d584262bbf204d7");
                assert_eq!(calls[0].arguments, r#"{"account_id":"ACME-001"}"#);
            }
            other => panic!("a parked run must not read as {other:?}"),
        }
    }

    #[test]
    fn a_resumed_run_is_answered_despite_replaying_every_call() {
        // The other half, and the one a position-based rule would fail: the resumed body contains
        // two `function_call` items, both answered. Reading "any call present" as parked would
        // leave a finished turn parked forever, and the runtime's own `builtin__result_read` call
        // would park a turn on a tool no caller could ever execute.
        let doc: ResponseObject = serde_json::from_str(MEASURED_RESUMED).unwrap();
        match to_outcome(doc) {
            Outcome::Completed { text, .. } => {
                assert_eq!(text, "The current ledger balance is $41,250.00 USD.");
            }
            other => panic!("a resumed run must read as completed, got {other:?}"),
        }
    }

    #[test]
    fn only_an_answered_outcome_may_be_shown_to_a_person() {
        let parked: ResponseObject = serde_json::from_str(MEASURED_PARKED).unwrap();
        assert!(!to_outcome(parked).has_answer());
        let resumed: ResponseObject = serde_json::from_str(MEASURED_RESUMED).unwrap();
        assert!(to_outcome(resumed).has_answer());
        // And a parked run stops the poll loop rather than spinning it: nothing about `GET` moves
        // a run that is waiting on us.
        let parked: ResponseObject = serde_json::from_str(MEASURED_PARKED).unwrap();
        assert!(to_outcome(parked).is_terminal());
    }

    #[test]
    fn output_without_text_is_skipped_not_stringified() {
        // A tool call is not an answer; rendering its JSON would put braces in front of a person.
        let raw = r#"{"id":"r","status":"completed","output":[
            {"content":[{"type":"function_call","name":"x"}]},
            {"content":[{"text":"real answer","type":"output_text"}]}]}"#;
        let doc: ResponseObject = serde_json::from_str(raw).unwrap();
        match to_outcome(doc) {
            Outcome::Completed { text, .. } => assert_eq!(text, "real answer"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failed_turn_carries_the_sanitized_code_and_message() {
        let raw = r#"{"id":"r","status":"failed","output":[],
            "error":{"code":"internal_error","message":"An internal error occurred."}}"#;
        let doc: ResponseObject = serde_json::from_str(raw).unwrap();
        assert_eq!(
            to_outcome(doc),
            Outcome::Failed {
                code: "internal_error".into(),
                message: "An internal error occurred.".into()
            }
        );
    }

    #[test]
    fn usage_flattens_including_the_cached_subset() {
        let raw = r#"{"id":"r","status":"completed","output":[],"usage":{
            "input_tokens":13467,"output_tokens":40,"total_tokens":13507,
            "input_tokens_details":{"cached_tokens":7360},
            "cost":{"total_cost_usd":"0.0339875","currency":"USD"}}}"#;
        let doc: ResponseObject = serde_json::from_str(raw).unwrap();
        match to_outcome(doc) {
            Outcome::Completed { usage: Some(u), .. } => {
                assert_eq!(u.input_tokens, 13467);
                assert_eq!(u.cached_tokens, 7360);
                assert_eq!(u.total_cost_usd.as_deref(), Some("0.0339875"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unknown_field_does_not_break_a_wire_type() {
        // The rule that is the exact inverse of the one for compiled-in configuration.
        let raw = r#"{"id":"r","status":"completed","output":[],"brand_new_field":42}"#;
        assert!(serde_json::from_str::<ResponseObject>(raw).is_ok());
    }

    #[test]
    fn the_error_envelope_parses_with_its_param() {
        let raw = r#"{"error":{"message":"The request conflicts with the current resource state.",
            "type":"conflict_error","param":"idempotency_key","code":"conflict"}}"#;
        let env: ErrorEnvelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.error.code.as_deref(), Some("conflict"));
        assert_eq!(env.error.param.as_deref(), Some("idempotency_key"));
    }

    #[test]
    fn a_response_id_refuses_to_be_empty() {
        assert!(ResponseId::parse("").is_err());
        assert!(ResponseId::parse("   ").is_err());
        assert_eq!(ResponseId::parse("resp_x").unwrap().as_str(), "resp_x");
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    /// Upstream's rules, mirrored so a bad key costs a construct error rather than a round trip
    /// and a 400. The colon case is the one that actually bit: an ISO timestamp in a key.
    #[test]
    fn upstream_rules_are_enforced_locally() {
        assert!(IdempotencyKey::parse("multi-1f4-18c9d2e").is_ok());
        for (bad, why) in [
            ("", "empty"),
            (" k", "leading space"),
            ("k ", "trailing space"),
            ("a:b", "colon"),
            ("2026-08-27T00:12:33+00:00", "an ISO timestamp is all colons"),
            ("a/b", "path separator"),
            ("a\\b", "windows separator"),
            ("a\nb", "control character"),
        ] {
            assert!(IdempotencyKey::parse(bad).is_err(), "{why}: {bad:?} should be refused");
        }
        assert!(IdempotencyKey::parse("k".repeat(256)).is_ok(), "256 bytes is the cap");
        assert!(IdempotencyKey::parse("k".repeat(257)).is_err(), "257 is over it");
    }

    #[test]
    fn the_reason_names_what_is_wrong() {
        let e = IdempotencyKey::parse("a:b").unwrap_err();
        assert!(e.to_string().contains("colon"), "{e}");
    }
}
