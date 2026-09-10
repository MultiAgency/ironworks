# Resolved: a runtime log named the provider's configured model, not the turn's

Date: 2026-08-31. **Resolved from source. The log line named the provider's configured model,
not the one that served the turn. No unpinned model served anything. What survives is that the
pin fails silently rather than closed — a property of the provider, not of this repository.**

## What was seen

While measuring confinement on the disposable probe (`4cb47cfaf3`, `IRONCLAW_REBORN_PROFILE:
production`), two turns failed with `internal_error`. The runtime log explained the failure — a
provider rate limit, not anything to do with confinement — and named a model in passing:

```
WARN ironclaw_llm::retry: Retrying after transient error (tools streaming)
     provider=deepseek-ai/DeepSeek-V4-Flash
     error=Provider nearai_chat rate limited, retry after Some(2s)
```

`MODEL_PIN` names `qwen/qwen3.5-397b-a17b`. Across that container's whole log: **two mentions of
DeepSeek, zero of qwen.**

## Why it matters more than an ordinary discrepancy

`MODEL_PIN` is described in this repo as *a privacy claim, not a version string*, and the rule
about it is absolute: **never add a fallback literal for a pin**, because *"a default is the one
value that can silently outrank the configuration."* If a turn's work reaches a provider the pin
does not name, the claim the pin exists to make is not true, and no amount of care on our side
about `model` in the request body would make it true.

`DESIGN.md` already records the audit gap: **[measured @ 1.3.0] `model` is echoed on create but
not on retrieval** — `GET` reports the literal string `"reborn"`, so which model produced a stored
answer is unverifiable after the fact. That was filed as "the pin is a construction-time invariant
we cannot audit post-hoc". This is the first observation suggesting the invariant may not hold
even at construction time.

## Why this is not yet a finding

Both log lines come from a **`tools streaming` retry path**, not from the main completion. Three
readings fit equally well and nothing here separates them:

1. IronClaw uses a different model for a tool-planning or streaming pass than for the answer.
2. The provider (`nearai_chat`) routes our named model elsewhere.
3. Only the retry path logs a provider at this level, so the main completion's model was never
   printed and the sample is not the population.

Reading (1) would be unremarkable; (2) would be serious; (3) would mean there is nothing here at
all. **Three plausible readings and one artifact is exactly the shape of the two mistakes made
earlier in the same session** — a naming convention read as a bypass, and an offered tool list
read as authority. So this is recorded as a question.

## Step 1 done: the pinned source explains it, and narrows the claim

Read at `4cb47cfaf3`. `model` on `/v1/responses` is **advisory, not a selection**.

`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/model_validation.rs`:

> Reborn carries the requested `model` string through to the projection reader **as a
> composition/policy hint** and echoes it back in the response.

Validation bounds only length and characters — its own tests accept `gpt-4`, `reborn` and
`anthropic/claude-opus-4` indifferently. The string becomes a
`LoopModelRouteSnapshot::Advisory { model_id }`, in which the other three route components are the
literal sentinel `"requested"`, and
`ironclaw@4cb47cfaf3/crates/contracts/ironclaw_loop_contracts/src/host/run_context.rs` states the consequence:

> Advisory routes exist so a caller can request a model without an operator-approved route
> binding: the non-routed gateway **honors the model id when its provider supports per-request
> overrides and otherwise falls back to the active model**, while routed hosts still validate the
> route and fail closed.

So a non-routed deployment may serve a turn on its active model while echoing back the model that
was asked for — and since `GET` reports the literal `"reborn"`, nothing downstream can tell which
happened. The DeepSeek line needs no bypass to explain it.

**What the pin can support, and what it cannot.** `DESIGN.md` calls it *"a construction-time
invariant on the request"* that cannot be audited post-hoc. The first half holds; the second was
doing more work than it could bear:

- Supportable: **every request we construct names the pinned model.**
- Not supportable: **every turn is served by the pinned model.**

The gap between them is *provider supports per-request overrides* and *routed vs non-routed host*
— neither of which IronWorks measures, controls, or can currently detect. For a pin described as
a privacy claim rather than a version string, that gap is the whole claim.

## Steps 2 and 3 done: the pin is probably honoured, and it fails silently if it is not

`ironclaw@4cb47cfaf3/crates/loop/ironclaw_loop_host/src/model_gateway.rs`, `request_model_override`:

