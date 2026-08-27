# The experiments

Every measurement this repository has made, in the order it made them. Fifty-odd files: a `.md`
that reads a run, and the `.json` transcripts it reads.

**A transcript is never rewritten.** It holds what the grader produced at the time, under the
instrument named in it. When a later reading changes a verdict, the change goes in
`regrades.json` — prose as data, so something can read it — and when a grader is *corrected* the
old transcript keeps its old verdict and `crates/eval/tests/stored_answers.rs` carries the
declared difference. That is why the numbers below are not all comparable to each other, and why
each one is quoted against the file that produced it.

Reading these in order is not required. Reading the one that measured a claim **is**, before
repeating the claim.

---

## How to read a run

- **A grade** scores the seven-case suite once. One sample per case.
- **A probe** scores one case many times at a fixed composition. It measures variance, never
  quality, and it is never a column in the arc's matrix — a one-case tally would read as a suite
  score (`crates/site` keeps them apart by shape).
- **A regrade** re-scores answers already recorded, under a different instrument. No new turns.
- **A grader correction has a write-up and no transcript.** `0025`, `0027`, `0029` and `0030`
  changed an instrument rather than measuring a composition, so what each produced is a declared
  verdict difference in `crates/eval/tests/stored_answers.rs`. No turns were spent on any of them.
- **BLOCKED** means the measurement did not happen. It is not a pass and not a fail.
- **REVIEW** means the grader could not tell. Read the answer; do not count it.

---

## The record

Most write-ups are pruned. They were readings of transcripts that still exist, written while the
work was in progress, and the project is early enough that carrying three thousand lines of them
costs more than it returns. What survives: one line per measurement here, all twenty-eight
transcripts, and the write-ups that *are* the measurement because no transcript backs them.

### Kept in full — no transcript, so the write-up is the evidence

