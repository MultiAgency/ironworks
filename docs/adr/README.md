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

## What each one turns on

- **0002, 0004, 0005** are the at-most-once execution set. They exist because the distinction that
  matters is whether a failed request *provably never left the process* or *may have executed*, and
  each answers one part of keeping that distinction decidable.
- **0003 and 0006** are about not trusting a convenience the runtime offers: an extension field for
  structured context, and a stored offset that only looks simpler than deriving one.
- **0001 and 0008** are the scope decisions. `0001`'s framing — replacement rather than fresh
  product — was itself superseded once parity with the Python implementation stopped being the
  goal, and `docs/PRD.md` §1 states the purpose that replaced it.
- **0007** settles what counts as delivery when there is no channel to deliver into.
