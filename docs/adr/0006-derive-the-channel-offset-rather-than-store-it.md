---
status: accepted
---

# Derive the channel offset rather than store it

A long-poll channel is told an offset, and everything below it stops being redelivered. That
number is the only thing standing between a crash and a client reading the same answer twice, so
where it comes from decides the shape of the crash boundaries around delivery.

ironworks stores it. `bridge_state` keeps a `cursor` column and advances it as updates finish,
which means "this update is delivered" and "the offset moved past it" are two writes. Two writes
that must agree create a boundary — the one its corpus calls **B6** — and it closes that boundary
with a transaction, asserting the result rather than describing it:

> *"Boundary 6 cannot exist, and this proves it rather than asserting it in prose: a crash inside
> that write leaves NEITHER the delivered state nor the advanced cursor."*

That is correct, and it costs a stored fact that the update rows already imply.

We derive it instead:

> **The safe cursor is the lowest open update, across every tenant.**

## Consequences

**B6 is absent rather than enforced.** There is no method that sets an offset, so there is no
second write to be non-atomic with, and a cursor that has advanced past unfinished work has no
representation. The test that replaces ironworks' crash-inside-the-transaction case enumerates
every durable combination of turn outcome and delivery evidence and asserts the derived cursor and
the update's settlement agree — which they do by construction, because one is computed from the
other.

**Three separate mechanisms collapse into one line.** The cross-tenant barrier (a fast tenant must
not acknowledge a slow tenant's unfinished work), journalling the batch before any worker starts,
and graceful shutdown leaving queued updates for the next process are all consequences of `min`
over open updates. None of them needed to be implemented; each needed only to be tested.

**Acknowledgement stays stored, because it is not derivable.** What the channel was *told* cannot
be implied by any amount of local state, so `cursor_acked` is a durable, monotone value — and it
is the bound on compaction, exactly as before. This is the `DELIVERED` / `ACKED` split, and
deriving one half of it makes the other half's separateness obvious rather than subtle.

**The derivation must not read as a position when there are no rows.** Compaction deletes rows
below the acknowledged offset, which removes the maximum the "nothing outstanding" branch reads
from. The honest answer once every row is gone is *no offset to send* — `None`, not a lower
number — and the stored acknowledgement is what carries the position from then on. A derivation
that fell back to zero here would redeliver the entire history; that is the one failure mode this
choice introduces, and it is pinned by a test.

**A settled update is not a successful one.** Since the cursor is the lowest *open* update,
openness has to mean "automatic work remains", not "went well". A turn parked for operator
reconciliation is settled: it must not hold the offset, or one stuck conversation would deafen the
bridge to every conversation behind it while the process looked healthy. The alarm survives
separately — nothing compacts an update that cost a model turn.
