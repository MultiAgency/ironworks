# What evidence `Stage::MemberConfined` must carry

Type: grilling
Status: open
Blocked by: —

## Question

`docs/INVARIANTS.md` §I names this as a gap with a known shape:
`crates/lifecycle/src/state.rs::Stage::MemberConfined` is a **bare stage** — recordable with no
evidence, so a tenant can be marked confined without anything having measured it. Everywhere else
this repository refuses that shape: `ScopeEvidence` records *how* a scope was established
**because a boolean cannot be audited**.

A second tenant is what makes this urgent rather than untidy. §13's Target turns confinement from a
one-off operator command into a per-tenant state that is **reconciled continuously**, and §L-4 and
§L-6 together mean a confined member is a state that can be left silently by anything holding the
credential. A stage that can be written without evidence cannot support a reconciliation claim.

Settle:

- **What evidence the stage carries** — by analogy to `ScopeEvidence`: what was written, as which
  member, against which catalogue, at what time, and what the resulting fingerprint was.
- **How it relates to the vacuity gap beside it.** §I: empty catalogues are refused, but a
  *truncated* all-disabled catalogue still verifies —`crates/lifecycle/src/confinement.rs` has no
  control requiring a **known egress tool be observed disabled**. Evidence that cannot distinguish
  a complete catalogue from a truncated one does not close this.
- **What the catalogue floor is**, given it is contested and dated: §L-8 states 50 disabled + 2
  locked for a **minted** member (52 rows), while an env bearer reports **53**. That difference is
  measured, not theoretical — `docs/experiments/0037-what-the-seam-never-asked-for.md` check 3 —
  and any evidence shape has to survive it.

Remember the term: confinement is **custody-backed**, not enforced. Whatever this records must not
read as a guarantee it is not.
