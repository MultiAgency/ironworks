# 0054 — a cutover orphans every conversation, and the order of the bump decides what it costs

**2026-09-07.** A rehearsal of the profile cutover discussed in
[`0051`](0051-the-probe-and-the-served-host-resolve-authority-differently.md) and
[`0052`](0052-the-inspector-flattens-the-prompt-it-shows-you.md): serve on the served
`hosted-single-tenant-volume-sandboxed` profile, tear the stack down so its durable state is gone,
bring up `production` on Postgres, and drive the **same journal** at the new runtime. Disposable
probes at `4cb47cfaf3` throughout; no member minted, no live host touched.

It was run to answer whether a cutover needs a data migration. **It does not — there is no migration
path at the pin and none is needed** — but it found one defect and settled one contract fact that
`DESIGN.md` records as undefined.

---

## 1. Upstream fails cleanly on a `previous_response_id` it has never seen

`MEASUREMENT` · settles an "undefined" in `DESIGN.md`

`DESIGN.md`'s upstream contract says cross-user `previous_response_id` resume "is explicitly
undefined upstream — it surfaces as a turn-level failure, not a `404`." The neighbouring case — a
`previous_response_id` that exists nowhere at all — was not recorded either way.

Measured: **`404 not_found`, `field: response_id`, `[not_found_error]`**, on `POST /v1/responses`
with a `previous_response_id` naming a response destroyed with its volume. Clean, enveloped, and
rejected before anything ran.

## 2. The seam calls that "a turn MAY have run" — WITHDRAWN as a defect; the seam is right

`WITHDRAWN 2026-09-07` · the "Not established" paragraph closing this section is what settled it

A source reading at the pin answered the question this section left open, and the answer retires the
`DEFECT` label. **`param: "response_id"` is emitted from both sides of the dispatch boundary**
(`ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:607`) — the
`previous_response_id` miss at `:588` before submit, and `record_accepted_ack` returning `None` at
`:631`, twenty-four lines after it, with an identical envelope. So neither the status nor `param`
can separate them, the narrower fix proposed below does not exist, and `Unknown` is the correct
verdict. `docs/INVARIANTS.md` §I carries the withdrawal and the three things the reading *did*
settle.

**What survives unchanged is everything measured here**: the wedge, its two reproductions, and the
bump orderings in §3. Only the cause is renamed — replaying an unresolvable pointer, not
misclassifying the reply to it. The text below stands as written.

---

`DEFECT` *(withdrawn — see above)*

`crates/ironclaw/src/client.rs::interpret_error` reasons about exactly this and stops one status
short. It returns `NotSent` for `401`/`403` — *"Rejected before the handler saw it, so nothing ran"*
— and for `400` — *"Rejected on validation; the handler never dispatched a turn."* **`404` falls
through to the default wrapper and becomes `Unknown`**, whose contract is "it may have executed,
never retry."

A `not_found` on the `response_id` *field* is the same kind of thing as the `400` the function
already classifies correctly: the request was refused on validation and no turn was dispatched.

**What it costs, measured rather than argued.** The refused turn is journalled
`submitted_unknown`, holds the channel cursor, and recovery replays it byte-identically — the same
dead pointer, the same `404`. Two consecutive cycles reproduced it. Every conversation with a
thread pointer does this on its first turn after a cutover, and each one holds its room's cursor.

**Not established:** whether any `404` from this route can follow a dispatched turn. If one can,
the current conservatism is right and the fix is narrower — match on `param: "response_id"` rather
than on the status.

## 3. The composition bump is both the prevention and the escape, and the order decides which

`MEASUREMENT`

`ironworks_domain::continuation::decide` returns `FreshBecause::CompositionMoved` whenever the
prior thread's alloy differs from the current one. Three orderings, all measured:

| when the composition moves | what happens |
|---|---|
| **before** the first post-cutover turn | `thread: fresh — composition moved`; **no chain attempted**; envelope back to 40 records complete. The cutover is invisible |
| **after** a turn has already wedged | the rebuild is no longer byte-identical, so replay refuses: **`OperatorRequired`**, and *"it no longer holds the channel cursor"*. The room resumes; one update needs a person |
| never | wedged, cursor held, indefinitely (§2) |

The middle row is the part worth knowing: **the seam does not wedge permanently.** A bump releases
the cursor even when applied late, because `Journal::begin` records the decision and recovery
replays it, so a moved composition makes the replay impossible rather than wrong. The escape costs
one reconciliation per stranded update.

## 4. Draining to zero before the cutover is load-bearing

`MEASUREMENT` (the gate) · `REASONED` (the consequence of skipping it)

`ironworks status --journal` reports the count, so *zero outstanding* is a query rather than a
judgement — the same standard `deploy/README.md` holds the Python bridge to.

An unresolved `submitted_unknown` turn carries **no response id**, so its recovery is not a fetch
but a byte-identical replay under its idempotency key. Against a runtime whose storage was
destroyed, that key has never been seen — so the replay is a **new turn**, billed, while the
original answer is gone. That consequence is reasoned from the recovery path, not measured; what
was measured is that the gate reports the state accurately, including the one this run produced by
accident.

## 5. Two things the rehearsal established without being asked

- **A direct `ironworks turn` never records a thread pointer.** The `threads` table is keyed by
  channel, so continuation is a property of a conversation. The orphan hazard is **relay-path
  only**, and a first attempt at this experiment measured nothing for that reason.
- **With the record store unreachable, the seam refused to serve and left the turn recoverable**
  rather than settling it or answering degraded — `Rebuild::Unestablished`, on a real outage caused
  by a killed fixture rather than by a fault injector.

---

## The procedure this implies

1. Stop the relay.
2. `ironworks status --journal` on every serving cell; require **0 unresolved**.
3. Tear down; bring up the new profile.
4. Re-mint the member, adopt, confine, certify — the toll any profile change charges, because the
   confinement baseline is profile-relative (`0039`).
5. **Publish a guidance change before the first served turn.** Not after.
6. Resume.

Step 5 is the one that is not obvious and the one this record exists for. Steps 2 and 5 are both
cheap; skipping either is measured above.

## What this did not establish

Nothing about the served host. This is two disposable probes on a laptop, and `0051` finding 1 is
the standing reason a result on one profile does not transfer to another — which applies to this
record as much as to any other. The 503 that ended step C is
[`0036`](0036-the-demo-question-lands-on-the-thirty-second-cliff.md)'s cliff, not a cutover
property, and `0044` is why it would have recovered on the next cycle.
