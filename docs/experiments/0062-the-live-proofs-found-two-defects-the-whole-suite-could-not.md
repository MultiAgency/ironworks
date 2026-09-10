# 0062 — The live proofs found two defects the whole suite could not

**Date:** 2026-09-09
**Pin:** `ironclaw@4cb47cfaf3fbacd3a2f575a8da654a6542d5fdfe`, model `qwen/qwen3.5-397b-a17b`
**Stack:** `dev/probe-up.sh`, production profile, fresh volumes

## Why this run happened

`crates/cli` became a library in the quality pass recorded in
[`0060`](0060-a-second-quality-pass-and-the-three-recommendations-it-withdrew.md), and nothing
live had been measured since. The unit and integration suites were green throughout, which is
exactly the state in which the serving path is least worth trusting: every check they run is one
somebody thought to write, and both defects below are in the space between the binary and the
scripts that measure it.

Four costed proofs, in order, with a rate window between each: `dev/prove.sh`,
`dev/prove-delivery.sh`, `dev/prove-confinement.sh`, `dev/prove-cutover.sh`. Two members were
spent finding this, and a third proving the fix.

## The two defects

### 1. An accepted `--call-log` did not exist until after the credential had been checked

`dev/prove.sh`'s wrong-credential check asserts two things: that the turn is refused, **and** that
no create call was made. It reported FAIL. The refusal was correct and printed the right sentence;
what failed was the second half — the call log did not exist, so no count could be taken from it.

`crates/cli/src/lib.rs::CallAudit::open` was reached inside `drive`, after the record scope had
been resolved. A credential refused during boot therefore returned before the file was ever
created, and the earliest and most security-relevant refusals were exactly the ones that left no
evidence.

This had been invisible because `dev/lib.sh`'s `creates` used to be
`awk … 2>/dev/null | wc -l`, which answers `0` for a file that does not exist. A missing log and a
log with no rows were the same answer, and the vacuous one was the one being asserted on. The
current `call_count` refuses to supply a count it cannot take — correctly, and the refusal is what
surfaced this.

Fixed at the other end, which is where the defect is:
`crates/cli/src/lib.rs::ExecutionControls::arm_call_log` creates the file when the flag is
accepted, before anything can refuse. Empty is the honest artifact for a run that made no calls.
`dev/prove.sh` then passes 9/9.

### 2. `dev/prove-cutover.sh` could not pass against a probe, and charged a member to say so

`registry publish-one` repeats the requested-model policy comparison and refuses unless IronClaw's
operator-visible policy is the singleton `MODEL_PIN`
(`crates/cli/src/lib.rs::require_model_policy_exact`). A fresh probe comes up
`selection_enabled=false, default_matches=false, exact_allowlist=false, allowed_count=0`.

The script never applied that policy and never named it a precondition. So it minted a member,
adopted it, confined it — spending it irreversibly — reached publication, was refused, and scored
the refusal **FAIL**: "the cutover is broken", when what happened is that the probe had never been
configured for one. A precondition that cannot be met is BLOCKED, and it must cost nothing
(§F-14).

Two changes, both in `dev/prove-cutover.sh`:

- `ironworks model-policy apply` runs in the preconditions block, **before the mint**, and
  `give_up`s if it cannot. A probe that cannot take the policy now costs no member.
- The publication FAIL carries `why "$out"`, as every BLOCKED arm in the same file already did.
  Without it the verdict was `rc=2 candidate=0 prior=1 rollback=no` — enough to know the cutover
  did not happen, and not enough to know why. Reading the sentence cost a second member.

With both, `dev/prove-cutover.sh` passes 4/4: publication, the candidate serving, rollback, and
the prior serving again after a restart.

## What passed, and what the provider did

| Proof | Result |
|---|---|
| `dev/prove.sh` | **9/9** after defect 1; 8/9 before |
| `dev/prove-confinement.sh` | **6/6**, and approval-off confirmed one-way again (§L-9): the restore was accepted and did not take |
| `dev/prove-cutover.sh` | **4/4** after defect 2 |
| `dev/prove-delivery.sh` | see below |

`dev/prove-delivery.sh` was run four times and never returned a FAIL. Each run blocked a different
one or two of its four checks, and every check passed in at least one run. The instrumented run
named the cause: `service_unavailable` from the model provider, which `dev/lib.sh`'s `degraded`
classifies as a dependency outage and scores BLOCKED. That is the right verdict and the right
behaviour — a measurement that did not happen — but it means **this run did not establish the
delivery boundaries in a single clean pass**, and a union across four runs is not the same claim.
That gap is this record's, not the code's.

> **Corrected 2026-09-09, later the same day, and the paragraph above is wrong twice.** The 503 is
> **not the model provider**: §G-5 and `DESIGN.md`'s upstream contract record that IronClaw's own
> create wait is `DEFAULT_RESPONSES_WAIT_TIMEOUT = 30s`, and a turn outlasting it is refused
> `503 service_unavailable` while **the turn keeps running server-side** and is recovered by
> replaying the idempotency key (ADR 0005).
> [`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md) measured exactly this
> question: eleven of seventeen refused, every refusal between 30.06s and 30.65s. So it is a slow
> turn, not an outage — and calling `dev/lib.sh`'s `degraded` verdict "the right verdict and the
> right behaviour" endorsed a misclassification that this repo had already written down twice.
> A direct check of the provider the same evening returned 5/5 on small completions, which is
> consistent with the cliff and was mistaken for evidence the provider had recovered.

The first delivery run blocked three of four, immediately after `dev/prove.sh` had spent nine
checks' worth of turns. That is the documented cause of a BLOCKED run — two costed proofs inside
one 30-request window — and it is why the later runs each waited a full window or two.

## What this says about the suite

Neither defect was reachable from `cargo test`. The first is about when a file is created relative
to a refusal that happens three layers up; the second is about a runtime's ambient configuration
that no fixture has. Both were found by the first live run in the pass, and both are now covered:
`arm_call_log` has a test with four cases, mutation-verified in both directions, and the cutover
precondition is enforced by the script that needs it rather than by an operator remembering.
