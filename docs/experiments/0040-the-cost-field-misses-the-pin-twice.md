# The cost field misses the pin twice, in opposite directions

Date: 2026-09-02. **`usage.cost` reports `"0"` for a turn on the pinned model and, for the same
turn read back a second later, GPT-4o's prices. Neither describes the model that ran. The two
numbers have unrelated causes, and the first reading of this — "create does not price, fetch does"
— was wrong.**

Also here: a retraction of a claim `0037` made about thread titles, and the source answer to a
question `0037` left as a hypothesis.

Measured on a disposable probe at `4cb47cfaf3`, `production` profile.

## The observation, and the wrong explanation it invited

Three fresh turns on `qwen/qwen3.5-397b-a17b`, each read back immediately:

```
run 1  create  {"input_tokens":12737,"output_tokens":43,"cost":{"input_cost_usd":"0",…,"total_cost_usd":"0"}}
       GET     {"input_tokens":12737,"output_tokens":43,"cost":{"input_cost_usd":"0.0318425",…,"total_cost_usd":"0.0322725"}}
run 2  create  "0"        GET  "0.0322125"
run 3  create  "0"        GET  "0.03228"
```

Token counts are correct and identical on both sides; only `cost` differs, the same way every time.
**The obvious reading is that the create path does not compute cost and the read path does. That is
wrong**, and it is worth recording because it survived three consistent runs — reproducibility is
not a mechanism.

## Finding 1 — create prices the model the request named, and the pin's name reads as "free"

The pricing table is `ironclaw@4cb47cfaf3/crates/contracts/ironclaw_common/src/llm_costs.rs`.
`model_cost` strips the provider prefix and matches the remainder. Before any paid entry is
consulted it asks `is_local_model`, which is a **name-prefix heuristic for Ollama-style local
models**:

```rust
lower.starts_with("llama") || … || lower.starts_with("qwen") || lower.starts_with("deepseek") || …
```

`qwen/qwen3.5-397b-a17b` → `qwen3.5-397b-a17b` → `starts_with("qwen")` → **treated as a local model
and priced at zero**. A TEE-attested 397B hosted model is charged as if it were running on the
operator's laptop.

The discriminating test: request a model that is *not* local-prefixed and not a table entry. The
catalogue offers one literally named `default`:

```
create, model "default"  ->  {"input_cost_usd":"0.0322475","output_cost_usd":"0.00002",…}
                             input $2.50/Mtok   output $10.00/Mtok
```

**Non-zero on create.** So the create path prices perfectly well; it is the pin's *name* that
prices to zero. The three-run pattern above was a property of the model string, not of the route.

## Finding 2 — fetch prices a model name that no longer exists

On retrieval `model` is the literal `"reborn"` (§G-2, measured at 1.3.0 and unchanged). `"reborn"`
is not local-prefixed and is not a table entry, so `model_cost` returns `None` and the caller falls
back to the default entry. Upstream says what that default is
(`ironclaw@4cb47cfaf3/crates/app/ironclaw_composition/src/llm_admin/openai_compat_serve.rs:1391-1394`):

> Unknown models fall back to the cost table's default (≈GPT-4o), so a new paid model never
> silently prices at zero.

Measured rates on every fetch: **$2.50/Mtok input, $10.00/Mtok output** — exactly `gpt-4o`'s table
entry, `(dec!(0.0000025), dec!(0.00001))`. Against the $0.50 and $3.30 `MODEL_PIN` records from the
provider's catalogue, that overstates by 5× and about 3×.

So the same turn is priced at zero by one route and at another model's rates by the other, and
**§G-2's post-hoc model amnesia is what makes the second one happen** — two contract facts that
looked independent are one mechanism apart.

This also answers the question `0037` left open as a hypothesis: `$2.50/$10.00` is the **fallback
default**, not a per-model entry for the pin. Answered from the pinned source, not inferred.

## Finding 3 — the cache-read discount is keyed the same way, so the pin gets none

