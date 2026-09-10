# What the seam never asked for

Date: 2026-09-02. **One disposable probe at the pin, one sitting, eight measurements. Four
questions this repository records as open were answerable in it, and one recorded disagreement about
a catalogue size turned out to be two correct readings of two different callers.**

The occasion was an alignment review: the seam issues six distinct upstream path shapes against a
surface an order of magnitude wider, and every large omission is a recorded decision. This asked
whether any of the omissions were decisions nobody had actually taken.

## The run

`dev/probe-up.sh` at `4cb47cfaf3`, image labelled `ironclaw.rev=4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`,
`IRONCLAW_REBORN_PROFILE: production`, model `qwen/qwen3.5-397b-a17b` from `MODEL_PIN`. The runtime's
own active provider model is `deepseek-ai/DeepSeek-V4-Flash`, which matters for check 6.

Two callers are used throughout and the distinction is the point:

- **operator** — the WebUI env bearer, which `dev/prove-confinement.sh` also uses as its member;
- **member** — a real one, minted by `POST /api/webchat/v2/admin/users`, whose returned token
  carries `"op":false` and an `exp` exactly 31,536,000 s after its `iat`, independently confirming
  §F-4's compiled-in 365-day session lifetime.

## 1 — `locked` is derived from the capability, not set by anyone — PASS

`docs/UPGRADE.md` asks whether an operator can *lock* a disabled state, which would make confinement
non-overridable rather than custody-backed, and calls it "not measured; worth a probe before the
confinement slice".

The pinned source answers it without a probe
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/reborn_services.rs:1951-1962`):

```rust
fn tool_permission_locked(tool) = tool.default_permission == Deny || hard_floor_tool(tool)
fn hard_floor_tool(tool)        = tool.effects ∩ {Financial, ModifyApproval, ModifyBudget} ≠ ∅
```

`locked` is computed per read from the capability's own declared effects
(`.../reborn_services.rs:1809`), with `mutable` its negation. **There is no write path that sets
it** — no operator lock, no tenant lock, no settings key. The `precedence` list the bump noticed is
a description of how a state was resolved, not a control.

**Confinement is therefore necessarily custody-backed at 1.4.0.** §L-6 is right, and it is right
structurally rather than by measurement. The question closes.

## 2 — the two undisableable rows are exactly the two the catalogue already flags — PASS

`ironworks_lifecycle::confinement::UNDISABLEABLE` is a literal two-element list. Both its members
declare `effects: vec![EffectKind::ModifyApproval]`
(`ironclaw@4cb47cfaf3/crates/extensions/ironclaw_extension_manager/src/operator_config_capability.rs:80,109`),
so both are `hard_floor_tool`. Measured on the operator's fresh catalogue — 53 tool rows plus
`agent.auto_approve_tools`:

```
locked:true rows: 2
    tool.builtin.operator_config_set_auto_approve       state=ask_each_time mutable=False source=locked
    tool.builtin.operator_config_set_tool_permission    state=ask_each_time mutable=False source=locked
