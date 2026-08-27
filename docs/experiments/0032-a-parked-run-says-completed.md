# A parked run says `completed`, and the lane calls a tool confinement cannot reach

Date: 2026-09-01. **One external-tool round trip, measured end to end at the pin.
`0031-the-external-tool-lane-is-open.md` could not observe a parked run; this one did, and two
things it found are not what the source read suggested. There is no parked status — a run waiting
on a caller's tool returns `status: "completed"` — and resolving the call makes the runtime invoke
`builtin__result_read`, which is not in the settings catalogue confinement governs.**

## The run

Disposable probe at `4cb47cfaf3`, `IRONCLAW_REBORN_PROFILE: production`, model
`qwen/qwen3.5-397b-a17b` from `MODEL_PIN`. One declared tool:

```json
{"type":"function","name":"get_balance",
 "description":"Return the current ledger balance for an account.",
 "parameters":{"type":"object","properties":{"account_id":{"type":"string"}},
               "required":["account_id"]}}
```

**Step 1 — create.** HTTP 200 in 12.63s:

```json
{"id":"resp_d5b1…","status":"completed","model":"qwen/qwen3.5-397b-a17b",
 "output":[{"type":"function_call","id":"fc_call_f92f…","status":"completed",
            "call_id":"call_f92f187e5d584262bbf204d7","name":"get_balance",
            "arguments":"{\"account_id\":\"ACME-001\"}"}]}
```

**Step 2 — resume**, a continuation carrying one `function_call_output` and the
`previous_response_id`. HTTP 200 in 23.04s, a **new** response id, and an answer:
*"The current ledger balance for account ACME-001 is: $41,250.00 USD, as of August 31, 2026."*
Usage 9,609 in / 402 out.

## Finding 1 — there is no parked status

Both steps report `status: "completed"`. The six-variant enum in `DESIGN.md` has no
`requires_action`, and nothing in the response envelope says a call is outstanding. **A caller
distinguishes a parked run from a finished one only by reading `output`** — a trailing
`function_call` item with no assistant message after it.

That turns an existing line into something load-bearing it was not written to bear:

> Status is a six-variant enum … Only `completed` may advance a thread.

`completed` no longer implies *answered*.

## Finding 2 — IronWorks would journal it as a completed turn with an empty answer

`ironworks_ironclaw::to_outcome` maps `WireStatus::Completed` to `Outcome::Completed { text, usage }`,
where `text` is `output_text(&doc.output)`, which skips content parts that carry no `text`
(`model.rs:228-233`). The measured `function_call` item has **no `content` array at all**, so the
text is empty.

So a tools-enabled turn that parks would today be recorded as a completed turn, commit a thread
pointer, and deliver nothing. This is not a live defect — IronWorks sends no `tools`, so no run can
park — and `output_text`'s own test is right about what it does:

> `output_without_text_is_skipped_not_stringified` — *"A tool call is not an answer; rendering its
> JSON would put braces in front of a person."*

Right about rendering. The gap is the question nobody downstream asks: **is a call outstanding?**
Anything enabling `tools` must answer that in `Outcome` before it answers anything else, because
every path after it — `TurnOutcome::Completed`, the thread pointer, delivery — treats a completed
turn as one holding an answer.

## Finding 3 — the lane invokes a tool confinement cannot see

The resume response replays the whole step history, and it is longer than what was submitted:

1. `function_call` — the original `get_balance`
2. `function_call_output` — the submitted result, **rewritten by the runtime**
3. `function_call` — `builtin__result_read`, which nobody asked for
4. `function_call_output` — its result
5. `message` — the assistant's answer

The submitted output does not reach the model as bytes. It is stored and handed over as a
reference:

```json
{"schema_version":1,"status":"success","trust":"untrusted_tool_output",
 "summary":"Tool completed; preview contains the full result.",
 "detail":{"kind":"result_reference","byte_len":56,"total_bytes":56,
           "preview":"\"{\\\"balance_usd\\\": 41250.00, …}\"",
           "result_ref":"result:dfd4977f-….12bd8e2c-…"}}
```

and the model then calls `builtin__result_read` with
`{"max_bytes":24576,"offset":0,"result_ref":"result:…"}` to read it back.

**`builtin.result_read` is not one of the settings rows.** §L-1 lists it among the *eight
captured-surface-only* ids — present on the host-captured capability surface, absent from the
catalogue `GET /settings/tools` returns and `ConfinementPolicy` writes against. So:

> A service that declares `tools` depends, on every single call, on a capability that confinement
> cannot enable, disable, or observe.

§L-7 already says the confinement claim covers the settings catalogue and not all executable
authority. That was an honest caveat about an enumeration gap. **It now has a concrete instance
that a tools-enabled service exercises continuously**, which is a different thing from a gap
nobody had walked through.

## Finding 4 — the runtime marks caller tool output untrusted, and mediates it

`trust: "untrusted_tool_output"` is the runtime's own classification, applied to bytes the *API
client* submitted, and access is through a reference rather than inline. That is the same posture
as the envelope's provenance lines in `ironworks_domain::envelope` — evidence to assess, not
instructions to follow — arrived at independently and enforced by the runtime rather than by
wording. Worth knowing before writing our own provenance text for tool results: there is already
some, and it is not ours to contradict.

## Finding 5 — each step is its own 30-second window

12.63s + 23.04s = 35.67s of wall clock across two waits. Neither step came close to the 30s
`DEFAULT_RESPONSES_WAIT_TIMEOUT` on its own; together they exceed it. A loop is therefore **N
creates and N waits**, not one long call, and ADR 0005's recovery obligation applies *per step* —
each resume needs its own reproducible body and its own idempotency key. A three-call tool loop on
a slower question is where the 503 path stops being hypothetical.

## What this does not establish

- **The served profile.** Measured on `production`, as `0031` was. Whether
  `hosted-single-tenant-volume-sandboxed` wires the ports is still unmeasured.
- **Idempotent replay of a resume.** Each step carried a distinct `Idempotency-Key` and neither was
  replayed. ADR 0005's byte-identity property is measured for create; it is **assumed, not
  measured, for resume.**
- **Parallel or repeated calls.** One tool, called once. Nothing here measures two outstanding calls
  in one park, or a second park after a resume, though the source says the latter happens
  (*"or park on a further call"*).
- **Confinement interaction.** The member here was unconfined. Whether `builtin__result_read`
  still works under `read_only_analyst()` is the obvious next question and was not asked.
