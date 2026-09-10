# Account coordination POC

The first product experience is MultiAgency coordinating its own accounts. It is dogfooding, not
an external tenant offering and not a claim that the Python implementation can be retired.

The product-facing name is **account coordination**. The stable service id remains
`relationship-intelligence`, because that id is already bound into guidance and instance
configuration. It is now the only service that ships, and nothing is served by default — a
configuration naming no service is refused.

## Operator journey

With the runtime and record-store credentials in the environment, ask from the terminal:

```sh
ironworks coordinate \
  --slug <multiagency-slug> \
  --guidance <private-guidance-path> \
  --journal <private-journal-path> \
  --ask "What needs my attention across our accounts right now?"
```

`coordinate` is deliberately a narrow alias: it selects `relationship-intelligence` and refuses
`--service`, so the operator-facing name does not require renaming the authority-bearing service
id. It otherwise uses the same verified, journalled turn path as `turn`.

**Expect that command to be refused about half the time, and read the journal before re-running
it.** This question generates a long enough answer to sit on the runtime's 30-second wait timeout
— `docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md` measured eleven
503s in seventeen asks — and the refusal says
`service_unavailable … (a turn MAY have run; it must not be repeated)`. The turn is not lost: it
completed server-side and is recovered by replaying its idempotency key. So:

```sh
ironworks status --journal <private-journal-path>
```

**`0 unresolved turn(s)` means the answer already arrived — stop.** Re-running is not a replay:
`coordinate` derives its key per invocation, so a second command against a settled journal mints a
new key, bills a second turn, and can leave an unresolved row where there was none. **`1 unresolved
turn(s)` means re-run the identical command**, which recovers by byte-identical replay in well
under a second, or up to ~16s if the turn is still running.

None of this applies on the channel path below: `relay` recovers on its next cycle, so
`dev/relay-loop.sh` absorbs the refusal with no operator action and the answer simply arrives
about fifteen seconds later than it otherwise would.

For the existing HTTP channel surface, select the same service explicitly:

```sh
ironworks relay \
  --registry <private-registry-path> \
  --journal <private-journal-path> \
  --channel <channel-adapter-url>
```

Every tenant it serves comes from the registry, including when there is one of them — `relay` has
no single-tenant path, so a channel cannot acquire a second and looser route into the serving
code. The entry names the service, the guidance, and a credential handle for each of the two
credentials; in a group it must also name the handle that summons the bot, or every message in
the room would cost a turn:

```json
{"tenants":[{
  "slug": "<multiagency-slug>",
  "channel": "<operator-room-id>",
  "org": "<multiagency-org-id>",
  "service": "relationship-intelligence",
  "guidance": "<private-guidance-path>",
  "address": "@<bot-username>",
  "records_credential": "env:IRONWORKS_RECORDS_TOKEN",
  "runtime_credential": "env:IRONWORKS_TOKEN"
}]}
```

`org` is what closes the operator-only binding this document deferred, and it is required: a
registry written before it existed is refused as a whole (§B-4) rather than served unchecked, so
adding it is a step in any upgrade rather than an option. It is **not** an authority —
`ironworks_domain::registry::ExpectedOrg` cannot become an `OrgId`, because a caller-supplied
scope key is the defect §B-1 exists to prevent. It is the org the operator recorded this tenant
as, and `crates/cli/src/lib.rs::boot_tenant` refuses to serve when the record store resolves the
credential to a different one. `registry publish-one` writes the value it just measured from the
store, so a published registry records what publication proved.

The guidance marker must bind both slug and service:

```text
<!-- tenant-guidance v1 slug: <multiagency-slug> service: relationship-intelligence -->
```

No private guidance, records, credentials or journal belongs in this repository.

### The operator-only binding, no longer deferred

**Closed.** This section recorded a deliberate POC deferral: the composition named MultiAgency, but
nothing compared the verified record scope against a separately configured operator org, and for a
one-operator POC deployment ownership, the explicit service selection and the guidance marker were
the boundary. It was to end before either of two events — a second tenant exists, or
`relationship-intelligence` is relayed into a channel the operator does not solely occupy.

