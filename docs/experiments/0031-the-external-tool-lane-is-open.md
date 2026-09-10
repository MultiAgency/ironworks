# The external-tool lane is open, and `DESIGN.md` did not know it

Date: 2026-09-01. **`POST /v1/responses` accepts `tools` and `tool_choice` at the pin, and the
probe deployment has the ports wired. IronWorks sends neither. What changes is that sending none
is now a decision this repository makes, not a constraint the runtime imposes — and the contract
section said the field list was complete when it was not.**

## What prompted it

A reading of the tree asked what "enable tools" would mean here. `DESIGN.md`'s upstream contract
opens:

> `POST /v1/responses` takes `model`, `input` (string or items), `instructions`,
> `previous_response_id`, and an IronClaw extension `x_context`.

That reads as an enumeration of what the surface accepts. Two fields are missing from it, and
`docs/INVARIANTS.md` §L-2 derives the empty confinement allow-set partly from IronWorks' request
body having no `tools` field — a sentence that is true about our body and easy to read as a claim
about the surface.

## The source, at `4cb47cfaf3`

`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses.rs:25-27` carries both fields:

```rust
pub tools: Option<Vec<serde_json::Value>>,
pub tool_choice: Option<serde_json::Value>,
```

`external_tools.rs` says what they are and, more usefully, what IronClaw refuses to do with them:

> A client-declared function tool parsed from a Responses `tools` entry. The model may call it;
> **the host never executes it** — a call parks the run and the call is handed back to the API
> client as a `function_call` output item.

Only `type: "function"` is accepted; any other tool type is a stable `400` naming `tools`. The
parse boundary caps are 256 tools, a 64-byte name, an 8 KB description and a 64 KB schema.

The lane is **feature-gated by host wiring**. `external_tool_store` and `external_tool_resume` are
both `Option` on the workflow (`responses_workflow.rs:80-88`), and `external_tools_enabled()`
requires both. When they are absent, `tools`, `tool_choice` and `function_call_output` fail closed
with a stable `400`.

The loop, when they are present, is a continuation rather than a second endpoint: the client
executes the parked call and re-posts to `/v1/responses` carrying `function_call_output` items and
the `previous_response_id`, which resumes the parked run and "waits for it to complete (**or park
on a further call**)" (`responses_workflow.rs:889-892`). Each step therefore re-enters the same
30-second wait recorded elsewhere in `DESIGN.md`.

## The discriminating measurement

Whether a given deployment has the ports wired cannot be read from the source, so it was measured.
The two validators differ in a way that makes one request separate them, with no turn submitted by
either branch — `validate_responses_request` is the first statement of the submit path
(`responses_workflow.rs:177`), before any reservation or dispatch:

| | unwired (`validate_responses_supported_fields`) | wired (`…_with_external_tools`) |
|---|---|---|
| non-empty `tools` | 400 `param: tools` | parsed and registered |
| `tool_choice` present | 400 `param: tool_choice` | not inspected |
| `function_call_output` without `previous_response_id` | not inspected | 400 `param: previous_response_id` |

So a request carrying **`tool_choice` and a `function_call_output` item and no
`previous_response_id`** returns `tool_choice` if the ports are absent and `previous_response_id`
if they are present. Both are 400; neither reaches a model.

Run against the disposable probe on `docker-compose.probe.yml`
(`IRONCLAW_REBORN_PROFILE: production`), image labelled `ironclaw.rev=4cb47cfaf3`, via
`POST /api/v1/responses`. `model` was the deliberately invalid string `"x"` throughout, so any
request that did get past validation could not reach a provider:

| request | result |
|---|---|
| A — `tool_choice` + `function_call_output`, no `previous_response_id` | **400 `param: "previous_response_id"`** |
| B — `function_call_output` alone, no `previous_response_id` | **400 `param: "previous_response_id"`** |
| C — one valid `type: "function"` tool, text input | **200**, submitted; `status: failed`, `internal_error` — the invalid model, not the tools |

A is decisive: `previous_response_id` is unreachable from the unwired validator, which would have
answered `tool_choice` and stopped. B confirms it without the `tool_choice` term. C shows the
positive direction — a declared function tool is accepted where the unwired path answers 400
`param: tools`.

## What this establishes, and what it does not

**Established.** At `4cb47cfaf3`, the Responses surface accepts client-declared function tools and
`tool_choice`; the host parks rather than executes them; the resume protocol is a continuation
carrying `function_call_output` plus `previous_response_id`; and on the probe stack both host
ports are wired.

**Not established, and the gap matters.** No turn completed, so **no parked run was observed** —
this measured the validator and the accept path, not a `function_call` output item, a `call_id`, or
whatever status a parked response reports. Anything about the parked shape is still source-read
only.

**Not established, and it is the one that would gate a deployment.** The probe runs the
`production` profile. A served deployment runs `hosted-single-tenant-volume-sandboxed`
(`deploy/ironworks-ironclaw@.service`), and port wiring is a composition-layer decision that the
profile may or may not carry. **Do not read this run as "external tools are available in
production."** It says the probe has them.

## What it does not change

**Confinement.** `ConfinementPolicy::read_only_analyst()`'s empty allow-set governs IronClaw's
*built-in* capability catalogue — the tools its own loop can invoke. Client-declared function tools
are a different lane, registered per request, executed by the caller. Enabling one says nothing
about the other, and §L-2's derivation stands unaltered: IronWorks sends no `tools`, records reach
the model as gated input, and every built-in grants authority a record-only turn does not need.

**§L-7 still bounds the claim.** Nothing here widens what confinement is measured over.

## The consequence worth carrying

*"IronWorks registers no caller-hosted function tools"* was load-bearing in two documents and read
in both as a property of the runtime. It is a property of our request body. The runtime offers the
lane; this repository declines it. That distinction is the whole difference between a limit and a
choice, and a choice is the thing that has to be argued for.
