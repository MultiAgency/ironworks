# 0044 — A refused turn recovers on the next serving cycle

Date: 2026-09-04. Runtime IronClaw 1.4.0 at `4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`, model
`qwen/qwen3.5-397b-a17b` via NEAR AI. Composition `5e75679666c8bf7e` (relationship-intelligence,
`fixtures/eval.guidance.md`, 26,850 B alloy), fixture book, disposable probe.

No transcript file: this measures **exit codes, cursors and adapter-boundary call counts**, not
answers, and no grader is in the path. The write-up is the evidence.

## Why it was measured

`dev/prove-cutover.sh` blocks whenever the provider crosses the thirty-second cliff `0036`
measured — on the afternoon this was written, four of its thirty-two checks. The obvious repair is
to let a blocked serving check run **one more relay cycle**, because the seam's own answer to a
503 is that the next cycle replays the same logical turn. Before writing that, two things had to
be true and neither had been measured on this path:

1. that the cursor barrier holds an unresolved turn, so a second cycle **sees the update again** —
   without which there is nothing to recover;
2. that the replay resolves to **one logical turn**, not a second billed one.

## 1. The barrier holds — deterministic, no provider involved

Forced with `--crash-at before-dispatch` rather than waited for, so the result does not depend on
provider latency. One update on a disposable channel, two `ironworks relay` processes, one journal.

```
CYCLE 1   relay: poll offset=0 · batch of 1 journalled · CRASHING at before-dispatch · exit 75
journal   1 unresolved · outcome=not_submitted delivery=pending
CYCLE 2   relay: poll offset=950436          <- the update itself, not past it
          rebuild is byte-identical; dispatching the original turn
          delivery=delivered · exit 0
channel   redelivered update=950436 · accepted chunk=0 channel=g-barrier
adapter   1 create call, 1 distinct key
```

The second cycle polled the unresolved update's own id. The channel re-served it, recovery ran,
and the answer reached the room. **The barrier is what makes a second cycle possible at all**, and
it is not incidental to this design — it is the same property `dev/relay-loop.sh` is built on.

## 2. The refusal path: eleven for eleven, and four more confined

The 503 was **induced rather than waited for**, using `0036`'s own finding that the lever is the
scope of the question: the flagship question at 30s against a trivial one at ~19s. Only attempts
where cycle 1 actually returned `service_unavailable` **and** the journal recorded
`outcome=submitted_unknown` are counted; a cycle that completed measures nothing about replay and
is reported separately.

| caller | attempts | 503 induced | recovered | exactly one distinct create key |
|---|---|---|---|---|
| operator bearer | 12 | 11 | **11** | **11** |
| confined member | 9 | 4 | **4** | **4** |

Every recovered cycle printed `rebuild is byte-identical` and `replaying`, and every one recorded
**two `create` calls under one key** — the replay is a second call at the adapter boundary and the
same logical turn, which is exactly what `ironworks_journal::TurnRecord::may_replay` gates.

**Confinement does not change the answer.** That needed asking: every recovery sample before this
one — `0036`'s eleven included — used the operator env bearer, and a closed tool surface is the
caller a tenant actually has. Four samples is enough to say the mechanism is not broken by
confinement and **is not enough to claim a rate**; none is claimed.

## The number that moves: `create=1` is the wrong assertion

A proof that permits a recovery cycle cannot assert one `create` call, because a healthy recovery
makes two. The assertion that survives is **one distinct idempotency key across every `create`
row**, which is strictly stronger: it says one logical turn was billed however many times the
transport replayed it. Measured across all fifteen recovered samples, it held every time.

## The recovery is not always fast

Cycle-2 wall time, operator bearer: 2, 2, 5, 5, 17, 17, 17, 17, 86 s. Confined: 3, 9, 16, 26 s.

`0036` recorded 0.038–16.43 s. **The 86 s sample is outside that range** — the replay returned
`running` and the client polled to completion, the same shape as `0036`'s two 16 s samples but far
longer. A check that adds a recovery cycle inherits this tail, so it is a bound to state rather
than discover: a blocked-then-recovered serving check can take a minute and a half.

## A defect this found in the proof, not in the code

The sittings that motivated all of the above were themselves misreporting. `degraded()` — shared
by `dev/prove-delivery.sh` and, copied from it, `dev/prove-cutover.sh` — carried `429` as a **bare**
alternative, which matches inside a hex digest. **Two of eight sittings reported BLOCKED on runs
that had delivered cleanly**, both on a line of the shape `request 4297bd26b3b174a1`; every relay
cycle prints an alloy digest, a request digest and a `resp_` id, so there are three such tokens per
run and `prove-delivery.sh` passes two runs at once.

Narrowing it to `HTTP 429` loses no coverage, which was checked rather than assumed:
`ironworks_ironclaw::client::interpret_error` never puts the status number in the text — it carries
the error envelope's `code`, so a rate limit arrives as `rate_limit...` and the existing
`rate.?limit` alternative already held it. The only literal emission in the tree is
`ironworks_records::append`. Both scripts were corrected; `dev/prove-delivery.sh` scores 4/4
afterwards with no pattern matching anywhere in its run.

**A false BLOCKED is the expensive kind of wrong.** It is the failure `prove-delivery.sh`'s own
comments were written about — an operator who chases two of them learns to skim the ones that
matter — and here it had corrupted a proof's headline figure before anyone read it.

## What this establishes, and what it does not

**Establishes.** The cursor barrier holds an unresolved turn so the next cycle re-sees the update;
a 503'd serving cycle recovers on the next one, fifteen for fifteen across both callers, always as
one logical turn under one key; confinement does not break that; the recovery tail reaches 86 s;
and a bare `429` in a proof's degradation matcher manufactures false BLOCKEDs at a measurable rate.

**Does not establish.** No rate for the confined caller — four samples. Nothing about answer
quality; no grader ran. Nothing about provider latency on another day: `0036` already records that
two sittings hours apart disagreed, and this one ran while the cliff was being crossed roughly a
third of the time. Nothing about a deployment, because there is not one. And **nothing about
whether the recovery cycle belongs in `dev/prove-cutover.sh`** — that is a design decision this
measurement informs and does not make.

## Superseded — the correction, not the finding

The finding above stands: a bare `429` matched inside hex digests and produced false BLOCKEDs in
two of eight sittings. **The correction it proposed does not.**

Narrowing to the literal `HTTP 429` was checked against
`ironworks_ironclaw::client::interpret_error` carrying the error envelope's `code` — which holds
whenever an envelope parses. Its no-envelope branch puts the response body into the text verbatim,
so a proxy's own `429 Too Many Requests` page carries the number and matches no other alternative
on that line. Requiring the literal prefix dropped exactly that case, and dropping it scores an
environment failure as FAIL: a defect reported where none exists, which this write-up's own closing
section names as the expensive kind of wrong.

Both scripts now exclude a hex run rather than requiring a prefix —
`(^|[^0-9a-f])429([^0-9a-f]|$)` — which still excludes `request 4297bd26b3b174a1` and keeps both
`HTTP 429: ` and a bare proxy page. Checked in both directions over nine cases before landing.