locked:false rows: 51
rows with NO locked field: 0
```

The discriminating write pair, which is what makes this a measurement rather than a restatement of
§L-4:

```
builtin.operator_config_set_tool_permission  disabled     -> 400 {"error":"invalid_request","kind":"validation","field":"state","validation_code":"invalid_value"}
builtin.operator_config_set_auto_approve     always_allow -> 400  (identical body)
builtin.time                                 disabled     -> 200  {"...","locked":false,"state":"disabled"},"mutable":true
builtin.time                                 always_allow -> 200  (restored)
```

So the derived flag and the observed refusal agree, and the ordinary-row control agrees in the other
direction. §L-4 already recorded reading `locked: true` back from exactly these two rows;
`ironworks_reconcile::ironclaw::BoundMember::observe`'s catalogue reader parses `key` and
`value.state` and reads past `locked`, `mutable`, `default_state`, `source` and `effective_source`.
**The list the repository hand-maintains is one the runtime already reports.**

This is not a defect today — the literal list is correct at this revision. It is a drift surface: a
later capability that gains a hard-floor effect becomes undisableable, `confinement::plan` queues a
write that answers 400, and `apply` stops at the first failure by design, so the whole plan halts on
a row nothing knew to skip.

## 3 — the catalogue size depends on who reads it — PASS, and it settles a disagreement

§L-1 records 53 settings rows; `0012` recorded 52, and the difference was resolved as
`nearai.web_search` being missing from one enumeration. Measured here, both readings were right:

```
operator rows: 53   member rows: 52
operator-only: ['tool.nearai.web_search']
member-only:   []
member state histogram: {'always_allow': 50, 'ask_each_time': 2}
member agent.auto_approve_tools: True
```

**The catalogue is caller-relative.** The operator sees `nearai.web_search`; a freshly minted member
does not. Both are "a fresh member's catalogue" under different readings of *member*.

This bears directly on §L. Confinement authenticates **as the member** (§L-6), so the catalogue
`ConfinementPolicy` actually writes against is the 52-row one, not the 53-row one §L-1 prints. The
floor in §L-8 — "50 disabled + 2 locked" — is stated against the member's 52 and is unaffected;
what changes is that §L-1's enumeration is an *operator's* view and must say so.

## 4 — replaying an in-flight idempotency key returns the id immediately — PASS

`DESIGN.md` records replay against a failed turn (1.3.0) and a completed one (1.4.0). This is the
third case, and it was not recorded: replay of a key whose turn is **still running**.

Original create issued in the background; replay of the same key with a byte-identical body 6 s
later, while create was still blocked:

```
HTTP 200 in 0.006306s
{"id":"resp_f58a07b5b9bd43a6ab1332b2517627f7","object":"response","created_at":1788368413,
 "status":"in_progress","model":"qwen/qwen3.5-397b-a17b","output":[]}
```

Meanwhile the original create ran on and returned `503 service_unavailable` at 30.053 s, and the
turn completed server-side — reproducing `0036`'s cliff exactly.

The consequence is not small. `DESIGN.md` says of the timeout that "no response id was returned to
poll", which is true **of create** and has been read as though the id were unobtainable until the
turn ends. It is obtainable at any moment, at 6 ms, by replaying the key the seam already minted.
Everything in checks 5 and 6 depends on that.

## 5 — a turn is cancellable, but not through the route named for it — PASS / FAIL

`POST /api/v1/responses/{id}/cancel` is mounted at the pin
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/descriptors.rs:27-37`) and its
handler `cancel_response` is fully implemented, delegating to `CANCEL_RUN_COMMAND`. **It does not
work.** Measured against one run, in both states, and against a second run:

```
in_progress  -> 400 {"error":{"message":"The request is invalid.","type":"invalid_request_error",
                     "param":null,"code":"invalid_request"}}   status after: in_progress
completed    -> 400 (identical)
unknown id   -> 404 {"...","param":"response_id","code":"not_found"}
```

Neither of the compat handler's own guards produces that shape — both name `response_id` as the
param — so the 400 arrives from the command invocation beneath it. **Why is not established here**,
and it is upstream's to answer.

The webchat route, against the *same run in the same state*, works:

```
POST /api/webchat/v2/threads/{thread_id}/runs/{run_id}/cancel  {"client_action_id":"probe4-…"}
-> 200 {"run_id":"0b7179f1-…","status":"CancelRequested","event_cursor":21,"already_terminal":false}
```

and the blocked create then returned, **before its 30 s wait elapsed**:

```
{"id":"resp_e7adab444a4c4686a5cf34169fcd3047","status":"cancelled","model":"qwen/qwen3.5-397b-a17b","output":[]}
```

On an already-finished run the same route answers `200 {"status":"Completed","already_terminal":true}`
— idempotent and self-describing, which is what a best-effort caller needs. Omitting
`client_action_id` answers `400 … "field":"client_action_id","validation_code":"missing_field"`; a
colon in that field is accepted, so the compat handler's `"{id}:cancel"` is not the cause.

So `Outcome::Cancelled` — modelled in `ironworks_ironclaw::model` and today unreachable — is
reachable. Every turn `0036` counted as a 503 ran to completion and billed; eleven of seventeen. The
loop that stops one is **replay the key (check 4) → read `run_id` (check 6) → cancel**, and it uses
one route the seam does not yet call.