It ended for a third reason instead: [ADR 0010](adr/0010-the-seam-serves-bespoke-per-client-service-shapes.md)
settled that the service varies per client, which retires the premise that a shipped composition is
always the operator's own.

What the deferral asked for is built: **which organization's records an instance may read** is
`org` on the registry entry, compared in `crates/cli/src/lib.rs::boot_tenant` against what the
record store resolved the credential to, refusing to serve on a disagreement. The organization
named in the composition remains reviewed prose, not a second metadata field that could agree with
the prose while still disagreeing with the runtime authority.

The original sentence said "no tenant registry required," which was right for one operator and
wrong the moment a second tenant exists: an expected org is per-tenant, so the registry is where it
belongs (operator, 2026-09-05). What survives of that clause is the part that mattered — **no
fallback literal.** The value is configuration and the authority is still the store's answer.

### Nothing in an answer says whether its records were real

Worth knowing before showing this to anyone. The seam prints the scope it verified and the store
that verified it — `ironworks: scope <org> verified by <endpoint> (evidence: record store)`
(`crates/cli/src/lib.rs::boot_tenant`) — so the fact is on stderr, once, before the answer. It is
not in the answer, and the two endpoints a demo switches between are both on `127.0.0.1`, which is
not a distinction an audience reads.

The fixture material is marked in the data — every record carries `_synthetic` and every domain is
under RFC 2606 `.example` — but **the model is not asked to repeat those markers, so a fixture
answer reads exactly like a live one.** `docs/FRONTEND.md` makes the opposite choice for the
published page deliberately, putting the markers on screen as the pitch rather than a disclaimer.
The terminal has no such affordance. If a session switches between the live tenant and
`coordination-eval` mid-flow, say so out loud; nothing on screen will.

## What it coordinates

The service derives current account state from dated evidence across onboarding, active delivery,
changes in commitments, renewal and offboarding. It separates requested from received, statements
from facts, and deleted identity from revoked authority. It names what waits on MultiAgency, what
waits on the counterparty, and what needs a human.

It does not advance a lifecycle state, assign work, write records, provision access, send outreach
or declare a process complete on its own.

## Evidence before private dogfooding

The public acceptance corpus is invented rather than redacted from MultiAgency's book:

```sh
# The graders' own controls, offline — they are tests, so they cannot be skipped by
# forgetting a flag the way a --self-test could be.
cargo test -p ironworks-graders

# After dev/probe-up.sh and its printed exports:
cargo run -p ironworks-eval --bin ironworks-eval
```

The seven cases cover an across-account attention brief, onboarding readiness, supersession,
contradiction, residual offboarding authority, embedded-instruction refusal and fail-closed
dependency ownership. Every answer is graded only after the turn proves it received all ten
synthetic records, and the driver invokes `ironworks coordinate` so the operator-facing entry
point is measured too. Execution and delivery recovery remain governed by `dev/prove.sh` and
`dev/prove-delivery.sh` rather than duplicated here.

### A grade and a stability result are different facts

Two shapes of run are recorded, and they must not be read as one another.

A **suite grade** runs each case once at a fixed alloy and says what the service does across its
obligations. It is a single sample per case. `0009-account-coordination-poc.json` is that shape at
alloy `a80c5ffed289fa6f`: **6 PASS / 1 FAIL stored**, and 7 PASS standing after one grader-false-
negative regrade on `ownership-default`.

**Both that alloy and that book are superseded.** The current composition is `3770e0942624bda4`;
the current book fingerprints `0f1291ddd6c78fcb` (ten records) where `0009` was measured over
`b32bd5a973ba7fdd` (six). The latest seven-case grade at the current alloy is
`0015-account-coordination-poc.json` — 7 PASS — but it too was run over the six-record book.
`0018` and `0019` were the first bound to the current fixtures, and both are single-case;
`0020`–`0023` and `0028` followed over the same ten-record book, and the paragraphs below are
what they found.

