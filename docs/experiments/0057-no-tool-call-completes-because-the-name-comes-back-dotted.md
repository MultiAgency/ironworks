# 0057 — No tool call completes, because the name comes back dotted

**2026-09-07.** The first attempt to run `dev/prove-workspace.sh`'s sandbox block, which had never
executed. Disposable probe at `IRONCLAW_PIN` (`4cb47cfaf3`, image `ironclaw:4cb47cfaf`, label
`ironclaw.rev` matching the pin in full), profile `hosted-single-tenant-volume-sandboxed` via
`dev/probe-up.sh --sandboxed`, model `qwen/qwen3.5-397b-a17b` from `MODEL_PIN`, private `dind`
daemon, workspace root `/workspace-root`.

No transcript: this measures bring-up, tool dispatch and latency, not answers, and no grader is in
the path. The write-up is the evidence.

> **Superseded in part by
> `0059-the-sandbox-works-once-the-model-is-told-the-tools-other-name.md` (2026-09-08).** Telling
> the model the wire spelling dispatches the call, two probe defects behind it were fixed, and
> `dev/prove-workspace.sh` now scores 3 PASS — the sandbox is exercised. What stands from this
> record is the mechanism and its evidence; what does not is the title's "no tool call completes"
> and the BLOCKED verdict below. Left unedited, as a dated record.

**Verdict: BLOCKED. The sandbox is still unexercised, and §L-10's sentence still stands** — but the
reason is no longer unknown, and it is not the reason this repository's own documents assumed.
§L-10, `DESIGN.md`'s status block and this script's own header all attribute it to no shipped
service declaring `sandbox`. That is true, and it is not what stopped the proof.

---

## What was measured

`dev/prove-workspace.sh` drives three turns from three fresh journals: one writes a 64-hex value
inside the per-user sandbox, two independently read it back. Matching values are the positive
persistence control. **No value was ever written**, so there is nothing to compare and the run
carries no persistence result in either direction.

Getting to a legible failure took four defects out of the way first. Three were ours.

## 1 — The bring-up could not complete, and the store was not why

`dev/probe-up.sh` failed at `start_store` on every sandboxed attempt, reporting *"the record store
did not become healthy"*. That message named the wrong thing.

| what | measured |
|---|---|
| `load_book`, 10-record book | 0.16 s |
| `load_book`, 40-record book | 0.16 s |
| debug build, both books, idle machine | 0.12 s / 0.13 s / 0.12 s over three runs |
| release build, both books | 0.27 s |
| same command, during a sandboxed bring-up | 11 s, 23 s, and once over 30 s |

The store's own work is not the variable. `start_store` runs at the instant the runtime reports
healthy — while IronClaw is still assembling and the private daemon is still settling — and waited
6 s, against 120 s for the runtime wait beside it. There was never an argument for the twentieth.

Corrected to 60 s, and the fix is not only the number: the loop now checks the process, so a store
that **exited** fails immediately with the command to reproduce it, and only a live one is given the
window. Waiting longer must not become how a dead store is missed.

**A first attempt to fix this was reverted.** It raised the window to 30 s on the strength of the
11 s measurement alone; the next bring-up failed at 30 s, which meant the diagnosis was incomplete.
The contention measurements above came after. A fix whose justification does not survive the next
run is worse than the defect it hides.

## 2 — The proof asked a read-only analyst to run a shell command

The qualification service composed `personas/COORDINATOR.md` — the shipped account-coordination
analyst, whose Boundaries section ends *"If asked to do any of those, decline plainly and explain
that you are read-only."*

Every `workspace.create` turn that got far enough answered **"I cannot execute this command as
requested."** The persona was correct and the composition was wrong: nothing in the proof's own
service told the model it was running a qualification check rather than serving a tenant.

Replaced with `scrap/personas/_qualification-operator.md`. The alloy fell from **17,899 bytes
(`cb9fb7f5e02b1f93`)** to **2,691 bytes (`96ea9459b0b244af`)**, and later to **2,978
(`ee71b947d43e4e2c`)** when the persona gained the tool-naming paragraph §4 describes.

This is why §L-10's "unexercised" had two candidate reasons and neither was the real one: the
shipped-service reason was true, and the proof's own composition would have blocked it anyway.

## 3 — The proof had no recovery cycle, and now it does

Every `workspace.create` attempt outlived the runtime's synchronous wait and was refused
`service_unavailable`: once on the 40-record book (envelope 14,751 bytes, **not timed**) and once
on the 10-record book (envelope 6,357 bytes) at **30.97 s wall**. The envelope changing by a factor
of two without changing the outcome is what rules the envelope out; the single timing is what
places the boundary. `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30 s at
`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:55`, and its
builder `with_wait_timeout` has **no caller outside that crate's own tests** at the pin — so it
cannot be raised from outside the binary, and raising it inside would be a fork.

`prove-workspace.sh` drove one-shot turns and scored the refusal as a failed measurement. It now
drives one logical turn across up to four serving cycles over the same journal, retrying only on a
`degraded()` refusal. That is `0044`'s mechanism, reached by this proof for the first time, and the
runtime confirms it:

```
ironworks: recovering ironworks-e32f-… from journal evidence (SubmittedUnknown)
thread: replaying a fresh turn
ironworks: rebuild is byte-identical; replaying the same key
```

Same request digest, same key, one logical turn. The three journals stay separate; a cycle re-uses
only the journal of the turn it is recovering, so the sole product path joining the three remains
the runtime's per-user workspace.

**Recovery works and it does not help here**, which is the useful part: it carried the turn past the
wait and into the real failure below.

## 4 — The finding: the model returns a tool name the runtime forbids

With `IRONCLAW_REBORN_LOG=debug`, one failing turn reads:

```
resolved provider tool definitions   tool_definition_count=27
    tool_name_sample=["builtin__apply_patch", …, "builtin__shell", …]
