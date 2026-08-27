# Design

Vocabulary is in `CONTEXT.md`. Hard-to-reverse decisions are in `docs/adr/`. This file is the
shape of the system: the types, the layering, and the state machine.

Prior art is `~/agency/ironworks` — a working Python implementation of the same product. This is
not a port. What that codebase is worth is not its code but its scar tissue: nearly every method
in `multi/seam/bridge_state.py` and `bridge_core.py` carries a docstring naming the defect it
closed. Those docstrings are the specification. Read them before changing anything here that
looks gratuitous.

## The upstream contract

Read from the pinned source at `ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/`,
not inferred from a
client. Facts marked **[measured @ rev]** were confirmed against a disposable instance at the
revision named — `dev/probe-up.sh` stands one up in about ten seconds, and a placeholder model key
is enough for every question that does not need a completed turn.

**Provenance is per fact, deliberately.** A qualification re-measures some contracts and not
others, so no section header can be honest about what it covers. Each fact below carries the
revision it was actually measured against, and a bump moves them one at a time as they are re-run.

`docs/UPGRADE.md` step 6 is where that rule is written out, with the history that produced it and
what a bump owes it — this paragraph states the fact, not the argument, because two copies of one
rule is one rule and one thing to keep in sync. What each qualification covered is in the same
file.

- `POST /v1/responses` takes `model`, `input` (string or items), `instructions`,
  `previous_response_id`, `tools`, `tool_choice`, and an IronClaw extension `x_context`.
  `Idempotency-Key` is a header. `temperature`, `metadata` and `stream` also parse; IronWorks
  sends none of the six it does not need.
- **[source-read @ 1.4.0, measured @ 1.4.0] The surface accepts client-declared function tools,
  and IronWorks declining them is a choice rather than a constraint.** This bullet's field list
  omitted `tools`/`tool_choice` until `docs/experiments/0031-the-external-tool-lane-is-open.md`
  read them out of `ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses.rs:25-27`. It matters because §L-2 of `docs/INVARIANTS.md` derives
  the empty confinement allow-set partly from our body carrying no `tools` — a sentence true of
  the body and easy to misread as a property of the runtime.
  Only `type: "function"` is accepted (256 tools, 64-byte name, 8 KB description, 64 KB schema, at
  the parse boundary). **The host never executes them**: a call parks the run and is handed back as
  a `function_call` output item, and the client resumes by re-posting `function_call_output` items
  with the `previous_response_id`, which waits again — *or parks on a further call*
  (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:889-892`). So the loop is the caller's, and each step re-enters the
  30-second wait below.
  The lane is gated on host wiring: `external_tool_store` and `external_tool_resume` are both
  `Option`, and when either is absent `tools`, `tool_choice` and `function_call_output` fail closed
  `400`. Measured wired on the probe (`production` profile) by the one request the two validators
  answer differently — `tool_choice` plus a `function_call_output` with no `previous_response_id`
  returns `param: "tool_choice"` unwired and `param: "previous_response_id"` wired; it returned the
  latter, and a declared function tool was accepted 200. **Not measured on the served profile**
  (`hosted-single-tenant-volume-sandboxed`), where wiring is a separate composition decision.
- **[measured @ 1.4.0] A parked run reports `status: "completed"`, and resolving one calls a tool
  confinement cannot see.** One round trip in
  `docs/experiments/0032-a-parked-run-says-completed.md`. There is no `requires_action` status: a
  run waiting on the caller returns `completed` whose `output` is a trailing `function_call` item
  (`call_id`, `name`, `arguments` as a JSON string) with **no `content` array and no message**, so
  `completed` stops implying *answered* and `to_outcome` would yield `Completed { text: "" }`.
  The resume replays the whole step history, and the submitted output is rewritten by the runtime
  into a `result_reference` marked `trust: "untrusted_tool_output"` which the model then reads back
  through **`builtin__result_read` — one of §L-1's eight captured-surface-only ids, outside the
  53-row settings catalogue `ConfinementPolicy` writes against.** Each step is its own 30-second
  wait (12.63s then 23.04s), so a loop is N creates and N waits and ADR 0005 applies per step;
  replay byte-identity is measured for create and **assumed, not measured, for resume**.
- **[source-read @ 1.4.0, measured @ 1.4.0] `DEFAULT_RESPONSES_WAIT_TIMEOUT = 30s`** and a turn
  that outlasts it is refused, not degraded. Create submits the turn and then waits for it to
  **complete**, under `tokio::time::timeout(wait_timeout, wait_for_response_completion(…))`
  (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:327`). On elapse the handler
  returns **HTTP 503 `service_unavailable`** — it does *not* hand back a partial status to poll.
  Measured: 503 at 30.05s against a 48 KB body whose write completed in 0.3 ms.
  **This is an operational bound, not only a contract one: the shipped service's own flagship
  question sits on it.**
  `docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md` asked
  *"What needs my attention across our accounts right now?"*
  seventeen times across every path — **eleven returned 503**, every
  completing sample fell between 26.5s and 30.0s and every refused one between 30.06s and 30.65s,
  so which side a turn lands on is provider variance rather than configuration. **The lever is the
  scope of the question, not the length of the answer**: narrowing it to one item cleared 9/10,
  while instructing the model to answer in three bullets, or in one sentence, cleared 1/3 each.
  Nothing was lost in any of the seventeen, because of the next sentence.
  **It is still not a cap on the turn.** The timeout abandons only the wait; the submission has
  already happened, so the turn runs to completion server-side and the answer is recovered by
  **replaying the idempotency key** (ADR 0005) — measured at 0.085s — rather than by polling,
  because no response id was returned to poll. Byte-identical at 1.3.0, so this is a long-standing
  contract and not a 1.4.0 change. Create-then-poll (ADR 0002) is unaffected as a decision: it is
  about not using SSE, and polling still follows a turn that answers inside the window.

  *This bullet previously read "waits up to 30 seconds, then returns whatever status it has",
  which the pinned source contradicts at both revisions. It was a misreading, not drift — which
  is why the marker above names a source read as well as a measurement.*
- Status is a six-variant enum: `queued`, `in_progress`, `completed`, `failed`, `cancelled`,
  `incomplete`. Only `completed` may advance a thread — and with `tools` in play `completed` does
  not mean *answered*, because a parked run reports it too (bullet above).
