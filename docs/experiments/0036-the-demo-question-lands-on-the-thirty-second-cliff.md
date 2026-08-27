# 0036 — The demo question lands on the thirty-second cliff

Date: 2026-09-02. Composition `6678953f4855d559` (relationship-intelligence@2, live operator
guidance, 27,941 B) and `3770e0942624bda4` (same service, fixture guidance, 25,575 B). Runtime
IronClaw 1.4.0 at `4cb47cfaf3`, model `qwen/qwen3.5-397b-a17b` via NEAR AI.

No transcript file: this measures **latency and exit codes**, not answers, and there is no grader
in the path. The write-up is the evidence.

## What was measured

One question — *"What needs my attention across our accounts right now?"*, the question the
operator journey in `docs/ACCOUNT-COORDINATION.md` is written around — asked seventeen times
across every path that can ask it, in two sittings, with a cold probe between the second sitting's
runs.

`DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30s and a turn outlasting it is refused HTTP 503 while the
turn keeps running server-side (`DESIGN.md`, the upstream contract). That contract was already
measured. What was not measured is **where the shipped service's own flagship question actually
falls against it**, and the answer is: on the boundary, with the outcome decided by roughly a
second of provider variance.

## The samples

| path | records | n | 503 | wall time |
|---|---|---|---|---|
| `coordinate`, live Account Service | 2 | 6 | 4 | 28.87 / 29.98 / 30.13 / 30.14 / 30.22 / 30.65 |
| `coordinate`, fixture book | 10 | 4 | 3 | 29.82 / 30.06 / 30.10 / 30.11 |
| `relay` one-shot, fixture book | 10 | 3 | 3 | 30.10 / 30.12 / 30.13 |
| `relay-loop.sh` to delivered, fixture book | 10 | 4 | 1 | 26.47 / 29.36 / 45.50 / 45.08 |

**Eleven of seventeen returned 503.** The six that did not were 26.47, 28.87, 29.36, 29.82, 29.98
and the first cycle of two 45s loop runs. Every completing sample sits between 26.5s and 30.0s;
every refused one sits between 30.06s and 30.65s. There is no bimodality and no slow tail — there
is one distribution with a cliff drawn through it.

**The record count barely matters.** Two live records (7,726 B envelope) and ten fixture records
(6,100 B) produce the same latency, because the turn is dominated by generating ~1,200–1,900
output tokens, not by reading the book.

## The control: a trivial question clears it every time

Ten short questions — *"Name one account. One line."* — through one `relay` cycle, one
conversation, took **189.55s for ten turns (~19.0s each) with zero 503s**. Same service, same
alloy, same runtime, same sitting. So the cliff is reachable from either side by changing what is
asked rather than anything in this repository.

## Narrowing the question works. Capping the format does not.

The obvious next step is to keep the flagship question and constrain its answer. **Measured, that
does not work**, and the failure is the interesting part. Four phrasings against the live book,
three samples each, then the best one taken to ten:

| | phrasing | clean |
|---|---|---|
| A | *What needs my attention across our accounts right now?* (current) | **3 / 9** |
| B | A + *"Answer in at most three bullets, one sentence each."* | **1 / 3** |
| C | *Name the single most urgent thing across our accounts, in two sentences, with the record date it rests on.* | **9 / 10** |
| D | *Name the single most urgent thing across our accounts. One sentence, and cite the record date it rests on.* | **1 / 3** |

B and D both **instruct the model to write less** and both land where A does. D asked for a single
sentence and still refused twice — and its one clean run returned 696 output tokens, fewer than
several refused turns elsewhere in this file. So output length does not predict wall time turn by
turn, and an instruction to be brief is not a latency control.

C differs from B and D by narrowing **what is asked** — one item instead of a survey of the book
— rather than capping how the answer is formatted, and it is the only phrasing that moved the rate:
9 of 10 clean at 23.45-29.43s against A's 3 of 9. Ten samples against nine is enough to separate
those two; it is **not** enough to rank B against D, and no ranking of them is claimed.

**The lever is the scope of the question, not the length of the answer.** That is a weaker and
more specific claim than "answer length is the lever", which is what this file said before the
four-phrasing trial was run, on the strength of the trivial-question control alone.

## Recovery works, and is the reason none of this lost an answer

Every 503 was recovered by replaying the idempotency key with a byte-identical body (ADR 0005).
Measured recoveries: **0.038s, 0.059s, 0.061s, 0.071s, 0.082s** where the replay returned
`completed`, and **16.35s, 16.43s** where it returned `running` and the client polled to
completion. `relay-loop.sh` performs that replay on its next cycle with no operator action, which
is what makes the loop's end-to-end figure 45s rather than a failure.

**No answer was lost in seventeen turns.** The finding is not that the system drops work — it
demonstrably does not — it is that the *default* experience of the shipped question is thirty
seconds of silence followed by a refusal that reads like a fault.

## A defect this found in the operator journey, not in the code

The obvious recovery instruction is "run the same command again". That is correct **only when the
first command failed**, and following it unconditionally is worse than doing nothing.

`coordinate` derives its idempotency key per invocation. When the first turn *succeeds*, the
journal holds no unresolved turn, and a second invocation is not a replay — it mints a new key
(observed: `ironworks-472d-18d1895134628e20` then `ironworks-54c5-18d18957effda880`), bills a
second turn, and in two of two observed cases that second turn hit the cliff itself and left the
journal holding one unresolved turn where it had held none.

`ironworks status --journal <path>` is what distinguishes the two cases, and it reported correctly
every time: `0 unresolved turn(s)` after a success, `1 unresolved turn(s)` after a 503. The
operator journey now says to read it first rather than to re-run blind.

## What this establishes, and what it does not

**Establishes.** The shipped service's flagship question sits on the 30s boundary; the outcome is
provider variance, not configuration; **narrowing the question moves the refusal rate and capping
the answer's format does not**; recovery is sound and, under `relay-loop.sh`, automatic; and an
unconditional re-run instruction manufactures the failure it was meant to repair.

**Does not establish.** Nothing about answer quality — no grader ran. Nothing about the provider's
latency on any other day: seventeen samples in two sittings on one laptop against one instance,
and the two sittings disagreed (Pass 1 refused three of three live turns, Pass 2 completed two of
three). Nothing about a deployment, because there is not one. And nothing about the runtime that
is not already in `DESIGN.md` — the 30s contract was known; only its collision with this question
is new.

**Do not read this as "the live path is broken".** It answered every time it was asked, including
the eleven times it first said `service_unavailable`. What it did not do is answer *quickly, on
the first invocation, without an error on screen* — which is a different and much narrower claim,
and the only one this file supports.