## 6 — every response id is a thread, with a resumable projection — PASS

Create issues `ProductCreateThreadRequest { requested_thread_id: <response public id> }`
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:768-819`).
Measured with the member bearer that created it:

```
GET /api/webchat/v2/threads/{response_id}/timeline -> 200
  {"thread":{"thread_id":"resp_e449959f…","metadata_json":"{\"client_action_id\":\"resp_e449959f…\"}", …},
   "messages":[{"kind":"user","status":"submitted",…},{"kind":"assistant","status":"finalized",…}]}

GET /api/webchat/v2/threads/{response_id}/events   -> SSE
  event: projection_update
  data: {"cursor":"…\"read_scope\":{\"thread_id\":\"resp_e449959f…\"}…",
         "state":{"items":[{"text":{"run_id":"f27f6fa5-…","body":"# A Treatise on the History of …
```

Three things follow. The `run_id` check 5 needs is here. The stream carries **progressive answer
text**, so a turn's output is observable while it runs rather than only after it terminates. And the
frame's `cursor` is the resume token — `Last-Event-ID` or `?after_cursor=` — so a dropped connection
resumes from stored state.

That last point is the one that bears on `docs/adr/0002-create-then-poll-not-streaming.md`. The ADR declined `stream: true` because "a
dropped stream would reopen exactly the ambiguity this design exists to close", and **that reasoning
is correct and is not disturbed**: it is about the create call's own response body. The thread
projection is a different object, read on a different route, after create has returned.

**Corrected 2026-09-02, the same day, by
`docs/experiments/0040-the-cost-field-misses-the-pin-twice.md` finding 4.** This paragraph
read:

> One thing worth recording that nobody asked: `GET /api/webchat/v2/threads` returns each thread
> with a `title` holding the **raw request input** … For the seam, the request input is the rendered
> envelope, which carries a tenant's records. They are durably readable on a second route … a second
> place a tenant's records live, and §B-7's audience rule should be read against it.

**It was a misreading of a console line this run had itself truncated, and it was wrong three
ways.** The title is a derived sidebar label capped at 60 characters, taken from the first non-empty
line of the earliest user message; derivation is read-time only and nothing is persisted
(`ironclaw@4cb47cfaf3/crates/domains/ironclaw_threads/src/title.rs`). Measured with a marker planted
at the front of the input, every thread carried the same 60 characters — the compat layer's JSON
wrapper prefix — and the marker did not appear. Not the raw input, not durable, and not record
bytes. §E-11 and PRD §19.2 are untouched.

It is kept rather than deleted because the shape of the mistake is worth having on record: a
`…`-truncated value printed by this run's own helper was read as a truncation performed by the
runtime, and the conclusion was drawn from the ellipsis rather than from a measurement.

## 7 — the runtime does isolate a response by member — PASS, and §I's gap closes

`docs/INVARIANTS.md` §I records this as an open bound: `GET /api/v1/responses/{id}` is fetched with
the tenant's own credential and "nothing here has measured what happens when the id belongs to a
different member. Looked for in `docs/experiments/` and in §L; not found."

Measured, with the operator's response id and the minted member's token:

```
member  GET /api/v1/responses/{operator's id}          -> 404 {"…","param":"response_id","code":"not_found"}
member  GET /api/webchat/v2/threads/{same id}/timeline -> 404 {"error":"not_found",…}
operator GET /api/v1/responses/{same id}               -> 200 status=completed      (positive control)
```

The positive control is what makes it a measurement: the same id is readable by its owner in the
same sitting. **Defence in depth is real here**, and `TurnRecord::belongs_to` is not carrying the
isolation alone. It remains correct that IronWorks must not *rest* on this (§B-13, §B-14).

## 8 — the model policy refuses off-policy models, and the member cannot lift it — PASS

`MODEL_PIN`, §G-2 and `0013` establish that the pin "fails silently rather than closed, and that is
structural" — a claim about the **provider** honouring a per-request `model`. Nothing here disturbs
it. What this measures is a different boundary: whether the *host* will refuse a model.

Written with the operator token:

```
PUT /api/webchat/v2/llm/model-policy {"workspace_default":"qwen/qwen3.5-397b-a17b",
                                      "allowed_models":["qwen/qwen3.5-397b-a17b"]}
-> 200 {"selection_enabled":true,"workspace_default":"qwen/qwen3.5-397b-a17b","models":["qwen/qwen3.5-397b-a17b"]}
```

Then, on `POST /api/v1/responses`:

```
model: deepseek-ai/DeepSeek-V4-Flash   -> 400        (the runtime's own active model)
model: qwen/qwen3.5-397b-a17b          -> 200 completed, echoed qwen/qwen3.5-397b-a17b
model: some/not-a-real-model           -> 400
```

**The control that makes this a measurement rather than a coincidence** — the 400 could have been
"model unavailable" rather than "model not permitted" — is to widen the policy and repeat the
identical request:

```
PUT … allowed_models:["qwen/qwen3.5-397b-a17b","deepseek-ai/DeepSeek-V4-Flash"] -> 200
model: deepseek-ai/DeepSeek-V4-Flash  -> 200 completed, echoed deepseek-ai/DeepSeek-V4-Flash
```

Same request, refused under the narrow policy and served under the wide one. The refusal is the
policy.

And the authority shape is the finding. Written as the minted member:

```
PUT /api/webchat/v2/llm/model-policy  -> 403  Operator WebUI configuration privileges required
```

which matches the source requirement `caller.operator_config`
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_operator/src/llm_admin/llm_config_service.rs:841-846`).
**This is the first upstream control measured here that a member cannot undo.** Every settings-tools
write is custody-backed by §L-6; this one is not. It is also absent from the 52- and 53-row
catalogues entirely, so confinement neither governs nor observes it.

It does **not** make the pin auditable — §G-2's `"reborn"` echo on retrieval is unchanged, and a
provider that bakes its own model still would. It bounds which model a request may *name*, from the
far side of the trust boundary, with an authority the credential the seam custodies cannot reach.

## Verdicts

| # | Question | Verdict |
|---|---|---|
| 1 | can an operator lock a disabled state? | **PASS** — no; `locked` is derived from the capability's effects |
| 2 | is `UNDISABLEABLE` derivable from the catalogue? | **PASS** — exactly the two `locked: true` rows, with both controls |
| 3 | is the fresh-member catalogue 52 or 53? | **PASS** — 53 to an operator, 52 to a member; `nearai.web_search` is the difference |
| 4 | can the id of a running turn be obtained? | **PASS** — replay returns it in 6 ms with `in_progress` |
| 5 | can an abandoned turn be stopped? | **PASS** via the webchat route; **FAIL** for `POST /v1/responses/{id}/cancel`, which 400s in every state tested |
| 6 | is a turn observable while it runs? | **PASS** — cursored SSE projection with progressive text and the `run_id` |
| 7 | does the runtime isolate a response by member? | **PASS** — 404 cross-member, 200 for the owner |
| 8 | can the host refuse an off-pin model? | **PASS** — refused 400, restored by widening the policy, and member-unwritable (403) |

## What this does not establish

The `production` profile is not the served profile. §L-10 and `dev/prove-workspace.sh` need
`hosted-single-tenant-volume-sandboxed`, and **none of checks 4 through 8 has been repeated there**
— the external-tool lane's wiring already differs by profile (`0031`), so nothing here should be
read as a property of the served topology until it is re-run against one.

*Retired the same day by `0039-the-catalogue-has-two-axes.md`, which stood that stack up and
repeated checks 4 through 8 on it. All five reproduced; check 3's caller axis gained a second,
independent profile axis. This paragraph is left as written — it records what this run established
and what it did not.*

Check 5 does not explain the compat cancel's 400; it measures it. Check 6 observed one turn's stream
and did not test resumption after a real drop, or the `SSE_MAX_LIFETIME` close. Check 8 measured one
provider with one active model, and did not test what the policy does to a *chained* turn or to a
turn submitted with no `model` field at all.

Nothing here measured a model's answer. Every check is a contract question, which is why the
warm-up turn is the only one whose content mattered.
