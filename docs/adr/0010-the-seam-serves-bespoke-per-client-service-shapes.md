---
status: accepted
date: 2026-09-05
narrows: 0001, 0008
---

# The seam serves bespoke per-client service shapes

**Decision:** MultiAgency sells operated agentic services to client organizations, and **the
service varies per client**. `relationship-intelligence` is the first worked example rather than
the product. The seam's job is therefore to carry a *shape* it was not written for, safely, and to
say what changed when it does.

## Context

Every prior scope record in this tree was written for one service served to many tenants.
`docs/PRD.md` §23 orders the work toward *"a second operator-provisioned client tenant"*; §14 says
one service ships; §3 describes a single vendor tenancy model. That ordering is not wrong about
tenancy — it is silent about the axis that actually varies.

Four axes vary per client, and the second client exercises all four:

- **what the service does** — its persona, its skills, and whether it *acts* rather than only reads;
- **what records it reads** — not necessarily Org → Account → Contacts + Activities;
- **where those records live** — not necessarily a store MultiAgency operates;
- **how it is reached** — not necessarily one Telegram bot.

Each of those is currently frozen, and none of the freezes is an oversight. `Access` has no `Write`
because effect execution was not implemented and *"an unimplemented capability that already has a
name in the type system is one somebody will eventually pass"*
(`crates/domain/src/capability.rs::Access`). §A-3 forbids the model reasoning or executing. PRD §6
fixes four entities under a complexity rule. PRD §19.1 chose platform custody on the premise that
the record store is MultiAgency-operated. PRD §15 has one adapter.

So the work is not four features. It is **four amendments, each owing evidence**, and this record
exists so that each one cites a decision rather than discovering it mid-build.

## Decision

**The seam serves service shapes, not one shape.** Concretely:

1. **A shipped composition may address a specific organization, and that organization is no longer
   always the operator's.** The hardcoded operator-org test in `crates/services/src/service.rs`
   therefore does not survive this decision. The organization name is part of the reviewed prose,
   not duplicated into definition metadata: agreement between two committed strings would say
   nothing about the server-resolved records authority, and an organization-neutral future service
   would still not make an omitted service selection valid.

2. **Which organization's *records* an instance may read stays a runtime check, from
   configuration.** `docs/ACCOUNT-COORDINATION.md` already prescribed it as the resolution of its
   operator-only deferral: instance construction fails closed unless the server-resolved org matches
   an org supplied by configuration, with **no fallback literal**. That is a different mechanism
   from (1) and must stay one — (1) is about prose and is checked at build; (2) is about records
   and is checked at boot. It is the only organization binding, because it compares configuration
   with the authority's own answer rather than comparing one local declaration with another.

3. **Effects arrive through the seam, not through the runtime — and the seam already acts.** This
   is not a new capability. `crates/records/src/append.rs::AppendClient::append` performs
   `POST /append_activity` today under a distinct `RecordAppend` credential, driven by
   `crates/cli/src/lib.rs::confirm_command`, and `docs/INVARIANTS.md` §I already records what is
   wrong with it: *"a `CapabilityProfile` cannot say whether a tenant may promote"* — that authority
   is carried instead by `TenantEntry::append()` returning `Option<&CredentialRef>`, *"a second,
   unmodelled axis. Two places to look for one question."*

   So the decision is not *let a service act*. It is: **make the effect the seam already performs
   expressible in the capability model, and then admit a second shape of it.** The order matters —
   it is what makes the approval invariants extraction from a running mechanism rather than
   invention, which is what "amend deliberately, with evidence" requires.

   The rule stays: the model proposes in prose; a person confirms; the seam executes. Note that
   *"the model never acts"* is today a **consequence** — the model has no tools and the effect sits
   on a separate command path — not a property anything enforces. Once a second effect shape
   exists it must become stated and enforced, or it is a marketing claim. §A-3 is untouched either
   way, because nothing declares `tools`.

4. **A second record model and a non-MultiAgency-operated store are amendments to PRD §6 and
   §19.1 respectively, and each is decided in its own record rather than arrived at by writing an
   adapter.** §19.1 names exactly one reopener for the custody decision — the Account Service
   ceasing to be MultiAgency-operated — and a client-hosted store is that reopener. It is also the
   only change that makes *"the operator cannot read your records"* sayable, so it must not happen
   as a side effect of an integration.

**What does not change.** Isolation is not on the table. §B-1's server-side org resolution, §B-7's
audience identity, per-tenant runtime members and confinement, and ADR 0009's serving cell are the
properties being *sold*, not costs being paid. Generality that erodes them is not generality; it is
a different product with a worse claim.

## The rejected alternative: the tool lane

`/v1/responses` accepts client-declared tools and parks a run for the caller to execute
(`docs/experiments/0031-the-external-tool-lane-is-open.md`). Declaring them is the obvious way to
let a service act, and it is refused here for the first acting service.