`0001` found cached tokens billing at the same rate as fresh ones, and `MODEL_PIN` carries that.
The upstream type's doc says `cached_input_cost_usd` carries "the provider's cache-read discount",
which looked like a contradiction. It is not:

```rust
pub fn cache_read_discount(model_id: &str) -> Decimal {
    if name.contains("claude") { 10 } else if name.contains("gpt") || o1/o3/o4 { 2 } else { ONE }
}
```

Anthropic 10×, OpenAI 2×, **everything else none**. `qwen/…` matches neither family, so the divisor
is 1 and cached bills at the full input rate. Measured on a turn that cached 3,584 of 12,962 input
tokens:

```
uncached  9378 tokens -> 0.023445 = $2.50/Mtok
cached    3584 tokens -> 0.00896  = $2.50/Mtok      identical
```

So the discount mechanism works and the pin is outside every family it knows. `0001` holds, and the
upstream doc comment is accurate about intent — an earlier draft of this write-up called it
"aspirational", which was unfair and is retracted here.

## What it costs us, and what changed

`ironworks_ironclaw::client` builds its outcome from the create response on the answering path, so
`Usage::total_cost_usd` is `"0"` for every turn the pin answers inside the 30-second wait. The
figure reaches exactly one place — the operator's stderr line in `crates/cli` — and nothing journals
or reconciles from it.

The defect is that the line read `cost 0`, every turn, for turns that cost real money. An operator
has no way to tell that from a free turn. That line now names the figure for what it is:

```
usage: 12737 in (0 cached), 43 out, runtime cost estimate 0 (not the provider's price; see INVARIANTS §G-7)
```

Nothing was dropped — the value is still shown, because for a pin the table *does* know it would be
right — and `crates/ironclaw`'s `Usage` now carries both failure modes in its own doc so the next
reader does not re-derive them.

## Finding 4 — RETRACTED: the thread title does not carry record bytes

`0037` check 6 closed with an aside that `GET /api/webchat/v2/threads` returns a `title` holding
"the raw request input", making an envelope's record bytes "durably readable on a second route".
**Wrong three ways.**

`ironclaw@4cb47cfaf3/crates/domains/ironclaw_threads/src/title.rs` derives the title from the first
non-empty line of the earliest user message, trims it and truncates at `MAX_CHARS = 60`. Its module
doc: *"Derivation is read-time only — nothing is persisted back into the thread record."*

Measured with a marker planted at the front of the input:

```
input : "ACCTSECRET-4417 Blackwell Holdings renewal at risk, owner j.pike, ARR 412000. Reply: ok."
title : '{"format":"openai_compat.responses_input.v1","input":[{"con…'   (60 chars)
leaks ACCTSECRET? False
```

Every thread carried the same 60 characters — the compat layer's JSON wrapper prefix. Not the raw
input, not durable, not record bytes. §E-11 and PRD §19.2 are untouched.

The mistake is worth keeping: a `…`-truncated value printed by that run's *own* helper was read as
truncation performed by the runtime, and a conclusion was drawn from the ellipsis instead of from a
measurement.

**What would make it a finding**, and is not the case here: a caller whose first input line begins
with tenant content. Ours does not, because the compat layer wraps input in a JSON envelope — a
property of the wrapper, not of anything IronWorks controls, so a change to how input is submitted
should re-measure rather than assume.

## What this does not establish

Measured on `production` only; `0039` showed the compat surface behaves identically on the served
profile for the checks it repeated, but cost was not among them.

Whether the seam *should* keep showing a figure it knows is a stand-in is a judgement, not a
measurement. It is kept and labelled on the grounds that a future pin the table knows would price
correctly. If the pin stays outside the table indefinitely, dropping it is the better answer.

No provider bill was consulted. Every "true" price here is `MODEL_PIN`'s record of the provider
catalogue, not an invoice, so "overstates by 5×" is a statement about two tables and not about money
that changed hands.
