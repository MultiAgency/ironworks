---
status: accepted
date: 2026-09-06
narrows: 0001
---

# Rename the Python implementation to `ironworks-py`; this tree is IronWorks

**Decision:** The Python implementation is renamed `ironworks-py` and leaves active development,
but remains writable while it is the only provisioning system. This Rust tree takes the name
**IronWorks**, unqualified and unrenamed. GitHub archiving and decommissioning the
`/opt/ironworks` deployment both wait on §F.

## Context

ADR 0001 decided replacement and set the bar: *"done is migration, not evaluation."* That bar has
not been met. The Python tree is not dormant — seventeen commits in the thirty days to 2026-09-06,
the most recent on 2026-09-01 titled *"fix Linux provisioning"* — and this tree still cannot
provision or deprovision at all. `DESIGN.md`'s status is explicit: the operator binary "does not
create external authority, activate from a deployment measurement, destroy authority, or maintain
the residual-authority ledger §F requires."

So the incumbent is ahead on lifecycle and behind on serving, and the two systems have coexisted
under **four** names for it. **IronWorks** in prose; **rustworks** on disk, in the Pages project and
as the GitHub repository; **`ironworks-*`** across twenty crates; and **`ironworks-rs`** in
`Cargo.toml`'s `repository` field — which named no repository at all. That fourth one is the
instructive case: it was not a second name for this tree, it was a wrong one, and it survived
because **nothing checks a URL**. `ironworks-claims` verifies that a referenced path exists, that a
`path::symbol` resolves and that a cited experiment is real; a manifest field pointing at a
repository nobody owns passes every check there is. It is corrected as part of this decision.

Only `rustworks` was ever intended to be temporary: `deploy/README.md` records that
`/opt/rustworks` exists because `/opt/ironworks` is "a live tree, written to the same day this was
checked", and that the two "coexist for as long as retirement takes".

The question this record answers is not which name is better. It is **which tree owns the name**,
and **when the deployment follows the repository**.

## Decision

1. **The Python tree becomes `ironworks-py` and stops being developed.** It remains readable,
   because `docs/INVARIANTS.md` extracts its behavioural specification and §A–§J are still quoted
   against it — the reference corpus keeps its job.

   **It is deliberately *not* archived on GitHub.** Stopping product development does not make the
   repository read-only. Two reasons, and the first is operational rather than tidy: **it is still
   the only system that can provision**, so a
   tree nobody can push to is a tree nobody can fix on the day a client needs standing up. The
   second is that GitHub archiving disables Actions, and this repository has three workflows — one
   of them a weekly integration run — which would stop silently rather than visibly. Reconsider
   archiving when §F lands and the provisioning dependency ends.

2. **This tree is IronWorks, and keeps every name it already had.** The project, the binary and the
   twenty crates do not change — no Furnace, no Steelworks. See the rejected alternative below.
   What changes is only `rustworks`, which was never a second product name but a placeholder for a
   collision, and item 3 retires it.

3. **`rustworks` retires everywhere, and the Python tree moves aside rather than being removed.**
   The GitHub repository took the name first, because that touches no running system. The deployed
   names follow: `/opt/rustworks`, `/etc/rustworks`, `/var/lib/rustworks`, `/run/rustworks-attest`,
   the six units and the Pages project all become `ironworks`. `/opt/ironworks` is freed by moving
   the Python tree to `/opt/ironworks-py`, **not** by decommissioning it — it still runs the
   Account Service this system's live tenant reads its records from
   (`deploy/README.md`: "this tenant's records come from the Account Service"), so removing it
   would be an outage of the thing currently serving.

   This supersedes the two-stage plan first recorded here, which held the paths until
   decommissioning. That was priced for an outage that cannot happen: the only live tenant is
   MultiAgency's own org, dogfooding, with no external user to interrupt.

4. **Live documents move to `~/agency/ironworks-py`; dated records do not.** Twelve references
   exist across ten files. Nothing functional depends on any of them — the three in Rust are two
   doc comments and one test fixture string — so this is a prose change. Two are in
   `docs/experiments/*.json` transcripts and stay as written, because a dated record is never
   revised to agree with a later tree. Live documents and dated records will therefore disagree
   about the path, permanently and correctly.

### `rustworks` is four kinds of name, and one of them must not change

Discovered by sweeping, and worth stating because three of the four look identical in a grep:

- **Paths** — `/opt`, `/etc`, `/var/lib`, `/run/…-attest`. Rename, host and repository in step.
- **Unit names** — six files, plus every `After=`, `Wants=` and `ReadOnlyPaths=` that names them.
- **Unix accounts** — `ironworks` and per-tenant `ironworks-<slug>`, appearing as `User=`,
  `Group=` and `runuser -u`, owning `/var/lib` state at `0600`/`0700`. This is the axis that
  locks a service out of its own files rather than losing them, and it is the one a path-focused
  rename misses.
