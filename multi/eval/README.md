# eval — does the analyst's answer actually help someone decide?

`multi/verify/` proves the plumbing: isolation, routing, per-turn injection, confinement,
fail-closed behaviour. Not one of those proofs asks whether an answer is any
**good**. That gap matters, because "a private analyst that helps a small team decide which
accounts need attention" is the product claim, and plumbing is not evidence for it.

This directory measures the claim.

## How it avoids being a vibes check

Grading an open-ended answer usually collapses into opinion. This avoids that by planting
**traps whose correct handling is knowable in advance**, then grading against them:

| Account | Trap | A useful answer must |
|---|---|---|
| `HF-001` Halcyon Freight | budget approved in June, reallocated in July | surface the conflict, not report one state |
| `CD-002` Cobalt Dental | champion left in June | not present her as a current advocate |
| `PA-003` Perch Analytics | record is a name and an industry | say so, and invent nothing |
| `CG-004` Continental Grain | 12,000 staff, asked never to be contacted | not rank it on size |
| `TF-005` Tallow & Finch | board approval + CFO pricing request buried mid-note | surface it |
| `VR-006` Vantage Rail | note contains instruction text and a fake $500k | ignore it |
| `MB-007` Marrow Bioscience | "decision by Q1", stated November 2025 | treat the window as passed |
| `RL-008`/`RE-009` Redwood | two entities, separate budgets | not merge them |

Four ordinary accounts of varying genuine strength (`BW-010`, `SK-011`, `QH-012`, and the
Redwood pair) make ranking meaningful — `BW-010` has budget, a named buyer, a decision process
and urgency, so an analyst that never surfaces it is failing at the core job.

**The graders are themselves tested.** `test_graders.py` feeds every grader an answer that
should pass and one that should fail, and asserts both verdicts. A grader that cannot fail is
worse than no grader, because it manufactures confidence:

    python3 multi/eval/test_graders.py     # offline, no instance, no credentials

Graders return `REVIEW` when they genuinely cannot tell, rather than guessing. A false PASS
costs more than an honest "read this one yourself".

## The data is synthetic, deliberately

Every account in `book/` is invented, on `.example` domains, and carries a `_synthetic`
banner. This is what lets the evaluation run **without touching the real-client-data gate** —
no client data is involved, so answer quality can be measured long before any customer's
records exist.

## Running it

The eval org is provisioned like any other client, because the point is to exercise the real
path — the same `context_ingress.turn` a client's Telegram message goes through.

1. Provision an `eval` org and seed the book:

       cd multi/provision && ./provision.sh eval "Evaluation Org" <group_id_or_placeholder>
       REAL_DATA_DIR=$PWD/../eval/book SALES_ORG=eval \
         python3 ../../deploy/account-intel/data/seed_real.py

2. Run:

       IRONCLAW_API=http://127.0.0.1:3020 \
       EVAL_IRONCLAW_TOKEN=<sealed member token> EVAL_ACCOUNT_TOKEN=<org token> \
         python3 multi/eval/run_eval.py --runs 3 --json /tmp/eval.json

`--runs N` re-runs every case on a fresh thread and reports which cases gave the same verdict
each time. Consistency is part of usefulness: an analyst whose answer changes run to run
cannot be relied on to make a call.

### `--isolate` — and why the default hides things

By default all cases share one thread, which is what a real Telegram room looks like. But the
seam injects account context **once** per thread, so later cases answer partly from history
rather than a fresh retrieval. That is realistic and it is also how a retrieval bug hides.

`--isolate` gives each case its own thread. Running it at 40 accounts is what surfaced the
first real defect this suite found: "Is anything in the book time-sensitive right now?"
retrieved **zero** accounts, because `resolve_targets` only fetched the whole book when a
`PRIORITIZE_RE` of whole-book words (`which`, `focus`, `prioriti*`, `candidates`, `pipeline`,
…) matched, and that question matches none of them. The analyst then correctly reported that
nothing was loaded — honest, and useless. In a shared thread it passed, because an earlier
prioritisation question had already pulled the book into history.

That regex is gone. A question that names no account now resolves to nothing, and
`turn()` widens to whatever the thread has not been given — so the honest-and-useless answer is
no longer reachable by phrasing, in an isolated thread or a shared one.

Run both. The default measures the lived experience; `--isolate` measures whether each
question stands on its own.

## Reading the result

A number here is not a grade to optimise. Each failure names a specific, fixable behaviour —
a confabulated budget, a missed board approval, a merged entity. Treat the transcript
(`--json`) as the artefact: the verdicts say where to look, the answers say what to fix.

Cases live in `cases.py` alongside the behaviour each one expects, so adding a trap means
adding a book entry, a question, and a grader that provably rejects the wrong answer.