received tool-capable provider response  finish_reason=ToolUse  tool_call_count=1
    tool_call_name_sample=["builtin.shell"]
rejected invalid provider tool name
    error=invalid provider_tool_name id 'builtin.shell':
          only ASCII letters, digits, '_', and '-' are allowed
… four model calls, each returning the same name and rejected the same way …
loop exit applied successfully  status=Failed
    failure=SanitizedFailure { category: "model_invalid_output",
                               detail: "model returned an invalid provider tool name" }
```

The log is the primary evidence for the mapping: capability `builtin.shell` is advertised to the
provider as `builtin__shell`, and the same `.`-to-`__` substitution is written out in their own test
at `ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/tests/trigger_poller_e2e.rs:124`, which
corroborates it rather than establishing it. So the model is offered `builtin__shell`. It returns `builtin.shell`, the capability-id spelling, which the
provider-tool-name validator at
`ironclaw@4cb47cfaf3/crates/contracts/ironclaw_host_api/src/ids.rs:408` refuses. The loop makes
four model calls, is rejected four times, and fails the turn. **Two turns did this identically** —
session-scoped `model_call` 2-5 and 6-9, four rejections each. The counter is session-scoped, not
per-run: call 1 was the tool-free warm-up, which succeeded.

**At this pin with this model, every call to `builtin.shell` failed this way, in every turn that
reached one.** The title generalises and the measurement does not: one tool was attempted. Turns
that call no tool are unaffected — the bring-up warm-up answered `ready` and delivered, alloy
`3770e0942624bda4`, 21,933 in / 51 out.

### What rules out our own prompt

The ask said *"Use builtin.shell"* and the first replacement persona repeated the dotted spelling, so
the obvious explanation was that the model was copying a string we handed it. It was not:

- Every dotted tool name was removed from the ask and from the persona, and the persona was given an
  explicit instruction to call tools by the name its own definitions give them.
- The alloy digest moved — `96ea9459b0b244af` → `ee71b947d43e4e2c` — confirming the change reached
  the composed prompt rather than only the file.
- The model still returned `builtin.shell`, and the rejection is byte-identical.

### What the API tells a client

Nothing usable. `GET /api/v1/responses/{id}` returns `status: "failed"` with
`error: { code: "internal_error", message: "An internal error occurred." }` — the sanitized value
`crates/ironclaw/src/model.rs::Outcome`'s own doc calls safe to log and useless for diagnosis. The
internal `SanitizedFailure` carries both a category and a detail; neither reaches the caller. **This
finding required raising the runtime's log level**, which is why `dev/docker-compose.probe.yml` now
takes `PROBE_LOG` (default `info`, unchanged).

That response also returned `"model": "reborn"`, which is §G-2 observed live rather than read from
source.

## Two smaller things, and one of them is ours

- **The sandboxed probe cannot restart onto an existing `ic-data` volume.** A recreated `ic`
  container exits 1 on
  `find: '…/sandbox-workspaces/.managed-egress': Permission denied`. That directory is mode `0711`
  owned by uid 1000; the probe runs the container as root with `cap_drop: [ALL]`, so it has no
  `DAC_OVERRIDE`, falls to the *other* bits, and can traverse but not list. **This is our compose
  file's doing, not upstream's.** The served unit sets no capability directives at all — it runs
  `User=ironclaw-%i` over `ReadWritePaths=/var/lib/ironclaw/%i`, and `deploy/README.md`'s install
  step creates that tree owned by the same account, so the process is the owner and the *owner*
  bits apply. **That last sentence is a source reading, not a measurement**: no served host has been
  restarted after a sandboxed turn, because none has run one. `dev/probe-up.sh` never meets this
  because it always builds a fresh volume; only an in-place recreate does.
- **The member's catalogue was not the obstacle, and was checked before it was blamed.** On this
  profile it reports 54 rows with `builtin.shell` present at `state: always_allow`,
  `effective_source: global`, `locked: false`, and `agent.auto_approve_tools: true`. The prompt
  surface carried 27 visible capabilities including `builtin.http` and `ironclaw.memory.*`, because
  this proof does not confine its member — consistent with §L-2 and not a finding.

## What this does not establish

One stack, one sitting, one model, one tool. The mechanism is the shared model gateway, so the
failure is **likely** general across IronClaw's built-ins — no other tool was attempted, so that is
unmeasured and must not be written as though it were. Nothing here measures whether the sandbox
workspace persists, because nothing ever entered it. Whether the dotted name originates in the
model, the provider, or something IronClaw puts in front of the model is **not established**: what
is established is the round trip, from advertised name to returned name to rejection.

Cost: ten or so billed turns across the sitting. The exact count is not recoverable — a recovery
cycle replays a key rather than minting one, and `usage.cost` is unusable at this pin (§G-7).

## What changed

- `dev/probe-up.sh` — `start_store` waits 60 s and distinguishes an exited store from a slow one.
- `dev/prove-workspace.sh` — one logical turn across up to four serving cycles; BLOCKED verdicts
  name the refusal instead of an exit code; no ask names a tool by its catalogue spelling.
- `dev/docker-compose.probe.yml` — `IRONCLAW_REBORN_LOG` takes `PROBE_LOG`, default unchanged.
- `scrap/personas/_qualification-operator.md` — new, and the reason the proof can reach a tool call
  at all.
- The runtime finding is registered as `docs/UPSTREAM.md` D10.