`docs/experiments/0035-the-tool-arm-read-the-records-out-of-memory.md` ran two arms differing only
in whether `tools` was declared. With it, the runtime ran **its own** loop over its built-in
catalogue; on an unconfined member that loop called `ironclaw.memory.*`, recovered 63 KB of the
tenant's account records from earlier turns, and answered from those, having been supplied no
records at all. It never called the declared tool. Confinement closes that specific hole, and the
cost objection is dead — the confined two-step loop billed 20,910 input tokens against a 20,899
baseline — but what remains is the thing §A-3 actually guards: a `tools` declaration is a request
for **agency**, and the second loop routes around `ironworks_domain::envelope`, which is §A's gate.

§A-3 enumerates six obligations an amendment owes. They are not discharged here, and this decision
does not weaken them. It defers them: the seam-owned effect lane is a strictly smaller amendment in
the same direction, and its approval invariants are a prerequisite of the tool lane rather than a
detour from it.

## What this obliges

- **`Access` gains no `Write` variant.** The premise of its doc comment has expired — effect
  execution *is* implemented — but its rule survives, and `Access::Write` breaks it the same way a
  `ToolAccess::Full` would: *"may write"* is broader than what is true, and a name meaning all of it
  is one somebody eventually selects. Follow the precedent `ToolAccess` already sets, which is a
  separate type precisely because *"it is not a read/write question"*: a distinct effect axis whose
  only non-empty variant means **confirmed by a person**, with no unattended variant to select.
  `CapabilityProfile::writes` retypes onto it; shipped `"writes": "none"` still parses, so there is
  no data migration. `Access`'s doc comment is revised to record that the premise moved and the rule
  did not.
- **The `approvals` invariants get written.** `docs/INVARIANTS.md` keeps `approvals` in the
  authority taxonomy as a named absence: *"when propose-and-confirm arrives, it arrives without
  precedent and will need invariants written rather than extracted."* Half of them are already
  *enforced* by `crates/proposals` and merely unstated — an unconfirmed echo cannot be confirmed,
  and a proposal belongs to the actor who made it, both made unavoidable by `Confirmable` having no
  public constructor. Those are extraction. The rest are not: that a confirmation authorises exactly
  the proposed bytes and no others, that an effect is attempted at most once, and that an
  unconfirmed proposal expires rather than lingering as latent authority.
- **Replay becomes a property of the effect, not a belief about the source system, and this must be
  written before the second effect exists.** `Confirmation::Appending` means *replay the
  byte-identical request*, and that is true only because the Account Service is idempotent on
  `(org, activity_id)`. **Nothing in the type says so.** A second source system without an
  idempotency key would inherit a state whose entire meaning is "send this again" — which is
  precisely how axis 4 of this decision turns into a duplicated effect nobody authorised. The
  licence to replay attaches to the effect by construction, and an in-flight effect that is not
  replayable and lost its outcome is permanently an operator's.
- **An effect adds crash boundaries**, and three of them are B2, B3 and B5's shapes on a different
  lane: intent durable / request not sent; request sent / outcome not recorded; outcome recorded /
  reply not delivered. They are covered by tests in `crates/proposals/src/tests.rs` and numbered
  nowhere, and `DESIGN.md`'s table is explicitly the reference corpus's numbering — so they take a
  suffixed spelling, as B10b and B11b already do for variants, rather than minting reference
  numbers the reference does not have. **One is genuinely new and no existing row covers it:** a
  crash while an effect is in flight at a source system that cannot be asked whether it happened.
  Every recovery in this tree leans on idempotency; that one cannot.
- **The two unmodelled-authority gaps in §I close with it, or the capability model stays
  decorative.** A registry entry carrying an effect credential under a service that does not declare
  the effect must be refused before any tenant is served — one question, one answer, rather than
  §I's "two places to look." And the profile must be consulted **at the effect boundary**, not only
  at instance construction: §I records that nothing on the serving path reads a `CapabilityProfile`
  at all, which is defensible for a read and not for a write.
- **`AuthorityPath::Approvals` flips to implemented, and one test goes vacuous with it.**
  The former *approvals_is_the_only_unimplemented_authority_path* test asserted the
  unimplemented set is exactly `[Approvals]`; when that is false the assertion becomes a green that
  measures nothing. It is replaced with one that can still fail, not deleted (§F-14, §K-8).
  **Superseded after implementation:**
  `crates/domain/src/lib.rs::model_selection_context_binding_and_approvals_are_authority_paths`
  now asserts that the approval path remains in the live taxonomy.
- **PRD §1, §3, §20 and §23 now describe a different business than the one being run.** They are
  revised as a consequence of this record, not rewritten to look as though this was always the
  plan (`docs/adr/README.md`). §20's effect-execution exclusion is narrowed by point 3 above;
  §23's ordering keeps its one real gate — step 5, the per-tenant serving cell — and loses the
  assumption that a second tenant takes the same service.
- **Nothing here is measured yet.** This is a scope decision, and PRD §8's Today/Target distinction
  applies to every one of its four points.

## Reopened by

A client whose service cannot be expressed as *read the org's records, propose, let a person
confirm* — at which point §A-3's six obligations are the next thing to cost, in their own record.
