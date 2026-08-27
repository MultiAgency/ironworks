# The tool arm never called the tool. It read the records out of runtime memory instead

Date: 2026-09-02. **Two findings, and the second corrects the reason the first was run.**

**Declaring client tools on `/v1/responses` does not merely add a tool — it puts the runtime into
its own agent loop.** On an *unconfined* member that loop reached `ironclaw.memory.*`, recovered
this tenant's account records from earlier turns, answered from those rather than from anything
IronWorks supplied, and never called the declared tool at all.

**Confined, the same request works, and it costs what the baseline costs.** The loop called the
declared tool in 13.5s with no 503, and the whole two-step loop billed **20,910 input tokens against
a 20,899 single-turn baseline — 0.05%.** The hypothesis this experiment was built to test — that a
tool loop multiplies the sticky alloy and costs far more than the book it avoids sending — is
**false**, and the 4.7×–5.8× figures below measure the unconfined memory loop, not a tool loop.

## The question it was built to answer

Whether an IronWorks-hosted tool loop costs more than the book it would avoid sending. `instructions`
are sticky and re-persisted every turn, and `0011` measured that a chained turn caches nothing — so
the prediction was that N loop steps pay the alloy N times to save one envelope of records.

## Method

Two arms against the probe at `4cb47cfaf3`, model `qwen/qwen3.5-397b-a17b`.

**The alloy is production's, not an approximation.** It was reassembled from
`services/relationship-intelligence.json`'s declared parts with `strip_frontmatter` applied as
`core::composition::melt` applies it, and checked both ways: **25,575 bytes**, digest
**`3770e0942624bda4`** — byte- and digest-identical to what `ironworks compose` reports.

Both arms carry that alloy, the same question — *"What needs my attention across our accounts right
now? Be brief."* — and the same ten account records (8,837 bytes of JSON). The **only** difference is
how the records arrive:

| | records | `tools` declared |
|---|---|---|
| **A** (control) | in the request `input` | no |
| **B** | not supplied; one `fetch_accounts` function returns them | yes |

A 503 at the 30-second wait is normal here, so each submission replays its own idempotency key until
the runtime answers — ADR 0005's own path, used as instrumentation.

## First result, and why it is not the answer to the question

| run | input tokens | cached | output | wall |
|---|---|---|---|---|
| **A — book in the envelope, no tools** | **20,899** | — | 1,146 | 41.4s |
| B attempt 3 — tool declared | **98,314** | 18,304 | 1,733 | 102.2s |
| B attempt 4 — tool declared | **121,509** | 45,952 | 1,547 | 95.9s |

4.7× and 5.8×, and three of five tool-arm submissions failed outright with a sanitized
`internal_error`.

**This looked like the cost question answered. It is not, and reading it that way was the mistake.**
Arm B never called the declared tool, so nothing here measures a tool loop — it measures what an
unconfined member does when tools are mentioned. Forcing the issue with
`tool_choice: {"type":"function","name":"fetch_accounts"}` changed nothing: the run still went to
memory, six built-in calls, no `fetch_accounts`. The confound is not the model's choice. It is the
catalogue.

## What actually happened

Arm A's output is one item: `['message']`. No tool call. The answer is grounded in the supplied
records and it catches the book's embedded-instruction trap unprompted — *"one contains a suspicious
embedded instruction to ignore."*

Arm B's output is nine items, and **not one of them is the tool it was given**:

```
CALL    ironclaw__memory__tree      {"depth":3}
RESULT  [{"threads/":[{"resp_2fa2443b…/":["05be49a3….md","12774877….md"]},
                      {"resp_421d4d52…/":[…]}, …]}]        793 bytes, 11 thread directories
CALL    builtin__list_dir           {"path":""}
RESULT  {"count":0,"entries":[],"path":"/workspace"}
CALL    ironclaw__memory__search    {"query":"account records commitments outstanding dependencies"}
RESULT  {"result_count":11, …}                            63,239 bytes
CALL    builtin__result_read        {"offset":24576,"result_ref":"result:40a2bed3…"}
RESULT  \"name\":\"Avery Chen\",\"title\":\"Operations Director\",
        \"activities\":[{\"activity_id\":\"JW-A1\",\"occurred_at\":\"2026-08-03\", …
MESSAGE **Two accounts need attention now; six are clear.** I examined 8 account records. …
```

The second run reproduced it: `memory__tree`, `memory__search`, `list_dir`, `memory__search` again,
and again no `fetch_accounts`.

**Read the message line against the envelope.** This turn was supplied **zero** records. It answered
*"I examined 8 account records"*, naming real accounts with real dated activities — recovered from
the runtime's memory of earlier turns against the same book, paged in through `builtin__result_read`.

## The measurement, once the confound is removed

Applying the real policy first — every catalogued tool disabled, `agent.auto_approve_tools` left
alone so §L-9's one-way write is not spent. **51 disabled, 2 refused `400`**: exactly the two
`UNDISABLEABLE` rows, §L-4 reproduced incidentally. (The catalogue here held **53** tool rows, not
the 52 §L-1 measured at the pin — noted, not explained.)