- **On-disk formats — renamed, and only because nothing is using them yet.** The two rollback
  sentinels in `crates/cli/src/registry_publish.rs` and the restic tag in
  `crates/cli/src/state.rs` are compared byte-for-byte on read, so they are formats rather than
  names. At any real scale leaving them would have been right; with one dogfooding tenant the cost
  is two follow-ups, which `deploy/README.md` carries. **The distinction survives the decision:** a
  string compared against something already written is not a name, and the next one will not be
  safe to rename just because this one was.

## The rejected alternative: renaming this tree

Renaming was considered seriously and at length, and is recorded here so it is not reopened without
new argument. Candidates and the reason each was refused:

- **Foundry** — the best-fitting word and the worst-available name. Palantir Foundry is an
  enterprise data platform sold for exactly these records; Azure AI Foundry is a platform name in
  the agent space; `foundry-rs` is the dominant Ethereum toolchain, in Rust. Two of the three
  collisions are in this product's market rather than its ecosystem.
- **Oxide** — Oxide Computer is a well-known Rust shop, and iron oxide *is* rust: the corrosion
  product a foundry exists to remove. Wrong on both the name and the metallurgy.
- **Alloy** — already a load-bearing term here: `composition::Alloy` is a public type, the journal
  carries an `alloy` column, and every experiment quotes an alloy digest. Naming the product after
  one of its own internal types makes "the alloy digest moved" ambiguous. Separately, `alloy` is a
  major crate namespace.
- **Bessemer** — too specific: a named 1856 process, plus a venture firm owning the search results.
- **Smelter, Furnace, Steelworks, Mill, Assay, Temper** — all workable, none better than the
  incumbent, and each costing 1,068 occurrences across 165 files, twenty crates, every deploy path
  and unit, and a permanent split with the dated records that cannot be rewritten.

The decisive argument is not aesthetic. **The collision that motivated a rename is temporary by
design**, and this record is what ends it. `IronWorks` also shares a morpheme with IronClaw, which
no candidate does, and *works* names the facility — composition, serving, confinement, lifecycle,
evidence — where every alternative named one vessel or one operation.

## What this obliges

- **§F is the gate on archiving and decommissioning.** Until end-to-end provisioning and
  deprovisioning exist here, the Python tree remains writable for operational fixes even though
  product development has moved here.
- **Update the twelve references deliberately, because nothing checks them.**
  `~/agency/ironworks-py` is not repo-relative, so `ironworks-claims` cannot see it: `is_ours`
  skips any path outside this tree's owned prefixes. This is the one class of citation the control
  is blind to, and the rename walks straight into it.
- **Revisit the framing, not just the path.** `README.md` calls it "the working Python
  implementation this replaces" and `CONTEXT.md` defines the *reference corpus* as the prior art at
  that location. "Working" becomes wrong when active development stops; "reference" does not, and
  should stay.
- **The repository is ahead of the host, and says so.** Everything here is `ironworks`; the machine
  still runs `/opt/rustworks`, `/etc/rustworks`, `/var/lib/rustworks` and `rustworks-*` units.
  `deploy/README.md` opens with that gap and carries the migration, because every command in it
  names paths the host does not have yet. Two details there are load-bearing rather than
  incidental: the published registry names its tenant's guidance by absolute path, so the old
  `/opt` path must survive as a symlink or the registry needs a republish that costs a member; and
  the account rename needs no `chown`, because ownership is by uid and the uid does not change.

### The repositories, done 2026-09-07

`MultiAgency/ironworks` became `ironworks-py`; `MultiAgency/rustworks` took `ironworks`. Neither is
archived, both clones are repointed, and `~/agency/ironworks` moved to match its remote. That is a
rename of two remotes and nothing else; `deploy/README.md` carries the deployed paths, which are a
separate switch.

**The second rename destroyed a redirect, knowingly.** GitHub redirects an old name only until a
new repository takes it, so an old link to the Python repository now resolves to this one: same
URL, different codebase, no warning. That was safe only because nothing pulls it — established from
`ironworks-py`'s own deploy documentation recording `/opt/ironworks` as a file copy, and from its
`sync-vm.sh` pushing with `rsync` from a `git ls-files` manifest.

**What appeared to establish it had established nothing.** An
`ssh <host> '...' || echo "not a git checkout"` printed its fallback because the shell read
`<host>` as a redirect and failed before `ssh` ran. `||` cannot tell *ran, answered no* from *never
ran* — the conflation §F-14 exists to prevent. Reading source answered what probing could not.

## Consequences

The half-migrated state is now named rather than drifted into. One system serves and cannot
provision; the other provisions and receives only operational fixes. That is a worse steady state
than either endpoint, so this record exists partly to make the discomfort visible: the way out is
§F, and the date on this decision is the date the clock started.
