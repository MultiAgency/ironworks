---
status: proposed
date: 2026-09-07
narrows: 0008
---

# Defer the move to the `production` composition profile

**Decision: not yet, and this records why rather than leaving it to be re-derived.** The served
deployment stays on `hosted-single-tenant-volume-sandboxed`. The case for moving to `production` is
good and is written out below, because the next person to notice the mismatch should find the
weighing rather than repeat it.

**Status is `proposed`, deliberately.** An ADR records what was decided; here what was decided is to
defer, and marking that `accepted` would read as though the move had been rejected on its merits. It
has not been. It is waiting on a trigger named at the end.

## What prompted it

`docs/experiments/0051-the-probe-and-the-served-host-resolve-authority-differently.md` found that
`ironclaw serve` has four boot arms and only `production` resolves its runtime policy from
configuration; the served profile hardcodes its deployment mode, runtime profile and org
constraints. Every proof in this repository except two therefore runs on a stack that resolves
authority through a different constructor than the one serving tenants.

## The case for moving

- **The evidence base is already there.** `dev/prove.sh` and the rest boot `production`, on
  Postgres. Moving the deployment makes the served host the thing that has been proven, rather than
  its neighbour.
- **Runtime policy stops being a constant.** `production` is the only arm that reads `[policy]`, so
  deployment mode and default profile become operator decisions instead of compiled-in ones.
- **The sandbox is unused.** The only shipped service declares `runtime_tools: none`; nothing
  declares `sandbox`, and §L-10 records that no turn has ever run in that workspace.
- **A smaller model-visible surface.** `docs/experiments/0052-the-inspector-flattens-the-prompt-it-shows-you.md`
  finding 3c measured the served profile's composed prompt carrying one more capability —
  `builtin.shell` — and about 13% more estimated context per turn than `production`.
- **A large apparatus becomes unnecessary**: the per-client rootless Docker daemon, its lifecycle,
  and the preflights that police it.
- **No data migration is required.** `docs/experiments/0054-a-cutover-orphans-every-conversation-and-the-order-of-the-bump-decides-what-it-costs.md`
  established that nothing requiring migration is authoritative — the runtime's durable state is
  derived — and that a cutover needs a drain and a composition bump, in that order.

## The case against, which is what settled it

- **Nothing in `deploy/` provisions PostgreSQL, and there is no partial version.** Every reference in
  the tree is in the disposable probe overlay. A server, a per-client database and role, a TLS
  posture, a connection secret and a store master key would all be new — and the master key appears
  only under `dev/`, so the served host has never had one.
- **Backup does not reach it.** `deploy/backup.example.json` is closed-world by design, and covers
  neither today's runtime state nor a database. Adding a durable-state surface that the backup
  rehearsal cannot see would widen a gap §I already names.
- **`deploy/check-ironclaw-artifacts.sh` fails closed without Docker**, so it must be changed rather
  than left to become dead — and its manifest is an exact-match comparison, so that edit is
  coordinated across the builder and its offline test.
- **It spends a member and resets the lifecycle store.** `LifecycleStore::adopt` refuses an existing
  slug and nothing re-records a member for one, so the store is reset or re-slugged; confinement is
  one-way, so the re-confine is another member gone.
- **Two normative statements would be contradicted**, not merely outdated: §L-10 says the deployment
  *must* use the sandboxed profile, and this ADR's parent — 0008 — argues the model-to-world boundary
  from the worker container and the managed proxy.

## What this ADR changes about 0008, if it is ever accepted

ADR 0008's isolation argument rests substantially on the per-client rootless Docker daemon, which
exists to run sandbox workers that nothing declares. Under `production` there is **no
process-execution lane at all**, so the model-to-world boundary would be achieved by *absence of the
capability* rather than by containing it.

That is arguably the stronger claim. It is also **a different claim**, and 0008 does not make it —
which is the reason this is written down now rather than assumed during a cutover. There is an irony
worth keeping: 0008 names hosted multi-tenancy as the shape to rebuild toward "when the scale
exists", and the served profile already hardcodes that deployment mode. The move would bring the
storage into line with a mode the runtime claims today.

## Revisit when

- a client asks for a capability the sandbox provides — at which point the sandbox stops being
  unused and the case inverts; **or**
- the PostgreSQL apparatus is wanted for another reason, so its cost is no longer charged entirely
  to this decision; **or**
- the transfer bound in §I starts costing something concrete. It is currently *stated*, which was
  the sharpest cost of the mismatch: a reader can no longer mistake a probe result for a served one.

## What was done instead

The mismatch is documented rather than removed: `DESIGN.md`'s upstream contract carries the
four-arm reading, §I carries the standing transfer bound, and
`deploy/check-ironclaw-instance.sh` now asserts the served profile fail-closed, with
`dev/test-ironclaw-artifacts.sh` holding the unit and the preflight to one spelling.