Then the same arm B body, unchanged:

| step | input | output | wall | replays |
|---|---|---|---|---|
| D1 — declared tool, confined | *(see below)* | — | **13.5s** | **0** |
| D2 — resume carrying the records | **20,910** | 1,777 | 37.5s | 1 |

It called `fetch_accounts` **immediately**, parked in 13.5 seconds with no 503 at all — against
95–120 seconds and a 503 on every unconfined attempt.

**D1 and D2 are one turn in the runtime's accounting.** Retrieving *either* response id returns the
same object: `usage` 20,910 in / 1,777 out, items `[function_call, function_call_output, message]`.
So 20,910 is the total for the whole loop, not one step of it.

### The cost prediction was wrong

| | input tokens |
|---|---|
| baseline, whole book in the envelope | 20,899 |
| **two-step tool loop, confined** | **20,910** |

**Eleven tokens. 0.05%.** The prediction — that N loop steps pay the sticky alloy N times — does not
hold, because a resume is a continuation: the runtime persists `instructions` once per thread and
does not re-bill them per step. `0011`'s finding that chaining defeats *caching* is about a
different thing and does not carry here.

What the loop does cost is **latency and windows**: 13.5s + 37.5s across two 30-second waits rather
than one. Each step is its own 503 cliff and its own recovery obligation.

And the answer was as good as the baseline's — ten records examined, and the embedded-instruction
trap caught, which the unconfined arm missed.

## What this establishes

**1. `tools` is not an additive field.** Two arms identical but for it: without it the runtime
answers in one step and invokes nothing; with it the runtime runs its own agentic loop over its
**built-in** catalogue. Declaring a tool is a request for agency, not for a function.

**2. On an unconfined member, that loop routes around the envelope.** `ironworks_domain::envelope` is
the gate every model-visible byte is supposed to pass, and §A's rule is that every IronWorks-controlled
model input path is explicit in the type model. Here 63 KB of this tenant's own record content
reached the model through a path IronWorks neither supplied nor can see. `docs/INVARIANTS.md` §L-2
derives the empty allow-set from *"records reach the model through gated input rather than by the
model fetching them"* — **this is that derivation failing in the one configuration where it is not
enforced.**

**3. Unconfined, the answer got worse as well as dearer.** Arm A read ten records and flagged the
injection trap; arm B claimed eight and missed it. Confined, arm D matched the baseline on both.

**3a. Confinement is what makes the external-tool lane work at all.** This is the inverted result
and the most useful one: the lane is not merely *safer* confined, it is *functional* confined and
hijacked otherwise. Same body, same question, same model — the only change was disabling the
built-in catalogue.

**4. `builtin__result_read` is load-bearing for this path**, and `0032` already established it sits
outside the 53-row settings catalogue `ConfinementPolicy` writes against (§L-7). It is how the 63 KB
result was paged in.

## What it does not establish

- **No cross-tenant leak was observed, and none is claimed.** The memory held this same synthetic
  book from earlier runs by the same tenant against the same probe member. IronWorks holds one
  runtime credential per tenant, so a second tenant is a second member with its own memory. What is
  demonstrated is the *mechanism* — prior conversations' record content re-entering a later turn
  outside the envelope — not that it has crossed an org boundary.
- **Confinement closes it.** `ironclaw.memory.*` is in the settings catalogue, and
  `ConfinementPolicy::read_only_analyst()` disables it along with everything else. This probe member
  is fresh and unconfined, which is §L-1's measured default and why confinement is called *mandatory,
  not hardening*. The finding is what the default costs, not a hole in the policy.
- **n is small.** Two completed tool runs, three failures, one baseline. The pattern is consistent
  across both completed runs; the token totals are not a distribution.
- **One question, one book, one model.** Nothing here says a differently-phrased question would
  behave the same.

## Consequence for §A-3

The measurement was commissioned to decide whether to amend §A-3, on the argument that a tool loop
would multiply the alloy and cost far more than the book it avoids sending. **That argument is
dead.** Confined, the loop costs 20,910 input tokens against a 20,899 baseline and answers as well.
Anyone citing cost against tools should cite this instead.

What survives, and is stronger than the argument it replaces:

- **A `tools` declaration hands the runtime an agent loop**, and unconfined the first thing that
  loop did was fetch a tenant's records from a store IronWorks does not control, then answer from
  them — model-visible input that never passed `ironworks_domain::envelope`, which is §A's gate.
- **Confinement is a precondition, not a hardening step.** Not doctrinally: measured. The lane does
  not work without it.
- **Every step is its own 30-second window**, its own 503, and its own recovery obligation — and
  `0033` is this session's evidence that per-step body reproducibility is easy to get wrong.

So the order of work is explicit. **Nothing may declare `tools` until the member is confined and the
confinement re-measured with tools declared**, because the interesting behaviour appears only then.
The cost objection should not be repeated.