**`0020` closes that gap and does not produce a score.**
`0020-account-coordination-ten-account-book.json` is the first seven-case grade at
`3770e0942624bda4` over `0f1291ddd6c78fcb`: 4 PASS / 3 FAIL recorded, 5/7 after one grader false
positive entered in `regrades.json`. A second run of the same suite, uncaptured, scored 6/7. Two
samples per case is not a distribution, so **neither figure may be cited as a pass rate.** What the
run appeared to establish — that every failure came from one grader rule, absence framing — was a
reading of fourteen answers that **did not generalise**, and `0026` replaced it by reading every
recorded failure: **seven of fifty-nine are the model**, the rest the instrument. The injection
refusal held across every recorded injection answer — the thirty in
`0016-injection-frozen-graders.json`, `0017-injection-absence-as-form.json` and
`0019-injection-ten-account-book.json`, which is what `docs/experiments/README.md`'s
`0018`/`0019` row means by *"the refusal never moved"*. **The graders disagreed about those same
answers** (`0026` moves `injection` from 5/10 to 8/10 on regrade), which is the point: what held
was the refusal, not the score.

**`0021` corrected that grader and the suite produced a third distinct result** — 3/7, against
`0020`'s 4/7 and its uncaptured 6/7, at a composition that never moved. **Every case has both
passed and failed.**

**What the instability mostly turned out to be was the instrument**, and sixteen grader defects
were corrected across `0024`–`0027`, each with an accepted and a rejected control. Every one of
those corrections moved a verdict upward, which read as bias until `0034` searched the other
direction — the fifty-six answers that *passed* — and found a leniency defect. The asymmetry was an
artefact of only ever searching one way. `docs/experiments/README.md` carries the arc and is where
the current count of open defects lives — a second copy here is a second thing to keep in step.

Two durable things came out of it: `crates/eval/tests/stored_answers.rs` pins every recorded answer
so no grader edit can move a verdict without declaring it, and every new transcript embeds the
grader source that produced it.

A **stability probe** runs one case repeatedly at the same alloy and says how reproducible that
case is. `0010-account-coordination-attention-stability.json` is five runs of `attention` at that
same alloy: **2 PASS / 3 FAIL** — PASS, PASS, FAIL, FAIL, FAIL. Its predecessor
`0008-…-attention-stability.json`, five runs at `bdbd8e9025ac8c72`, read 0 PASS / 3 REVIEW /
2 FAIL. The composition moved between them; the instability did not go away.

So `attention` carries a PASS in the suite grade and is not reproducible. Both are true, and the
first alone would misdescribe the service. `crates/site`'s arc page keeps them apart structurally —
a probe is never a column in the arc's matrix and never enters a suite tally — because a one-case
run scored against runs that graded the suite is exactly the confusion this section prevents.

**This is the result, not a blocker.** Synthetic prompt optimization stops here: the remaining
variance is a property of the model on this question, and the next evidence that can move it is
supervised dogfooding against real coordination work, not further tuning against an invented book.

The POC may advance through supervised agent-user dogfooding; Claude Code's use and changed review
decisions are valid evidence for that purpose. Repeated use by MultiAgency's human operator is
valuable confirming evidence, not a gate. Scope leakage, invented evidence, unsafe ownership,
effect execution, broken recovery or duplicate delivery remain blocking. This status is not a
claim of external customer demand or market validation.

## Inspecting a stuck turn

Supervised use keeps one persistent journal per serving path. Inspect it without opening the
runtime, record store or guidance:

```sh
ironworks status --journal <path>
```

The command lists every turn that is not both completed and delivered, including its key,
instance, organization, model outcome, delivery evidence, retrieval timestamp and response id
when known. It opens the journal read-only, refuses unknown schemas and does not create a missing
journal. For an operator-required row it also prints the journal's escaped technical reason. It
makes recovery state legible; it does not replay, reconcile, deliver or clear it.

For `coordinate` and `turn`, stdout is the delivery surface. The command records delivery only
after the complete answer is written and flushed. A crash in the residual window may print the
same known response again on recovery, never execute a new logical turn; once the recovered output
flushes, the persistent journal is available for the next question. See ADR 0007.
