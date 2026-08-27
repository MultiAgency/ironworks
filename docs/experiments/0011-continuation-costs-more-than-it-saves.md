# Continuation defeats prompt caching, and that outweighs the supplied set

Date: 2026-08-31. Composition `67f80eb2d20172fa` (account-analysis, 40 synthetic records),
disposable stack, `ironworks relay` against `devchannel`. Five turns.

## What was measured

Two questions, asked as a chained pair and separately, with `usage` read off the turn telemetry.

| | envelope | input | cached | **uncached** |
|---|---|---|---|---|
| turn 1, fresh | 14,512 B | 19,584 | 9,856 | 9,728 |
| turn 2, **chained** | 281 B | 23,445 | 0 | **23,445** |
| turn 2, chained (repeat) | 281 B | 23,525 | 0 | **23,525** |
| the same question asked **fresh** | 14,514 B | 19,587 | 9,856 | **9,731** |

`cached: 0` on a chained turn reproduced across both runs.

## The finding

**Chaining costs about 2.4x the uncached input of not chaining**, on this stack at this book size.

The supplied set does what it was built to do — the chained turn's envelope fell 14,512 → 281
bytes, a 98% cut, and the model was told the records were carried rather than handed an empty
book. That saving is real and worth roughly 3,600 tokens. It is dwarfed by what chaining gives up:
a fresh turn's input is almost entirely a constant alloy plus a near-identical envelope, which the
provider caches half of; a chained turn's input contains the previous answer, and cached nothing at
all in either run.

## What this contradicts

`DESIGN.md` justified the thread bound on the cost curve in `0001-alloy-drift.md` — input growing
~3,100 tokens per turn because IronClaw re-persists `instructions`. That measurement stands. What
it did not account for is caching: the re-persisted instructions are exactly the bytes a provider
caches best, so the growth is cheaper than its token count suggests, while a chained turn's growth
is not cacheable at all.

So the reasoning "chaining is expensive, therefore bound it at 8 turns" is the right conclusion
from an incomplete measurement. On this evidence a bound of 8 is far too generous: by the second
turn chaining is already losing.

## What it does not establish

One stack, one composition, one book size, five turns. Caching behaviour belongs to the provider
and could change; a much larger book would shift the balance, since the resent envelope grows while
the replayed history does not. And the whole comparison is about **cost**, not about answers: a
chained turn can answer a follow-up that a fresh turn cannot, and nothing here measures that.

The question this raises is not "is continuation worth it" but "worth it *where*" — which is a
per-tenant decision about whether follow-ups matter more than 2.4x the uncached tokens, rather
than a global default of 8.