> Providers that honor per-request overrides (**e.g. NEAR AI**) serve the requested model;
> providers that bake the model at construction ignore it and fall back to their active model —
> the "route if the provider can serve it, else fall back" behavior, **decided at the provider
> boundary rather than a route allowlist**.

The precedence is caller-requested model, then the profile's `model_override`, then
`provider.active_model_name()` only when nothing was requested. IronWorks always sends a concrete
model, so the fallback branch is not reached, and the provider we run against is the one the
comment names as honouring overrides.

**So the completion is probably served by the pinned model.** Two things stop that being an
all-clear, and they matter more than the reassurance.

**The pin fails silently, not closed.** Whether it is honoured is a property of the *provider*,
not something the request can enforce — "decided at the provider boundary rather than a route
allowlist". Swap the provider for one that bakes its model at construction and every turn serves
the active model instead, while IronWorks changes nothing, errors nowhere, and notices never. A
routed host would validate and fail closed; a non-routed one degrades quietly. For a pin that is
described as a privacy claim, **the absence of a detectable failure is the finding**, and it is
structural rather than a bug.

**And it does not explain the DeepSeek line.** If NEAR AI honours the override and every request
names qwen, a completion should be served by qwen — yet the only model named anywhere in that
container's log was `deepseek-ai/DeepSeek-V4-Flash`, twice, both in `tools streaming` retries.
That now points at the first of the three readings above: **a separate tool-planning or streaming
pass selecting its own model**, distinct from the completion.

## The log line explains itself: it prints the wrong name

`ironclaw@4cb47cfaf3/crates/domains/ironclaw_llm/src/retry.rs`:

```rust
tracing::warn!(
    provider = %self.inner.model_name(),
    ...
    "Retrying after transient error{label}"
```

The field labelled `provider` carries **`model_name()`** — the provider's *configured* model, what
it was constructed with. Not `effective_model_name(requested_model)`, which is what a per-request
override resolves to. Both exist on the same trait, adjacent; the retry logs the one that answers
"how is this provider set up" rather than "what served this request".

So the whole observation resolves as: the probe's runtime is configured with DeepSeek as its base
model, our request names qwen, NEAR AI honours per-request overrides, and the retry printed the
configuration. Nothing served a turn on an unpinned model, and nothing needed to.

**The auxiliary-pass hypothesis was also wrong.** `" (tools streaming)"` labels
`complete_with_tools_streaming` — the *main* completion with tools attached, in streaming mode.
There was no second planning pass to be suspicious of. That was the third bad reading of an
artifact in one session, and the same shape as the other two: a name taken at face value.

## What is left, and it is the part worth keeping

The alarming reading is gone. The structural finding is not:

> The pin fails **silently**, not closed.

Whether it is honoured is decided at the provider boundary. A provider that bakes its model at
construction ignores the request and serves its active model, and IronWorks changes nothing,
errors nowhere, and notices never. On this stack the provider does honour it — but that is a fact
about NEAR AI, not a property IronWorks holds, and it is exactly the kind of fact that stops being
true without anybody editing this repository.

**One cheap thing would close the gap**, and it is worth recording as the ask rather than doing
blind: `effective_model_name(requested_model)` already exists on the provider trait and already
answers "what will actually serve this". Logging it at submit — or exposing it on the response —
would let a turn's model be *observed* instead of assumed. That is the upstream question this
whole thread reduces to:

> Can a host report, or fail closed on, the model that actually served a turn — rather than
> echoing the one that was requested?

`DESIGN.md`'s wording needs no repair: *"the pin is a construction-time invariant on the request"*
is exactly right, and its "cannot be audited post-hoc" is the same gap named here from the other
side.

## Superseded



*Steps 2 and 3 as originally written — "is this deployment routed" and "does `nearai_chat`
support per-request overrides" — are both answered above by the provider-boundary comment, which
makes the routed/non-routed distinction moot for this provider. The upstream question they were
building toward is still worth asking, and is now narrower: **can a host be made to fail closed
when a provider cannot honour the requested model, rather than falling back to its active one?**
That is the behaviour a pin needs and the one the source says does not exist.*

Until 2 and 3 are answered, **do not repair the wording in `DESIGN.md` by weakening it to match
this note.** The narrower claim above is the one the source supports; whether the broader one is
*false here* depends on the two measurements, and stating either outcome now would be asserting an
unmeasured thing about the claim that matters most.