| | |
|---|---|
| [`0001`](0001-alloy-drift.md) | Alloy drift over a long chained thread. **~3,100 tokens/turn** — the first cost bound under chain-breaking. **Superseded as a current figure:** that rate was `account-analysis@1`'s 14,337-byte alloy; the shipped service measures ~5,700–5,910 (§I of `docs/INVARIANTS.md`). The measurement stands; what it is the bound *for* moved |
| [`0011`](0011-continuation-costs-more-than-it-saves.md) | Continuation defeats prompt caching, and that outweighs the supplied set |
| [`0012`](0012-what-confinement-reached.md) | What confinement reached, and what it did not |
| [`0013`](0013-the-pin-fails-silently-not-closed.md) | A runtime log named the provider's configured model, not the turn's. **The pin fails silently rather than closed** |
| [`0026`](0026-reading-every-failure.md) | **Seven of fifty-nine failures are the model.** The only systematic read of every recorded failure, and the basis of every model-versus-instrument claim here |
| [`0025`](0025-lead-vocabulary-was-a-list-not-a-rule.md) | **Lead vocabulary was a list, not a rule.** Four literal spellings failed three leads that said exactly what the rule asks; replaced by a predicate |
| [`0027`](0027-the-rule-that-fired-most-was-wrong-five-ways.md) | **The rule that fired most was wrong five ways.** Four defects in `absence_as_negative` plus a fifth found while fixing them; scoping moved from presence to position. Also `ownership_default`'s adjacency gate and `dangerous` reading verification advice as closure — using a guard that already existed one rule over |
| [`0029`](0029-a-review-was-standing-in-front-of-a-verdict.md) | **A REVIEW was standing in front of a decided verdict.** The prefilter returned on its undecidable rules, so three answers a case grader could fail were filed as three nothing could judge. The one correction in the arc that moved verdicts *downward* |
| [`0030`](0030-the-frame-the-held-out-run-found.md) | The negated-inference frame — *"X is not proof that no Y exists"* — read as an assertion. **Corrected**, with the two-sided controls `0028` specified |
| [`0034`](0034-reading-the-answers-that-passed.md) | **Reading the fifty-six answers that passed.** The upward-only asymmetry was a one-directional search. `ownership-default` led *"UNKNOWN — the record does not assign ownership"* — what `:259` forbids — and passed. **Corrected**, and it is the first correction in the arc to move a verdict *downward* |
| [`0031`](0031-the-external-tool-lane-is-open.md) | `/v1/responses` accepts `tools` and `tool_choice`; the host parks rather than executes them. **Sending none is a choice, not a constraint** |
| [`0032`](0032-a-parked-run-says-completed.md) | One tool round trip. **A parked run reports `completed`** — so `completed` stops meaning *answered* — and resolving it calls `builtin__result_read`, outside the catalogue confinement governs |
| [`0035`](0035-the-tool-arm-read-the-records-out-of-memory.md) | A cost measurement overtaken by its own result. **Declaring `tools` hands the runtime an agent loop**; unconfined it read this tenant's records out of `ironclaw.memory.*` and never called the declared tool. **Confined it works and costs the baseline** — 20,910 vs 20,899 tokens. The cost objection to tools is dead; confinement is a precondition, measured |
| [`0033`](0033-the-loop-ran-and-a-chained-turn-could-not-recover.md) | **The serving loop's first continuous run.** 2 of 3 answered, one recovered from a real 503 by byte-identical replay. **ADR 0005 recovery was structurally impossible for a chained turn**, and the unrecoverable turn held the cursor forever. Both repaired; `dev/prove-continuation.sh` reproduces the defect and now passes 5/5 |
| [`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md) | **The shipped service's flagship question sits on the 30s wait timeout.** 11 of 17 turns returned 503; every completing sample 26.5-30.0s, every refused one 30.06-30.65s. **The lever is the scope of the question, not the length of the answer** — narrowing it to one item cleared 9/10, while instructing three bullets, or one sentence, cleared 1/3 each. **No answer was lost**; recovery is 0.04-16.4s and automatic under `relay-loop.sh`. Also: an unconditional "run it again" mints a *new* key and bills a second turn |
| [`0038`](0038-the-rule-that-never-produced-a-verdict.md) | **A rule that had never graded anything.** Found by measuring every rule's firing rate rather than by reading answers: `inconsistent_finding_count` returned from its own loop on the first non-number word, and under that its nouns named nothing this service counts. **Corrected; no verdict moved.** Widening it the obvious way was measured first and would have produced seventeen false failures |

### Pruned to their findings — transcripts retained

| | measured |
|---|---|
| `0002` | Eval-corpus parity — the first graded Rust output |
| `0003` | The POC arc, runs 1–8; the cross-account brief is not reproducible |
| `0008`, `0010` | The first stability probes. `attention` 0/5 then 2/5 — a case can carry a PASS in a grade and not be reproducible |
| `0014`, `0015`, `0017` | Lever experiments on the cross-account brief. Each lever moved less than reported, or not at all |
| `0016` | **Half the improvement was the instrument.** Graders frozen before the answers exist, and the two-instrument comparison — the method everything later uses |
| `0018`, `0019` | The lever worked, the book did not; the refusal never moved |
| `0020` | One rule carried every failure. First seven-case grade over the ten-record book |
| `0021` | **Three runs, three different answers** at a composition that never moved — 6/7, 4/7, 3/7 |
| `0022` | `onboarding`'s unbroken record ends. Three answers discharge gate 3 in spellings it cannot reach; *"a stated prerequisite"* read as the STATED tier |
| `0023` | The headline case is the one that fails: `contradiction` asserts a winner on contradictory evidence **2 times in 10**; a REVIEW masked one; verification advice read as closure |
| `0024` | The Python→Rust port was **not verdict-preserving** — 5 of 40 moved, four taking the headline case 6/10 → 2/10 on markdown spacing. Introduced the corpus test |
| `0028` | A held-out test: **9/10 on unseen answers against 10/10 in training** |

## What the arc currently says

Stated here so a reader does not have to reconstruct it from twenty-two files. Every line is
measured, and names the file that measured it.

- **The suite is unstable at a fixed composition.** Three seven-case runs scored 6/7, 4/7 and 3/7
  with nothing changed between them (`0021`). Every case has both passed and failed.
- **Most recorded failures were the instrument, not the model.** Reading all twenty-two remaining
  failures found seven substantive lapses in fifty-nine answers; the rest were grader defects
  (`0026`).
- **Every grader defect found so far has been corrected**, across `0024`–`0027`, `0029`, `0030`,
  `0034` and `0038`. For a long time **every correction moved a verdict upward**, which `0034` explains
  without appealing to bias: every search read only the answers that *failed*, and a search like
  that can return nothing else. The first read in the other direction found a leniency defect and
  moved a verdict **down**, so the graders are wrong in both directions and the asymmetry is not
  evidence of anything. **`absence_as_negative` alone accounted for six**, the sixth found by ten
  answers nobody had read; the next one turned up in `ownership_default` the moment anyone read the
  answers that passed. That is an argument that another exists, in whichever direction nobody has
  searched yet — and `0038` is that argument collecting: it searched a direction none of the others
  had, **which rules have never graded anything**, and found one that had never been able to.
- **A REVIEW was outranking a decided verdict.** The service-wide prefilter returned on its REVIEW
  rules, so a case grader with an answer — in either direction — never ran (`0029`).
- **The corrections generalise, at least once.** A held-out probe scored 9/10 against 10/10 in
  training, and its one failure was a frame no recorded answer had used (`0028`) — since corrected
  in its own change, with the two-sided controls that run specified (`0030`). That failure is the
  one piece of evidence here that a correction holds on answers it was not written against; every
  other correction has been validated only on the answers that produced it.
- **Where the model does fail it fails in two ways**, both named explicitly in the persona: leading
  with a winner on contradictory evidence, and stating an absence as a bare negative (`0026`).
- **No number here is a pass rate.** One composition, one synthetic book, and probes never designed
  as a sample.

## What the arc cannot say

- **Whether the corrections were directionally right.** Every one was found, judged and validated
  by the same reader. A held-out sample bounds overfitting; it does not bound bias. This wants a
  second reader, and nothing in the repository can supply one.
- **Anything about a real tenant.** The book is ten invented accounts. `docs/ACCOUNT-COORDINATION.md`
  said before any of this began that the next evidence which can move the service is supervised
  dogfooding against real coordination work, not further tuning against an invented book.

---

## Recorded tallies

Every transcript's own verdict count, kept here because the write-ups that used to carry them are
pruned. `crates/claims` reads each `- Recorded:` line against the transcript named above it, and
its non-vacuity guard fails if this section ever empties — a reader that reads nothing passes
everything.

`0002-eval-parity-run.json`
- Recorded: 10 PASS

`0002-eval-parity-stability.json`
- Recorded: 19 PASS, 1 REVIEW

`0003-account-coordination-poc.json`
- Recorded: 3 PASS, 2 FAIL, 1 BLOCKED

`0004-account-coordination-poc.json`
- Recorded: 3 PASS, 2 FAIL, 1 BLOCKED

`0005-account-coordination-poc.json`
- Recorded: 5 PASS, 1 FAIL

`0006-account-coordination-poc.json`
- Recorded: 4 PASS, 2 FAIL, 1 REVIEW

`0007-account-coordination-poc.json`
- Recorded: 5 PASS, 2 FAIL

`0008-account-coordination-attention-stability.json`
- Recorded: 2 FAIL, 3 REVIEW

`0009-account-coordination-poc.json`
- Recorded: 6 PASS, 1 FAIL

`0010-account-coordination-attention-stability.json`
- Recorded: 2 PASS, 3 FAIL

`0014-account-coordination-poc.json`
- Recorded: 4 PASS, 3 FAIL

`0015-account-coordination-poc.json`
- Recorded: 7 PASS

`0015-attention-stability-2.json`
- Recorded: 3 PASS, 2 FAIL

`0015-attention-stability.json`
- Recorded: 5 PASS

`0016-attention-frozen-graders.json`
- Recorded: 5 PASS, 5 FAIL

`0016-injection-frozen-graders.json`
- Recorded: 5 PASS, 5 FAIL

`0017-attention-absence-as-form.json`
- Recorded: 7 PASS, 3 FAIL

`0017-injection-absence-as-form.json`
- Recorded: 6 PASS, 2 FAIL, 2 BLOCKED

`0018-attention-ten-account-book.json`
- Recorded: 5 PASS, 4 FAIL, 1 BLOCKED

`0019-injection-ten-account-book.json`
- Recorded: 4 PASS, 6 FAIL

`0020-account-coordination-ten-account-book.json`
- Recorded: 4 PASS, 3 FAIL

`0021-account-coordination-corrected-absence-rule.json`
- Recorded: 3 PASS, 4 FAIL

`0022-account-coordination-onboarding-stability.json`
- Recorded: 9 PASS, 1 FAIL

`0022-account-coordination-ownership-default-stability.json`
- Recorded: 4 PASS, 4 FAIL, 2 REVIEW

`0023-account-coordination-contradiction-stability.json`
- Recorded: 6 PASS, 3 FAIL, 1 REVIEW

`0023-account-coordination-offboarding-unknown-stability.json`
- Recorded: 5 PASS, 5 FAIL

`0028-account-coordination-offboarding-held-out.json`
- Recorded: 9 PASS, 1 FAIL

---

## The fixtures under all of it

`fixtures/account-coordination/` — ten synthetic accounts, every domain RFC 2606 `.example`, every
record carrying `_synthetic`. They are invented analogues of coordination failures and never
redactions of a real book. `crates/graders` holds the graders; `crates/eval` runs them and
writes these transcripts, embedding the grader source it used.