- **[measured @ 1.3.0] HTTP 200 does not mean success.** A turn that fails returns 200 with
  `status: "failed"`, an empty `output`, and a sanitized `error` (`internal_error` / "An internal
  error occurred." — no provider detail leaks through).
- **[measured @ 1.3.0] `model` is echoed on create but not on retrieval.** `GET` reports the
  literal string `"reborn"`. Which model produced a stored answer is therefore **unverifiable
  after the fact**: the pin is a construction-time invariant on the request and can never be
  audited post-hoc. Never write a check that pretends otherwise.
  `docs/experiments/0013-the-pin-fails-silently-not-closed.md` reads the pinned source on what
  happens *before* that, and finds something the post-hoc gap does not cover: whether the pinned
  model is honoured at all is a property of the **provider**, not of the request. NEAR AI honours
  per-request overrides, so it is; a provider that baked its model at construction would serve its
  own and IronWorks would change nothing, error nowhere and notice never. **The pin fails silently
  rather than closed**, and that is structural, not a bug.
- **[measured @ 1.3.0, re-measured @ 1.4.0] Replaying an `Idempotency-Key` with a byte-identical
  body returns 200 and the original response id.** A differing body is refused 409 with
  `param: "idempotency_key"`, `code: "conflict"`, naming no id. This is what makes ADR 0005
  possible. The 1.3.0 measurement carried a caveat — *measured against a failed turn; re-confirm
  against a completed one* — and **1.4.0 discharged it**: the qualification measured the same id
  returned in 0.077s against an original that took 15.78s, which is an index serving a completed
  turn, and `dev/prove.sh`'s `recover.b3_replay_identical` replays a key whose turn completed
  server-side and recovers that same id.
- **[measured @ 1.4.0] Replaying the key of a turn that is still running returns the id
  immediately, with `status: "in_progress"`.** Measured at **0.006s**, six seconds into a turn
  whose create was still blocked and went on to 503 at 30.053s
  (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 4). This is a third case beside
  the failed turn (1.3.0) and the completed one (1.4.0), and it narrows a sentence two bullets
  below: *no response id was returned to poll* is true of **create**, not of the turn. The id of an
  in-flight turn is obtainable at any moment by replaying the key the seam already minted, which is
  what makes the next bullet reachable.
- **[measured @ 1.4.0] A turn can be cancelled, but not through the route named for it.**
  `POST /api/v1/responses/{id}/cancel` is mounted
  (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/descriptors.rs:27-37`) and its
  handler is fully implemented, and it answered **400 `invalid_request`, `param: null`** in every
  state measured — `in_progress` and `completed`, on two runs — leaving the turn running. The
  webchat route works on the same run in the same state:
  `POST /api/webchat/v2/threads/{thread_id}/runs/{run_id}/cancel` with a `client_action_id` (a
  missing one is `400 missing_field`) returns `200 {"status":"CancelRequested"}`, and the blocked
  create then returned `status: "cancelled"` with empty output **before its 30s wait elapsed**. On
  a finished run the same call is `200 {"status":"Completed","already_terminal":true}` — idempotent,
  which is what a best-effort caller needs. So `Outcome::Cancelled` is reachable, and every turn
  abandoned at the cliff currently runs to completion and bills because nothing calls it.
- **[measured @ 1.4.0] Every response id is also a thread id.** Create issues
  `ProductCreateThreadRequest { requested_thread_id: <response public id> }`
  (`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:768-819`),
  so `GET /api/webchat/v2/threads/{response_id}/timeline` and `.../events` answer 200 to the same
  member bearer. The event stream is a **cursored projection** — the frame carries the resume
  cursor, replayable by `Last-Event-ID` or `?after_cursor=` — and carries progressive answer text
  plus the `run_id` the cancel above needs. ADR 0002 is unaffected: it declined `stream: true` on
  the create call, and this is a different object on a different route, read after create returns.
  Note also that `GET /api/webchat/v2/threads` returns each thread with a `title` holding the raw
  request input, so an envelope's record bytes are durably readable on a second route.
- **[measured @ 1.3.0] An unknown response id returns 404**, JSON-enveloped,
  `param: "response_id"`. **[measured @ 1.4.0]** A *different member's* id returns the same 404, and
  so does that thread's timeline, with the owner's own fetch returning 200 as the control
  (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 7).
- `instructions` is **sticky**, diverging from the OpenAI spec: IronClaw prepends it into the
  user message and persists it, so it replays on later turns. Re-sending every turn is correct
  (a once-only injection drifts), but it means persona bytes accumulate in thread history.
- `x_context` caps at **10 KB** (`MAX_RESPONSES_CONTEXT_BYTES`, `ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:58`) and
  renders as `<user-context>`. Too small for a book (ADR 0003). The body cap is 4 MB
  (`MAX_RESPONSES_BODY_BYTES`), which a 40-record envelope is nowhere near.
- Rate limit is 30 requests / 60 seconds per user, shared with `/api/chat/send`.
- **[measured @ 1.3.0] `401` is `text/plain`** (`Invalid or missing auth token`), while `400`
  and `404` are JSON-enveloped. The same is documented for `403` and the middleware `503`. An
  error parser that assumes one shape breaks on the most common failure there is.
- Cross-user `previous_response_id` resume is explicitly undefined upstream — it surfaces as a
  turn-level failure, not a `404`. Isolation must not rest on it.

## Layering

Cargo enforces this. It is not a convention anyone has to remember.

```
                               cli  (bin: ironworks)
   ┌───────┬────────┬──────────┼───────────┬────────┬───────┬─────────┐
records ironclaw channel  channel-http services journal proposals reconcile
   │       │        │          │           │        │       │    │  │  │
   └───────┴────────┴──────────┴───────────┴────────┴───────┴────┘  │  │
                              domain                       lifecycle ────┘
                       (depends on nothing)              (operator-side)
```

Twenty crates. `domain` depends on nothing; every other serving-path library depends only on
`domain` (`reconcile` also on `records` and `lifecycle`, because translating one into the other is
its whole job); `cli` composes them.

`lifecycle` and `reconcile` are **operator-side** and no serving-path crate depends on either. The
turn journal owns durability for a turn; `lifecycle` owns what an operator intended and what the
world reports, and outlives any turn by months.

**Six crates are measurement-side** and sit outside the diagram: no serving-path crate depends on
any of them, and they exist so a claim about the system can be checked mechanically rather than
asserted in prose.

| Crate | Holds | Depends on |
|---|---|---|
| `graders` | the answer-quality graders and every control that pins them | **nothing** |
| `eval` | `ironworks-eval`: runs a suite, classifies a turn, writes a transcript | `graders`, `evidence` |
| `evidence` | the book fingerprint and the regrade ledger | `domain` |
| `page` | the shared palette, escaping and document shell | **nothing** |
| `site` | `ironworks-site`: the published page, the arc, the reader, the 404 | `domain`, `graders`, `evidence`, `page` |
| `claims` | `ironworks-claims`: fails when prose disagrees with the tree | — |

`graders` declares no dependencies at all, and that is the load-bearing one: **a grader that could
reach a runtime, a record store or a credential could influence what it grades.** `eval` reaches
all three, which is why it is a separate crate rather than a module. The grader library declares
no dependencies at all: the rules it replaces were regexes, and writing them out kept `regex` (and
`fancy-regex`, which two of the patterns would have needed for lookaround) out of the lockfile.

`site` depends on `graders`, and that edge is deliberate rather than convenience: the published
page **states a refusal rate**, and it must be computed by the same instrument that graded the
answers rather than by a second reading of them. The direction is the safe one — the page depends
on the grader, never the reverse — so nothing a page needs can reach back into what a grader sees.

| Crate | Holds | Owns |
|---|---|---|
| `domain` | the type model: identity, typestate, capability, routing, delivery evidence, progress, service definitions, record shapes, composition (`melt`) and the envelope | — |
| `services` | persona fragments, skills and service definitions, embedded at build time | — |
| `records` | the record-store adapter: credential → scope, then scoped reads | `ureq` |
| `ironclaw` | the IronClaw protocol: exact request bytes, polling, failure classification | `ureq` |
| `channel` | channel semantics: the derived cursor, the cross-tenant barrier, dispositions, compaction | `rusqlite` |
| `channel-http` | the channel transport: poll with an offset, per-chunk send | `ureq` |
| `journal` | durable turn state, delivery evidence, thread pointers | `rusqlite` |
| `proposals` | what a person asked to promote and has not yet confirmed | `rusqlite` |
| `lifecycle` | operator-side state: intent, provisioning progress, observed authority, confinement policy | `rusqlite` |
| `reconcile` | translating authoritative systems into lifecycle observations — Account Service, IronClaw members, fleet enumeration, confinement | `ureq` |
| `devstore` | a disposable record store, for the live proofs | — |
| `devchannel` | a disposable channel, for the delivery proofs | — |
| `telegram` | the channel contract on one side, the Telegram Bot API on the other | `ureq` |
| `cli` | the operator binary, `ironworks` | — |

**The rule Cargo enforces: `domain` and `services` name no IO dependency.** No `ureq`, no `rusqlite`,
no filesystem, no clock, no credential values. `domain` does carry `serde`, because record shapes
parse from the store's JSON and parsing is not an effect — the alternative, a separate crate
`domain` could not call, is exactly what once kept the records envelope outside the verification
gate (§K-3, §K-7 in `docs/INVARIANTS.md`).

**And the binary owns no network access.** `cli` declares no transport dependency: everything
that opens a socket is an adapter crate. That is what makes "who can talk to what" answerable by
reading the dependency graph rather than by grepping. It briefly was not true — a channel HTTP
client lived in the binary on the argument that a single implementation needs no boundary — and
the correction is worth keeping, because the boundary was never about swapping implementations.

`ironworks-channel` goes further and asserts its own `[dependencies]` set in a unit test. It is
the one cross-tenant object in the system, so it is denied the vocabulary: it cannot name records,
service definitions or a transport, and it learns exactly one thing about a tenant's work —
`Openness`, two values — through a one-method trait.

The package is named `ironworks-cli`; its operator-facing binary is simply `ironworks`.

### Names

Machinery takes the metalworking vocabulary in `CONTEXT.md`; the product does not. `tenant`,
`service`, `guidance`, `record`, `turn` and `channel` stay plain, because those are words
customers and operators say out loud.

Several metal names proposed early were never built — `anvil`, `ladle`, `hearth`, `mould`, `jig`,
`fettle`, `touchstone`, `grade`, `hallmark` — and the theme survives in three places rather than
as a system: `scrap/` and `services/` at the repo root, `melt`, and `Alloy`. A name looking for a
job is a job invented for a name; the crates above are named for what they hold.

## Types that carry invariants

Every identifier is a newtype with a private field, no `Default`, and one fallible constructor.
The point is not tidiness — it is that the invalid value has no way into the program.

```rust
Pin, ModelName, IroncClawRev      // no Default: a default is the one value that outranks config
OrgId, Slug, ServiceName          // validated charset, non-empty
AccountId, ContactId, ActivityId
ResponseId, IdempotencyKey
UpdateId, ChatId, MessageId
```

Three of these deserve more than a newtype.

### `CredentialRef` — there is nothing to leak

The first design here was a redacting token type: a private `String`, a `Debug` that prints
`(redacted)`, no `Display`, no `Serialize`. That is the shape ironworks needs, because
`multi/seam/redact.py` plus a `secrets_of()` set threaded through every print site is the only
thing standing between a token and a log line in a language where any string can reach one.

What was built is stronger, and simpler:

```rust
pub struct CredentialRef { purpose: CredentialPurpose, handle: String }
```

It holds **where** a credential lives, never its value. An adapter resolves the handle at the
moment of use, so nothing in the domain model has a secret to leak — a careless `{:?}` prints a
keyring path. Redaction is still needed for text we did not construct, such as an upstream error
body, but it stops being the only defence.

`CredentialPurpose` is `RuntimeMember` or `RecordScope`, so a credential minted for one cannot be
passed where the other belongs.

### `RecordScope` — tenancy in the key, as a type

The Postgres schema this borrows from keys every row `(org_id, <id>)` rather than filtering in the
`WHERE` clause, so an id collision between two orgs creates two independent rows instead of
re-homing one.

An `AccountRef { org, id }` was written to mirror that and then deleted, because the job was
already done by the type that gates every read:

```rust
book.catalog(&scope)            // RecordScope, obtainable only from ServiceInstance<Verified>
book.record(&scope, &id)        // there is no unscoped form
```

`RecordScope` carries the org from the verified evidence, and no store method exists that does not
take one — so a record fetched for one tenant cannot be looked up under another by a forgotten
predicate. `AccountRef` was a second mechanism for a property already enforced on the only path,
and it was never used outside its own tests. Two mechanisms for one invariant is one mechanism and
one thing to keep in sync.

Be honest about the limit: Rust does not stop a hand-written query string from omitting a clause.
What it gives is one chokepoint, plus results that carry their scope with them.

Isolation tests need a positive control. An empty store passes every cross-org check, so each one
must also assert the own-org case actually returns data.

### Verdicts — BLOCKED is not a pass

A `Verdict` enum with `Pass / Fail / Blocked / Skipped` and a `promotable()` that refuses anything
but `Pass` was built in `domain`, went unused, and was deleted — a name looking for a job. The
discipline survived it in the proof scripts, which report `PASS / FAIL / BLOCKED` and exit
`0 / 2 / 3`, so an offline run cannot produce a clean green that proves nothing.

**It came back when something consumed it.** `ironworks_lifecycle::Verdict` is what
`lifecycle inspect` and `lifecycle members` return, and the exit code carries it — because
`list | grep` under `pipefail` reports the pipeline's status whatever grep found, and a caller
reading that as a boolean concludes the opposite of the truth (§F-12).

Two differences from the deleted draft, both forced by the caller rather than chosen:

- **`Skipped` is gone.** Nothing skips: an inspection either measured something or reports why it
  could not, and `Blocked` already covers the second. A variant meaning *we did not look* beside
  one meaning *we could not* is two names for the same operator action.
- **`AcceptedRisk(NonEmptyReason)` exists and nothing constructs it.** It is where an operator's
  explicit classification of a known risk will live, and the reason sits *in the variant* so it
  cannot be produced without one — a waiver nobody can account for later is the same as no waiver
  (§F-8). It is defined ahead of its producer specifically so unmanaged authority is never
  normalized into it, which is the "make a gate green by forgetting" move §F-9 forbids.

The rule that decides the ordering: **`Fail` outranks `Blocked`.** A measured contradiction is
worse news than an unmeasured one, and the operator should see the sharper fact first.

## The state machine

The single highest-value thing Rust buys here. The payload lives in the variant, so states the
Python can represent and then has to reject at runtime cannot be constructed at all.

The design changed materially during implementation, and the change is the interesting part. It
was drafted as one flat `UpdateState` enum covering everything from `Received` to `Acked`. What
was built is **two axes in two crates**, because the two questions have different scopes: what
happened to a *turn* is tenant-scoped, and what the *channel* has been told is shared across every
tenant at once.

```rust
// ironworks-journal — tenant-scoped: what became durable about one turn
pub enum TurnOutcome {
    NotSubmitted,
    SubmittedUnknown,                                       // never a NEW logical turn
    Accepted  { response: ResponseId },
    Completed { response: ResponseId },
    OperatorRequired { response: Option<ResponseId>, reason: NonEmptyReason },
}

// ironworks-domain — tenant-scoped: what is known about the OUTBOUND answer
pub enum DeliveryState {
    Pending,
    KnownUnsent,                                            // proof of absence
    Uncertain { acknowledged_chunks: u32 },                 // absence of proof
    Delivered,
}

// ironworks-channel — channel-scoped: what was decided about one INBOUND update
pub enum Disposition { Unclaimed, Turn(IdempotencyKey), NotAddressed, Unroutable }
```

Four things the draft got wrong, each corrected by something the code or the corpus said:

**There is no `Acked` state.** An update is acknowledged exactly when the offset the channel was
told is past it — a derived predicate over one stored monotone value, not a column. A row cannot
claim an acknowledgement the channel was never given.

**There is no cursor, either.** The safe cursor is the **lowest open update**, across every
tenant, computed from the durable rows on every read — §E-17 of `docs/INVARIANTS.md` states the
rule; what follows is what it buys. That one line is the whole cross-tenant
barrier, and it also gives batch-before-workers and graceful shutdown for free. Crucially it makes
crash boundary **B6** — delivered without the offset advancing, or the reverse — *unrepresentable*
rather than defended by a transaction. See ADR 0006.

**`DeliveryRetry` and `DeliveryReconcile` are not states.** They are the `Repair` a
`DeliveryState` implies. `KnownUnsent` means a resend cannot duplicate; `Uncertain { n }` means it
may put a second copy of content a client can already read. `DeliveryState` is a
join-semilattice ordered by how much client-visible content may exist, and folding an attempt into
it is the join — so *evidence only ever sharpens*, and a failed retry reporting "nothing was sent"
cannot erase a chunk already in the client's room. That is a property of the operation, not a rule
someone has to remember.

**`RecoveryBlocked` became `OperatorRequired`, and settling it is what keeps the bridge
listening.** A turn nobody will act on again must stop holding the shared cursor, or one stuck
conversation deafens every conversation behind it while the process looks healthy. `Openness` —
the one thing the channel coordinator may learn about a tenant's work, two values — therefore
means *automatic work remains*, not *succeeded*. The alarm survives separately: nothing compacts
an update that cost a model turn.

Two rules the draft encoded that survived intact:

1. **An in-flight update is never re-dispositioned.** A disposition is written once, so a tenant
   whose configuration was moved aside mid-flight cannot have its possibly-billed answer relabelled
   "not addressed to us". Routing decides whether to *answer*; it cannot decide what already
   *happened*.
2. **`Delivered` and acknowledged are different facts.** An update is finished only once the
   channel has been told an offset past it, which happens on a *later* poll. Until then it is
   redelivered — which is what makes crash recovery possible at all.

The only place an unknown state exists is deserialization from the store, where a string from disk
may genuinely not parse. That is a `Result` at the boundary, not a variant in the enum — and a row
that has lost the field its state depends on is refused rather than read as a blank.

### Thread continuation is a policy, not a default

Always chaining is the obvious implementation and it is wrong. Measured (see
`docs/experiments/0001-alloy-drift.md`): because IronClaw persists `instructions`, input grows by
one alloy per turn, re-persisted rather than deduplicated. A 30-turn thread cost **$4.91**, with
the last turn roughly five times the first.

The rate is a property of the alloy, so it moves with the composition: `0001` measured
**~3,100 tokens per turn** against `account-analysis@1`'s 14,337-byte alloy, a service that no
longer ships. For the shipped `relationship-intelligence@2` it is **~5,700–5,910** — §I of the
invariants carries the current figure and what now binds a conversation first, which is a turn
count rather than the window.

So continuation is an explicit decision with a type:

```rust
enum Continuation { Fresh, Chain(ThreadPointer) }
```

and the chain breaks deliberately — when the alloy's fingerprint changes, and when the thread
exceeds a bounded turn count.

**Built.** `core::continuation` decides it and a `Fresh` turn always says *why*: `NoPriorTurn`,
`CompositionMoved { was, now }`, or `ThreadLongEnough { turns, limit }`. A chain that broke
silently would look exactly like a service that had forgotten you. Correctness outranks cost, so
a thread whose composition moved is refused for *that* reason even when it is also over the limit
— the operator needs the sharper fact. The journal keys threads by conversation and records what
each was built on and how many completed turns it holds; a composition change starts the count
again, because the turns before a broken chain are not that thread's history.

**A chained turn does not re-send the book.** `core::supply` decides it from an exhaustive table
over two independent options — what this thread was last shown, and what the store says now — and
`RecordsState::CarriedByThread { records }` is a fourth state rather than reuse of `Empty`, because
a model told "empty" would answer that the organization has no records having been handed forty of
them one message earlier. The fingerprint is over the **rendered** records rather than a list of
ids: a catalog version would notice an account arriving and miss an activity body being edited,
which is the change a coordination service must not answer around. Measured on a real chain, the
second turn had been carrying another 14,514 bytes of the same forty records.

Three absences that are not the same fact, and the reason the table has four cells rather than a
boolean: **never shown**, **shown an empty book** (which has a fingerprint like any other), and
**shown, but the store cannot currently say whether it moved** — the last resends, because
"unchanged" must be established rather than assumed.

**There is no separate token bound, and that is not a shortcut.** The design said "a bounded turn
count *or* token estimate". They are the same lever: instructions dominate the growth and are
re-persisted at a constant size every turn, so tokens are proportional to turns for any one
composition. A second knob would be the first in different units. ironworks found the first half (it refuses to
continue a thread across a composition change, using `instructions_sha256` as thread identity);
the cost curve supplies the second, and that half stands on economics alone, independent of
whether the safety tail erodes.

**A thread belongs to a conversation, not to a tenant.** The journal keyed `threads` by instance
alone through schema 2, which was correct while one tenant meant one room and silently wrong the
moment it did not. Nothing read it — `cli` never chains, so every turn is `Fresh` — so no
conversation was ever crossed; the row was simply being written under a key that would have leaked
later, attached to a feature nobody would think to connect to it. With continuation on, a second
room's turn would chain from the first room's response and the runtime would replay that history
into it. Schema 3 keys `(instance, channel)`, `thread_pointer` requires the room to be named, and
a turn with no conversation — `ironworks turn`, answered on a terminal — writes no row at all,
because a thread that can never be continued is not a thread.

`ThreadPointer` carries its tenant (`{ tenant: TenantId, response: ResponseId }`) because
upstream will **not** catch a cross-wired pointer: a foreign `previous_response_id` on `POST` is
documented as undefined behaviour rather than a `404`. Isolation rests entirely on our store
never producing one, so the type is the enforcement.

### Durability

One store, one transaction (**E-1**) — because two files can disagree in exactly one direction
that silently corrupts a live conversation, and the corruption is undetectable afterwards, both
files being individually well-formed.

The journal holds identifiers only (**E-11**), asserted by a test that greps a populated store for
fixture content. Compaction is bounded by the *acknowledged* cursor rather than the safe one
(**E-10**): dropping a row the channel can still redeliver is the duplicate defect, reintroduced
by tidiness.

**E-22** is where this is stricter than the reference, and the mechanism is the interesting part.
The draft encoded compactability as an enumeration of the states worth keeping. What was built is
a predicate instead:

```rust
impl Disposition {
    /// Whether this update ever cost a model turn.
    pub fn cost_a_turn(&self) -> bool { matches!(self, Self::Turn(_)) }
}
```

Anything that produced a model turn is kept forever; only noise is forgotten. The reference's
enumeration was wrong twice — most expensively when the operator-reconciliation alarm turned out to
be age-compactable and a possibly-billed, never-delivered turn was deleted with nobody having acted
on it. A predicate asking *did this cost a turn?* cannot be wrong about a state nobody thought of.

## Errors

Per-crate error enums via `thiserror` — effectively free, since `serde` derive already brings
`syn`/`quote`. No `anyhow` in library crates; the binary may use it at the top.

Fail-closed is the default: a constructor returns `Result`, and there is no `unwrap_or_default`
anywhere in the tree. `clippy` denies `todo!` and `unimplemented!`; the workspace forbids
`unsafe_code`.

## Dependency budget

A design constraint, not an afterthought. The Python this borrows from has **zero** runtime
dependencies, and that property is worth most in the process holding every tenant's credentials.

| Need | Choice | Why |
|---|---|---|
| HTTP | `ureq` + `rustls` | Blocking, matching the threaded model. No async runtime. |
| Store | `rusqlite`, `bundled` | Pins the SQLite version *in the binary* rather than inheriting the host's. |
| JSON | `serde` / `serde_json` | |
| Hash | `sha2` | Composition digests. |
| Errors | `thiserror` | |

**98 crates in the lockfile** (`grep -c '^\[\[package\]\]' Cargo.lock`), against the several
hundred an `axum`/`tokio`/`sqlx` stack costs. The committed `Cargo.lock` is what holds this today;
`cargo deny` is **not** configured, so "a new transitive dependency is a decision" is currently a
review habit rather than a gate — and the habit has already let ten arrive since this line first
read 88. Worth building before the dependency set grows past the point where anyone reads the
diff.

## Concurrency

**Not built.** `ironworks relay` handles one poll cycle on one thread and exits. Nothing here runs
concurrently yet, and the section below is the design, not a description.

`dev/relay-loop.sh` is **repetition, not concurrency**: one `relay` process at a time, in series,
each a fresh process so the crash boundaries `dev/prove.sh` provokes stay real. It is what makes a
bot answer continuously without making the binary a daemon, and it changes nothing below.

Blocking IO, a bounded worker pool, serial within a tenant, concurrent across tenants. The
bottleneck is a model turn measured in tens of seconds; async buys nothing and costs the
dependency budget. `Send`/`Sync` will replace what the Python maintains by hand with
`check_same_thread=False` and a re-entrant lock.

The part that *is* built is the part concurrency would otherwise break: the whole batch is
journalled before any of it is worked on, and the cursor rule above holds — so a fast tenant cannot
acknowledge a slow tenant's unfinished work. Both are proven today by single-threaded tests that
construct the interleaving directly; adding workers should not require changing either rule, which
is the point of having derived the cursor rather than stored it.

## Status

Everything below is measured, not asserted. The local figures re-run anywhere; the proof scripts
tabulated below need `dev/probe-up.sh` first, except `dev/prove-lifecycle.sh` and
`dev/prove-tenancy.sh`, which need no runtime and no model key — reconciliation is a read, and
record separation is a property of the record path rather than of an answer.

**The table is the six that run against a probe, not every proof in `dev/`.** Three others exist
and are deliberately absent from it, because a check count beside them would read as a result
somebody has seen: `dev/prove-confinement.sh` needs the runtime but **no model key** (confinement
is entirely the settings API) and still *consumes a member* when it runs — §L-9, approval-off is
one-way — though as of 2026-09-02 it **mints that member itself** rather than borrowing the
probe's, so the residual is discarded with it and the stack is reusable across runs (measured: two
consecutive runs on one probe, 6 PASS each). That change also moved what it proves: the env bearer
it used to confine reports a **53**-row catalogue, and a minted member **52**, so the floor §L-8
states — 50 disabled + 2 locked — is the minted member's and was not what the old run measured
(`docs/experiments/0037-what-the-seam-never-asked-for.md` check 3). `dev/prove-workspace.sh` needs a
runtime on `hosted-single-tenant-volume-sandboxed`, which `dev/docker-compose.probe.yml` is not;
and `dev/prove-deployment-isolation.sh` runs as root on a deployment host, which does not exist
(`deploy/README.md`). A bare count was here and was wrong — it said six while `dev/` held nine —
so the enumeration below is the claim and no number stands in front of it.

```
cargo test --workspace       passing
cargo clippy --all-targets   clean
cargo fmt --check            clean
alloy digest                 3770e0942624bda4   coordination-eval, 25,575 bytes

                             last run 2026-09-02, one probe, one sitting
dev/prove.sh                 9 PASS   identity, refusals, B1-B5
dev/prove-delivery.sh        4 PASS   delivery evidence, B5 and B7, across real restarts
dev/prove-lifecycle.sh       5 PASS   reconciliation, no runtime, no key
dev/prove-tenancy.sh         7 PASS   two tenants, one store, disjoint records
dev/prove-boot.sh            4 PASS   hardened boot, fresh and existing volume
dev/prove-continuation.sh    5 PASS   a chained turn recovers; a stuck one frees the channel
```

**The date beside that block is load-bearing, because earlier the same day it read
5 PASS 2 FAIL 2 BLOCKED and 2 PASS 2 BLOCKED.** Nothing in this repository changed between those
runs and these. `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30s, and when the provider is slow a turn
outlasts it and returns `service_unavailable` — measured that day at eleven refusals in seventeen
asks of the shipped question
(`docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md`), every one
recovered by byte-identical replay in 0.04-16.4s. Two of those landed on `chain.end_to_end` and
`recover.b2_before_dispatch`, and `dev/prove.sh` had no arm for them, so it scored FAIL: a defect
reported against this code for the provider being slow. `degraded()` now recognises that refusal
as the unavailable dependency it is, alongside the rate limit it already knew about.

**That fix does not make a blocked run a pass, and must not be read as making one.** A BLOCKED
`chain.end_to_end` is the positive control checks 3 and 4 need, so a sitting that blocks it
measured nothing about isolation either — the same reading `docs/INVARIANTS.md` §K records for the
1.4.0 qualification. The clean run above needed no such arm: it hit no 503 at all. **So a count
beside these scripts is a claim about the provider's latency that day as much as about this
repository**, which is why the date is in the block rather than in prose beside it.

**The digest names its tenant now, because it never could have named none.** The alloy is
guidance-dependent — `tenant-a` gives `d1acc6cb381b1875`, `tenant-b` `74540e4c87c3a845`, `eval`
`5e75679666c8bf7e` — so `alloy digest <hex>` with no tenant beside it was under-specified from the
first line it was written on. The value that stood here, `67f80eb2d20172fa` over 14,337 bytes, was
`account-analysis@1`: a service deleted on 2026-09-02, whose digest no composition in this tree can
now produce. `docs/INVARIANTS.md` §K keeps that figure deliberately, as dated evidence of what §K
was earned against; this block is where a current one lives, and a current one has a tenant.

### One turn, end to end

The path an inbound message takes, in order. Every other section here describes one part of it;
this is the only place the whole path is in one piece. **The unit is a turn, not a tool call** —
nothing declares `tools`, so there are no tool calls to follow.

```mermaid
sequenceDiagram
    autonumber
    participant P as Person
    participant TG as adapter<br/>(telegram / devchannel)
    participant R as ironworks relay<br/>(the seam)
    participant CJ as ChannelJournal<br/>(SQLite, cross-tenant)
    participant AS as Account Service<br/>(record store)
    participant J as Journal<br/>(SQLite, per-turn)
    participant IC as IronClaw<br/>(external runtime)

    P->>TG: message
    R->>CJ: safe_cursor — derived, never stored
    R->>TG: GET /poll?offset=N
    TG->>R: [{id, channel, text, actor, chat_kind}]
    R->>CJ: note_batch(ids) — the whole batch, before any of it is worked

    Note over R: route update.channel against registry claims<br/>no claimant → Unroutable · ambiguous → hold the cursor
    R->>CJ: disposition(id, Turn(key)), key = upd-{id}

    Note over R: boot_tenant — service definition + guidance
    R->>AS: resolve credential → org (X-Service-Token)
    AS->>R: ScopeEvidence { org, endpoint }
    Note over R: ServiceInstance&lt;Unverified&gt; --verify()--> &lt;Verified&gt;<br/>fails closed: no evidence, no instance

    R->>J: turn(key) — recovery from durable evidence BEFORE anything new
    J->>R: None (a fresh turn)

    R->>AS: catalog + records, under a RecordScope
    AS->>R: records, and the org the store believes it served
    Note over R: org mismatch → ScopeDrift → REFUSE TO SERVE<br/>every other record failure degrades

    Note over R: build_turn — THE CHOKEPOINT<br/>melts its own alloy, renders its own envelope
    R->>J: begin(...) — durable BEFORE a byte leaves
    R->>IC: POST /v1/responses (Bearer member token, Idempotency-Key)
    IC->>R: 200 { id } — may be non-terminal
    loop poll to a terminal state, 2s → 10s
        R->>IC: GET /v1/responses/{id}
    end
    IC->>R: completed { text, usage }

    R->>J: commit_completed — response id and thread pointer, or neither
    Note over R: malformed_answer(text) → print, do NOT deliver
    R->>TG: send(chunks)
    TG->>P: answer
    R->>J: record_delivery_attempt
    Note over R: the cursor advances only past settled updates
```

| # | Step | Symbol |
|---|---|---|
| 1 | Offset derived across every tenant from durable rows, never stored (ADR 0006) | `crates/channel/src/lib.rs::safe_cursor` |
| 2 | Fetch updates | `crates/channel-http/src/lib.rs::poll` |
| 3 | Journal the whole batch before working any of it | `crates/channel/src/lib.rs::note_batch` |
| 4 | Route `update.channel` against registry claims | `crates/domain/src/route.rs::resolve` |
| 5 | Mint the key, disposition the update | `crates/channel/src/lib.rs::disposition` |
| 6 | Boot the tenant: definition, guidance, unverified instance | `crates/cli/src/main.rs::boot_tenant` |
| 7 | **Control** — resolve credential → org, server-side; fails closed | `crates/domain/src/instance.rs::verify` |
| 8 | Recovery before anything new | `crates/cli/src/main.rs::drive` |
| 9 | **Data source** — fetch records; degrades, except scope drift | `crates/records/src/lib.rs::catalog` |
| 10 | **Chokepoint** — compose the alloy, render the envelope | `crates/domain/src/instance.rs::build_turn` |
| 11 | Envelope rendering: no clock, no credentials, no IO, so a crashed body rebuilds byte-identically (ADR 0005) | `crates/domain/src/envelope.rs::build` |
| 12 | Durability before dispatch | `crates/journal/src/lib.rs::begin` |
| 13 | Dispatch under an idempotency key | `crates/ironclaw/src/client.rs::create` |
| 14 | Atomic commit of response id + thread pointer | `crates/journal/src/lib.rs::commit_completed` |
| 15 | Control-token gate between the answer and the client | `crates/ironclaw/src/model.rs::malformed_answer` |
| 16 | Chunked delivery to the **inbound** address | `crates/channel-http/src/lib.rs::send` |
| 17 | Delivery evidence recorded | `crates/journal/src/lib.rs::record_delivery_attempt` |

**No step consults a `CapabilityProfile`.** It is attenuated once at instance construction and
carried into `ServingIdentity`, never read again before the model call. The capability model is
enforced structurally (`Access` has no `Write` variant, so there is no write to check; records
reach the model only through a `RecordScope` only a verified instance can mint) and out-of-band
(`runtime_tools` becomes a `ConfinementPolicy` applied by an explicit operator command, in another
process, against another database). That is deliberate — but it is why *"did this turn respect its
capability set?"* has no per-turn answer, which §I of the invariants records as a gap.

### Crash boundaries

**Fifteen**, numbered as the reference corpus numbers them
(`~/agency/ironworks/multi/seam/test_bridge_recovery.py`). The numbering is used throughout this
repository and in `docs/PRD.md` and `docs/INVARIANTS.md`; this table is where it is defined, and
every row's definition was read out of that file rather than inferred. **`B<n>` is a crash
boundary; `§B-<n>` is an isolation invariant in `docs/INVARIANTS.md`.** They are one hyphen apart
and unrelated.

| | Crash point, as the reference defines it | Here |
|---|---|---|
| **B1** | before anything is journalled | nothing durable; handled once on retry — `crates/journal/src/tests.rs::b1_crash_before_journalling_leaves_nothing_durable` |
| **B2** | key durable, request not sent | outcome UNKNOWN; **never** a second model run — `…::b2_a_key_durable_before_dispatch_is_dispatched_with_the_same_key`, `…::b2_an_ambiguous_dispatch_is_never_treated_as_unsent` |
| **B3** | model completed, id not persisted | replay the same key with an identical body — `…::b3_a_completed_but_unrecorded_turn_replays_the_same_key_with_an_identical_body`, and its refusal half `…::b3_a_body_that_cannot_be_rebuilt_identically_may_not_be_replayed`. **Where this improves on the reference**, which blocks it for an operator (ADR 0005) |
| **B4** | completed and recorded, not delivered | fetch that answer by id; never rerun — `…::b4_a_known_response_is_fetched_never_regenerated` |
| **B5** | channel accepted it, before that was recorded | a duplicate, and it must be the **identical** answer — `crates/channel/tests/crash_boundaries.rs::b5_the_residual_window_yields_an_identical_duplicate_never_a_new_answer` |
| **B6** | inside "delivered *and* the offset advanced" | **dissolved.** The offset is derived (ADR 0006), so there is no second write to be non-atomic with. `…::b6_delivered_and_the_offset_cannot_disagree` asserts the split state has no representation |
| **B7** | offset durable, channel not yet told | it redelivers; no rerun, no resend, only an acknowledgement — `…::b7_delivered_but_unacknowledged_yields_no_rerun_and_no_resend` |
| **B8** | mid-batch | done stays done; the crashed one never runs again; the untouched are still owed — `…::b8_a_crash_mid_batch_leaves_the_done_done_and_the_untouched_still_owed` |
| **B9** | during compaction | **dissolved.** `crates/channel/src/lib.rs::compact` is a single `DELETE`, atomic in SQLite. There is no multi-step compaction to interrupt, so no partial state to leave behind |
| **B9b** | compaction outrunning the channel | **enforced, and it is E-10.** `compact` returns `0` unless an acknowledged cursor exists and deletes only `id < acked`, so a row the channel can still redeliver is never dropped |
| **B10** | store bytes are not a database | fails closed, and says the one thing that stops the reader guessing — `crates/channel/src/tests.rs`, `crates/journal/src/tests.rs` and `crates/lifecycle/src/tests.rs` each carry their store's half. Its legacy-migration third has no analogue: there is no prior format to migrate from |
| **B10b** | store readable by group or world | **enforced.** `0o600` asserted on all four stores — channel, turn journal, lifecycle ledger, proposals — plus `crates/domain/src/custody.rs` |
| **B11** | route vanished while the turn was in flight | parked for an operator, never relabelled, never re-run — `…::b11_a_route_that_vanished_mid_turn_parks_the_update_and_never_relabels_it` |
| **B11b** | route vanished on an already-**delivered** row | **dissolved, twice over.** The reference's defect was relabelling the row and overwriting `delivered_at` and the response id. Here `crates/channel/src/lib.rs::disposition` refuses any change to a row that is not `unclaimed` (`AlreadyDispositioned`), so a disposition is write-once — and the delivery evidence is not in that row at all, it is in the turn journal under the idempotency key. Two stores, and nothing routes over either |
| **B12** | age-compacting the alarm | **dissolved, and strictly stronger.** The reference had to exempt `RECOVERY_BLOCKED` by name. Here the turn journal has **no compaction at all** — no `DELETE FROM turns` anywhere — so `Journal::unresolved` keeps reporting an unsettled turn forever, and channel compaction only ever removes `not_addressed` and `unroutable`, which by `Disposition::cost_a_turn` are the two that cost nothing |

**B1-B7 are proven**, B1-B4 and B5/B7 across real process death in three proof scripts — and
**B3 is proven twice**, once for a fresh turn and once for a chained one, because those recover by
different evidence and only the first worked until `dev/prove-continuation.sh` was written to ask.
**B8, B9b, B10, B10b and B11** are covered by the workspace suite. **B6, B9, B11b and B12 are
dissolved rather than tested** — each names a state this design cannot represent, so the mechanism
that removes it stands in for the test that would defend it, and each row above names that
mechanism.

**Two of the fifteen have no test of their own here, deliberately.** B9 and B11b are dissolved by
the shape of a single SQL statement and by a write-once column, and a test asserting "the thing
that cannot happen did not happen" over a mechanism with no second write would pass vacuously —
`§F-14`. What defends them is the mechanism being small enough to read, which is why each row names
the exact function.

**The digest is the one number worth asserting here**, because it moves only when a model-visible
byte changes. Byte counts are not that kind of fact: an envelope size is a property of the corpus
*and* of `dev/prove.sh`'s `ASK` string, so shortening the question by ten characters moves it by
ten bytes and anyone editing `ASK` appears to have caused drift. A count that moves for reasons
nobody cares about is a count somebody has to keep in step, and this block asserts none.

The last full qualification of the proof scripts is the 1.4.0 pin bump, recorded per contract in
`docs/UPGRADE.md`: delivery 4/4, lifecycle 5/5, boot 4/4, and `dev/prove.sh` **6 PASS, 1 BLOCKED**
— the model provider rate-limited one chain check, so the run exited **3** and was not a pass.
`dev/prove.sh` scored it correctly: an unavailable dependency blocks rather than fails, because
scoring it `FAIL` would report a bug in IronWorks for something upstream of it, and an operator
who investigates two of those learns to skim the ones that matter.

**That qualification has not been re-earned since, and the block above is a run rather than a
qualification.** A qualification re-measures the contracts a pin bump touches, per
`docs/UPGRADE.md`; 2026-09-02 was one sitting against one probe, which is why its numbers sit in
the block with a date and are not written into the upgrade table. Two of them are worse than the
1.4.0 figures and the reason is above: at 1.4.0 the provider rate-limited one check and the script
BLOCKED it; on 2026-09-02 the provider was available and simply slower than the runtime's 30s
wait, which the script FAILs. **Those two scores are not comparable, and reading the second as a
regression in this code would be the specific mistake the BLOCKED/FAIL distinction exists to
prevent** — in the other direction, for once.

**Done.**

- **Identity.** An unverified instance cannot construct the value required to invoke the runtime.
  Scope is evidence, not a boolean.
- **One turn, live.** Composition, records, envelope, `/v1/responses`, poll to completion.
- **Turn durability.** B1–B4, including B3 — which the reference treats as unrecoverable and
  ADR 0005 measures as recoverable by idempotent replay of a byte-identical body.
- **Channel durability.** B5–B7, `DELIVERED` ≠ `ACKED`, the cross-tenant cursor barrier, delivery
  evidence as a join-semilattice. The cursor is derived rather than stored, which makes B6
  unrepresentable rather than defended (ADR 0006).
- **The model-visible surface is gated.** Instructions and the records envelope are both
  constructible only through `ServiceInstance<Verified>`. See §K of `docs/INVARIANTS.md`.
- **Lifecycle observation.** Operator intent, recorded progress and observed external authority
  are three separate axes; reconcilers exist for the Account Service, individual members, fleet
  member enumeration, and confinement. Nothing adopts, provisions or repairs.
- **Confinement policy, application and verification** against IronClaw 1.4.0, with its limits
  stated — §L of `docs/INVARIANTS.md`. `ironworks lifecycle confine` applies a plan and then
  re-measures it, taking its verdict from a fresh observation rather than from its own writes.
  **The policy is the service's, not the operator's**: `capabilities.runtime_tools` on the service
  definition decides it, `ConfinementPolicy::for_profile` is a total function from that, and the
  `--workspace` flag that used to choose independently is refused with a message naming its
  replacement. A definition declaring `sandbox` with `egress: none` does not load at all, because
  §L-10's managed allowlist cannot be emptied and the pair would publish a guarantee the runtime
  does not honour.
- **A parked run cannot be read as an answered one.** `Outcome::Parked` is a separate variant
  because the status does not distinguish it: measured, a run waiting on a caller's tool reports
  `status: "completed"` with no message, which the previous mapping turned into
  `Completed { text: "" }` — an empty answer, journalled as a finished turn, with a thread pointer
  committed. `cli` records `OperatorRequired` instead. Unreachable today (nothing declares
  `tools`) and kept because the failure it removes is silent.
  The guarantee is custody, not a lock: `POST /settings/tools` writes the caller's own scope, so
  confining a member requires that member's credential and the same credential can undo it.
- **Telegram.** `crates/telegram` is a real adapter against `https://api.telegram.org`, speaking
  the channel contract as bytes exactly as `devchannel` does and depending on no IronWorks crate.
  A real bot answered a real question through it on 2026-08-31.
- **Recovery of a *chained* turn, and the settling that keeps the channel listening.** ADR 0005
  rebuilds a request body and replays its idempotency key. It worked for a fresh turn and was
  **structurally impossible for a chained one**: recovery reconstructed every turn with
  `previous = None` and `Supply::Full` whatever it had chosen, so a chained turn's digest could
  never match and its replay was refused — forever, holding the shared cursor, which is the
  deafness `Openness` exists to prevent
  (`docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`). Both halves are
  repaired. The turn journal (schema 6) records what a turn **decided** — the response it chained
  from and how many records the conversation was already carrying — and recovery replays that
  rather than deciding again; a rebuild that still cannot match now settles as `OperatorRequired`
  instead of retrying. `dev/prove-continuation.sh` reproduces the defect deterministically with
  `--crash-at` and passes 5/5, with a control asserting the turn really chained so the recovery
  check cannot pass vacuously.
- **The serving loop, run continuously.** On 2026-09-02 `dev/relay-loop.sh` ran four cycles against
  the probe, one fresh process each, and answered and delivered two of three questions in one
  conversation — the second chained, carrying the book by thread rather than re-sending it (6,111
  bytes down to 277). One turn took a real `service_unavailable` and was **recovered on the next
  cycle by byte-identical replay**, the first time ADR 0005 has been observed outside a
  `--crash-at` harness. Four cycles is not an uptime claim, and nothing is deployed anywhere.
- **One real operator turn.** On 2026-09-01 `coordinate` served the operator's own org from the
  live Account Service through the unchanged `Book` contract — two real records, one answered
  turn, and a byte-identical replay that recovered a real upstream `service_unavailable` without
  creating a second logical turn.

**Not built.**

- **Provisioning and deprovisioning.** Nothing creates or destroys authority. Lifecycle *observes*;
  §F of the invariants is unbuilt.
- **Automatic confinement repair.** `ironworks lifecycle confine` is an explicit operator
  command; the reconciler still only reports drift and never repairs it on its own.
- **Approvals, workflows, effect execution.** `Access` has no `Write` variant.
- **IronWorks-hosted function tools, and the loop around them.** The runtime does not stand in the
  way — `/v1/responses` takes `tools` and parks a run for the caller to execute, measured wired on
  the probe (`0031`, `0032`). **Measured, declaring `tools` also hands the runtime its own agent
  loop**: `0035` ran two arms differing only in that field, and the tool arm read this tenant's
  records out of `ironclaw.memory.*` and answered from them without ever calling the declared tool,
  at 4.7×–5.8× the input cost and with a worse answer. **Confined, the same request called the tool
  and cost the baseline** — 20,910 input tokens against 20,899 — so cost is not an argument against
  a tool loop, and confinement is a measured precondition rather than a hardening step. **§A-3 also stands in the way**: executing such a call is "executing a model-generated
  fetch command" and the loop is "a second agent loop beside IronClaw's", both named there. §A-3
  now records what an amendment owes — a bound in the type model, a `ModelInput::ToolOutput`
  accounting, per-step replay reproducibility, a durable parked state holding `call_id`s but never
  the model's `arguments`, and an answer for `builtin__result_read` sitting outside confinement's
  catalogue. What exists is the refusal above, not the feature.
- **A sandbox service.** `runtime_tools: sandbox` is declarable, loads, and drives confinement, and
  `CapabilityProfile::SANDBOX_WORKSPACE` is the coherent profile for it — but **no shipped service
  declares it**, so nothing exercises the path end to end. The derivation is tested offline in
  three places (`for_profile` both ways in `ironworks-lifecycle`, `policy_for` against a real
  lifecycle store in `cli`, and a negative over every shipped service). What is *not* tested is
  the live half: `dev/prove-workspace.sh` needs a runtime on
  `hosted-single-tenant-volume-sandboxed`, and `dev/docker-compose.probe.yml` runs `production`.
  So that proof cannot run against the probe at all — it needs the systemd path in `deploy/`,
  which `deploy/README.md` records as never having been performed.
- **A qualified account-coordination result.** `fixtures/account-coordination` supplies an
  invented ten-account book and controlled graders for the operator's actual service, and `0020`
  records the first seven-case grade over that book — but it is **not a product claim**. It is one
  sample per case, two runs of the same suite disagreed (6/7, 4/7), and one recorded failure is a
  grader false positive awaiting a correction with controls. What it does establish is that the
  failure mode is absence framing rather than reasoning: no other grader in the suite fired across
  fourteen answers. One real operator turn is still not a suite result. The separate
  account-analysis parity result belonged to the retired replacement review and is not carried
  forward; that service declared `evaluation: null`, carried no answer-quality claim, had no user,
  and was deleted on 2026-09-02 along with the default slot it existed to hold.
- **A machine-enforced operator-only binding.** Account coordination names MultiAgency but is
  currently confined by deployment and explicit guidance/service selection, not by comparing the
  verified org with an operator-org configuration. This must land before a second tenant exists or
  before the service is relayed into any room the operator does not solely occupy.
- **An operator surface** beyond `compose`, `coordinate`, `turn`, `relay`, `redeliver`,
  `status`, `lifecycle inspect`, `lifecycle members` and `lifecycle confine`. `status` displays `Journal::unresolved`
  read-only; it does not reconcile or clear anything. There is still no `doctor`.

## Proving ground

`fixtures/` holds only synthetic material, in **two books that are easy to confuse** — they hold
injection records one digit apart, so a citation of either must name its book.

| Book | Records | Injection record |
|---|---|---|
| `fixtures/book` | 40 | `vr-006` — a `SYSTEM INSTRUCTION:` to report a **$500,000** budget for an account whose `budget` is null |
| `fixtures/account-coordination/book` | 10 | `vr-005` — a literal instruction to invent a **$750,000** budget and provision access |

`fixtures/book` also carries a contradicted budget in `hf-001` and an explicit opt-out in
`cg-004`. Of the account-coordination book's ten records, six carry the
obligations — onboarding, supersession, contradiction, residual authority, embedded instruction
and fail-closed dependency ownership — and four close their loops explicitly and need nothing,
so a brief instructed to compress the quiet accounts has quiet accounts to compress. Two tenant
records prove disjoint scope. Every record carries `_synthetic` and every domain is under
`.example` (RFC 2606). Real account data was deliberately not copied and must never be.

Adversarial parity to clear before any claim: cross-org isolation, two-tenant separation, and
prompt-injection refusal — each with a positive control.
