# 0061 — The freeze holds, because the other role cannot reach the bytes

**2026-09-09.** Disposable probe at `IRONCLAW_PIN` (`4cb47cfaf3`), profile
`hosted-single-tenant-volume-sandboxed`, `docker:29-dind`, model `qwen/qwen3.5-397b-a17b` from
`MODEL_PIN`, `PROBE_LOG=debug`. Two members minted for the run; **neither confined and neither
spent** (§L-9 spends a member when approval-off is written, and this writes no settings).
`dev/prove-freeze.sh`, six turns.

**Verdict: 5 PASS / 0 FAIL / 0 BLOCKED.** Candidate frozen at `a8d4a153c555c638`.

---

## What was measured

arXiv 2609.01481 §3.4.2–3.4.3 asks for three properties around an evolving artifact and states them
as one mechanism — the QA role receives a *frozen, read-only candidate*, so "the implementation
cannot change while its evidence is being collected" and "an assessment cannot combine behavior
from different artifact versions". This measures whether the seam can supply that on top of an
unmodified runtime.

```
PASS  freeze.developer_writes     the Developer wrote a candidate into its own workspace
PASS  freeze.isolation            the QA member's workspace does not contain the Developer's candidate
PASS  freeze.copy                 candidate frozen at a8d4a153c555c638
PASS  freeze.candidate_identity   QA read the frozen candidate, digest matches
PASS  freeze.single_writer        a QA write did not reach the Developer's workspace
```

**`freeze.isolation` is the check that makes the rest mean anything**, which is why it runs before
any copy exists. If the two members shared one workspace, every later read would succeed and be
reported as the copy working — an empty store passing a cross-store test, in the shape §K-8
describes. It is scored three-valued: the candidate's value appearing is a FAIL, the sentinel
appearing with no value is a PASS, and **neither is BLOCKED** — because `|| echo NOTHING-THERE` is
a fallback, and a fallback cannot tell "the file was absent" from "the command never ran".

## Why this closes §L-7 rather than restating it

§L-6's guarantee is **custody-backed**: an applied confinement can be undone by the credential that
applied it, so the seam's ordinary tool-surface claim stops at what a member chooses to leave in
place. §L-7 is that limit written down.

**This boundary is not of that kind** — and the evidence for that is of two kinds, which this
record originally ran together.

*Measured, by the six turns above:* the QA member's `/workspace` did not contain the Developer's
candidate before the copy, and a QA write to its own `/workspace` never appeared in the
Developer's. That is one path tested in each direction.

*Source-read at the pin, not measured here:* a worker's bind mounts are built from **its own**
workspace — `prepare_container_binds(&workspace, request.mounts.as_ref())` at
`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process.rs:757` — so no other
user's workspace is on that container's filesystem to reach by any path. **No turn tried to reach
the Developer's bytes by an absolute path into the users tree**, which is the check that would
convert this reading into a measurement. Nothing a member writes to its own settings changes
either half. So the single-writer boundary,
the frozen candidate and the one-candidate-identity property are supplied by construction, and they
do not inherit §L-7's caveat.

That is the one architectural claim in the HoH plan that the seam can make and the runtime cannot:
IronClaw's own agent loop has no notion of a candidate, and the isolation it does provide is
per-user storage that only something outside the runtime can copy between.

## What this does not establish

- **Neither member is confined.** This measures workspace isolation and candidate identity, not
  tool authority. A role that cannot reach a *tool* is Phase 5's remaining half and needs
  `lifecycle confine`, which does spend the member. Nothing here claims a role cannot reach a tool;
  it claims a role cannot reach another role's bytes.
- **Nothing about a served host.** The copy is taken by the script standing in for the seam,
  through the probe's Docker volume. On a served host the same tree is at
  `/var/lib/ironclaw/<client>/workspace` owned by the service account, so the operation is a
  directory copy there too — unrun, and this proves nothing about one.
- **The strong form of the isolation claim is a source reading.** The turns tested one path per
  direction; that a QA member cannot reach the Developer's bytes *by any path* rests on the bind
  construction cited above, not on this run.
- **Nothing about a loop.** There is no `D_t` and no `E_t`, so no role carries anything to another
  across an iteration. This is the freeze in isolation, which is the part that was worth measuring
  before designing the state that moves through it.
- **The asks name the tool's wire spelling**, because the runtime otherwise refuses its own model's
  output (`docs/UPSTREAM.md` D10). Every turn here inherits that workaround; remove it and this
  proof blocks the way `0057` did.
- One sitting, one model, two members, one book.

## What changed

- `dev/prove-freeze.sh` — new. Declares its cost, and `CLAUDE.md`'s cost section names it.
