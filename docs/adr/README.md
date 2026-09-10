# Architecture decision records

Decisions that are hard to reverse, and the reasoning that was available when they were made.

**An ADR is a record, not a specification.** It says what was decided and why, at a point in time.
When a decision is later narrowed or replaced, the ADR gains a status line saying so — it is never
rewritten to look as though the newer decision was always the plan. The bar for current *behaviour*
is `docs/INVARIANTS.md`; what a capability must *become* is `docs/PRD.md`.

Each file carries `status` in frontmatter so the set can be read without opening all of it.

| | Decision | Status |
|---|---|---|
| [`0001`](0001-reimplement-ironworks-in-rust.md) | Rust IronWorks is a replacement implementation — specification-compatible, not storage-compatible | accepted |
| [`0002`](0002-create-then-poll-not-streaming.md) | Execute turns by create-then-poll, not SSE streaming | accepted |
| [`0003`](0003-envelope-in-input-not-x-context.md) | Carry the envelope in `input`, not in `x_context` | accepted |
| [`0004`](0004-per-phase-timeouts-for-failure-classification.md) | Configure per-phase HTTP timeouts, never a global one | accepted |
| [`0005`](0005-reproducible-envelope-for-crash-recovery.md) | Make the envelope reproducible so a crashed turn can be recovered | accepted |
| [`0006`](0006-derive-the-channel-offset-rather-than-store-it.md) | Derive the channel offset rather than store it | accepted |
| [`0007`](0007-flushed-stdout-settles-direct-delivery.md) | Treat flushed stdout as direct-turn delivery | accepted |
| [`0008`](0008-single-tenant-deployment-multi-tenant-code.md) | Deploy one seam per tenant; keep the code multi-tenant | adopted for the sandbox-backed deployment, scope narrowed |
| [`0009`](0009-align-the-serving-cell-with-one-tenant-channel-stream.md) | Align one tenant's serving cell with one channel stream | accepted |
| [`0010`](0010-the-seam-serves-bespoke-per-client-service-shapes.md) | The seam serves bespoke per-client service shapes | accepted |
| [`0011`](0011-rename-the-python-implementation-to-ironworks-py.md) | Rename the Python implementation to `ironworks-py`; this tree is IronWorks | accepted |
| [`0012`](0012-defer-the-move-to-the-production-composition-profile.md) | Defer the move to the `production` composition profile; document the mismatch instead | proposed — deferred, with the trigger to revisit named |
| [`0013`](0013-one-evidence-vocabulary-with-two-existing-projections.md) | One evidence vocabulary — `Finding` — and the two existing verdict enums are projections of it | accepted |
| [`0014`](0014-the-evidence-state-is-append-only-and-derives-what-it-can.md) | The evidence state goes through `ironworks-store`, appends and never rewrites, and derives everything it can rather than letting it be authored | accepted |

## What each one turns on

- **0002, 0004, 0005** are the at-most-once execution set. They exist because the distinction that
  matters is whether a failed request *provably never left the process* or *may have executed*, and
  each answers one part of keeping that distinction decidable.
- **0003 and 0006** are about not trusting a convenience the runtime offers: an extension field for
  structured context, and a stored offset that only looks simpler than deriving one.
- **0001, 0008, 0009 and 0010** are the scope decisions. `0001`'s framing — replacement rather than
  fresh product — was itself superseded once parity with the Python implementation stopped being the
  goal, and `docs/PRD.md` §1 states the purpose that replaced it. `0009` makes `0008` mechanical by
  aligning the tenant process and journal with the channel credential's single cursor stream.
  `0010` moves the axis again: the tenancy decisions above assume one service served to many
  tenants, and it records that the *service* varies per client. It narrows `0001` for the same
  reason `0001` was narrowed once already, and it is what `docs/PRD.md` §20 and §23 are revised
  against rather than rewritten to match.
- **0007** settles what counts as delivery when there is no channel to deliver into.
- **0011** ends the naming collision `0001` opened. It narrows `0001` by separating two switches
  that had been treated as one — renaming the Python repository, which happens now, and
  decommissioning its deployment, which waits on §F — and it settles the name question by recording that the
  collision motivating a rename was temporary by design. It is also where the rejected names are
  written down, so the question is reopened only with a new argument.
- **0012** is the one deferral in the set, and `proposed` is its status on purpose: it keeps the
  served deployment on `hosted-single-tenant-volume-sandboxed` rather than `production`, writes the
  case for moving out in full so the next person to notice the mismatch
  `docs/experiments/0051-the-probe-and-the-served-host-resolve-authority-differently.md` measured
  finds the weighing rather than repeats it, and names the trigger that reopens it. It narrows
  `0008`.
- **0013 and 0014** are the evidence state. `0013` decides what an observation says — one
  vocabulary, `Finding`, of which the existing verdict enums are projections — and `0014` decides
  where it lives: through `ironworks-store`, appended and never rewritten, and derived wherever it
  can be rather than authored.
