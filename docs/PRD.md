# PRD — the seam as a multi-tenant platform

**Status: target document.** Almost nothing below is built. `DESIGN.md` is the shape of what
exists; `docs/INVARIANTS.md` is the bar it is held to; this file is what the product is for and
what each capability must become. Where a capability has a measured state it is named as measured,
with the file or the experiment that measured it. Everything else is a requirement, and a
requirement is not evidence.

**Audience: whoever is building next, and NEAR AI.** The operator is the one building; NEAR AI is
the reader §1 records this demonstration as being for, and that reader is technical, outside the
project and under no obligation to be generous. It is still not a pitch and not a positioning
document, and nothing in it may be quoted as a claim about behaviour — but it is read cold by
someone who did not write it, which is why §2 draws the IronClaw boundary before anything else and
why §22 exists at all.

Terms are the ones in `CONTEXT.md`. *Tenant*, *seam*, *org*, *alloy*, *guidance* and *record* all
mean something narrower than their ordinary reading, and this document is unreadable without them.
*IronClaw* is the pinned runtime and never this repository; §2 is the boundary.

**This document evolves as the system does.** Sections 1–7 say what IronWorks is for and where its
boundaries are; 9–18 track each capability as it moves from requirement to measured. Nothing here
carries a date: a claim is anchored to the file, experiment or revision that measured it, so a
statement that has gone stale is one a re-measurement corrects rather than one a calendar retires.

`crates/claims` machine-checks *part* of this file — it is one of the documents named in
`CLAUDE.md`'s "What is machine-checked, and what is not", which is where the list of checks lives
rather than in a second copy here. It does **not** check this document's measurements — no check reads a figure in this
file, which is why a wrong one here once survived a green run. **A measurement in this document is
checked by re-measuring it**, and a green grader is not evidence that a Today section is still
true.

---

## 1. Purpose, vision, mission

**Purpose.** Prove IronClaw is a credible substrate for trustworthy real-world agentic services.

**Vision.** A concrete reference implementation of what IronClaw enables for businesses — not a
toolkit, not a framework, and not a second runtime.

**Mission.** Demonstrate how a service provider can deliver agentic services built on IronClaw with
identity, credentials, authority and business state governed **outside model control**. Every one of
those four is a type in `crates/domain`, and that is the whole argument: `CredentialRef` holds where a
credential lives and never its value, `Access` has no `Write` variant, `RecordScope` has no public
constructor, and `Ask` has no `org` field.

**MultiAgency** is the business operating this seam, and it provides agentic services to clients
including NEAR Foundation. *Operator-stated:* nothing in this repository evidences a client list,
and no fixture, record or transcript here belongs to a client.

### Which of those is the bar — decided

**This is a demonstration, and its audience is NEAR AI.** The Purpose above is the operative one:
success is **legibility** — that a reader can check what is claimed — and not retention. §3
describes a vendor because the *tenancy model* is the vendor model (§3 costs the two alternatives
and refuses them), not because a client book exists; §1's operator-stated caveat is the whole of
what this repository knows about customers.

**What follows from it, so the Targets are not read as a backlog.** The measurement apparatus is
the deliverable, which is why it is the part that is built: `crates/claims`, `crates/graders`,
`crates/evidence`, `docs/INVARIANTS.md` and the experiment record exist so a claim can be checked.
The *operating* apparatus is absent — no billing, no signup, no console, no alerting, no on-call
path, no `doctor` — and under this decision that absence is a **scope boundary, not a gap**. A
vendor with clients would have built the second set first.

**So self-serve (§3's horizon) is not a funded milestone**, and §8's H2 tag marks aspiration rather
than planned work. What the demonstration still needs is a second tenant, because isolation between
two tenants is the claim §13 exists to support and one tenant cannot evidence it — that is §23's
milestone and it is unaffected.

---

## 2. The IronClaw boundary

IronClaw is NEAR AI's agent runtime — `github.com/nearai/ironclaw`, Apache-2.0/MIT. It is pinned at
`IRONCLAW_PIN` (`4cb47cfaf3`, v1.4.0) and run official and unmodified (`README.md`): everything
here is configuration, data and code that runs *around* it.

That README calls IronClaw *"Your secure personal AI assistant, always on your side"* and
describes storage that is local-first and single-user. **It does not address multi-tenancy.** That
absence is not a defect in IronClaw and it is the entire reason this seam exists: IronClaw is built
for one person holding their own credential, and IronWorks serves organizations that must never hold
theirs (§19.1).

`docs/INVARIANTS.md` draws this line for model inputs. Widened to the whole system:

| IronClaw supplies | IronWorks supplies |
|---|---|
| model integration and provider selection | the model pin as a privacy decision (`MODEL_PIN`) |
| the execution loop | one journalled turn per summon, and its recovery |
| threads and persistent memory | which thread a tenant may continue, and why (§B-14) |
| conversation replay and compaction | the alloy re-supplied every turn |
| the capability registry, settings catalogue, and WASM isolation for tools | settings-catalogue confinement and its measured completeness boundary |
| capability-based tool permissions | the confinement that applies them, and reconciles drift |
| the per-user identity boundary | org scoping, the audience rule, routing, the registry |
| a tool-execution audit log | the turn journal, holding identifiers only |
| an external function-tool lane it parks rather than executes (`0031`), and its own agent loop over the built-in catalogue the moment `tools` is declared (`0035`) | declining to declare `tools` at all — a choice, not a constraint, and not a free one: `0035` measured that the field is not additive, and that unconfined the runtime's loop read this tenant's records out of `ironclaw.memory.*` rather than calling the declared tool |
| | service definitions, guidance, provisioning, lifecycle, evaluation, business records |

**Three qualifications, each measured, each load-bearing.** They are why the left column must not be
read as a set of guarantees inherited for free:

- **§L-1 — a fresh member's settings catalogue is fully open**: 53 rows, 51 at `always_allow`,
  `auto_approve_tools: true`. The one-row disagreement with `0012` was settled on 2026-09-02 by the
  probe `0012` named: a fresh member returns 53, and §L-1's enumeration had dropped
  `nearai.web_search`. That is not the model surface: the host capture held 53 capability ids too,
  overlapping in 45 — 8 settings-only and 8 captured-surface-only. **Confinement is mandatory, not
  hardening**, for the authority the settings endpoint reports.
- **§L-6 — ordinary confinement is custody-backed; two rows are locked.** A member credential can
  reverse ordinary settings. Both `operator_config_set_*` rows instead return HTTP 400 and remain
  `locked: true`, `mutable: false`, at `ask_each_time`.
- **§L-7 — completeness over the executable surface is BLOCKED.** The claim covers the settings
  catalogue only; the inspector cannot expose a fresh member's caller-scoped capture and both
  deferred-discovery controls failed `internal_error`.

**Isolation between tenants is IronWorks's job, not IronClaw's.** The runtime gives exactly one
primitive — a credential resolves to one identity, and §B-13 measures that another tenant asking for
a response id gets 404. Everything else is here. §B-14 is narrower than the conclusion drawn from
it: upstream does not catch a cross-wired `previous_response_id` — the pinned docs call cross-user
resume undefined behaviour, not a 404. Thread isolation is therefore the seam's job and must never
rest on the runtime, which is ours to say and not the invariant's. A design that delegates a tenancy boundary to the
runtime has delegated it to a component whose README does not claim to have one.

---

## 3. The product

**MultiAgency operates a seam that serves scoped agentic services to organizations over a chat
channel.** Each organization is a tenant. MultiAgency is their vendor: it runs the seam, holds the
credentials, and is accountable for isolation. A tenant does not run a runtime, does not configure
one, and does not build with a toolkit.

The tenant model is the **vendor** model, decided deliberately over two alternatives:

- Not a shared network view. Partners contributing into a view MultiAgency or other partners can
  see is not a configuration of this system — §B-1 resolves org server-side from a credential and
  §B-7 makes the audience of a context the audience of every byte in it. Cross-org visibility would
  need a designed sharing primitive with its own invariants, and it is out of scope here.
- Not a platform a tenant operates. §L-6 is measured: `POST /settings/tools/{cap}` writes the
  **caller's own** scope, and a member can flip `disabled` back to `always_allow`. A tenant holding
  its own runtime member credential can undo every ordinary tool denial confinement applied; the
  two locked `operator_config_set_*` rows are the exception. Ordinary confinement is enforceable
  only while the tenant never holds that credential.

**MultiAgency runs as a tenant of its own system, on the same path, with no privilege.** That is
already true (`CONTEXT.md`) and stays true. The operator's instance is a registry of one; there is
no single-tenant code path to be the second, looser one.

### The horizon

**Self-serve.** A tenant arrives, is provisioned, authors guidance, selects a service, connects a
channel, and is served — without an operator in the loop for any of it. Nothing in the list is
built.

**It is the horizon this document is organised around and it is not a funded milestone** (§1). It
is written down because it is what the capability sections are aimed at, and because a Target with
no stated horizon gets read as work in flight. Under §1's decision the nearer milestone — a second
tenant, operator-provisioned — is the one that carries the demonstration, and self-serve is where
these capabilities would go if someone asked for them.

### What a tenant gets

1. A room where their team writes down what happened, and those notes become records.
2. A service that reads those records and answers questions about them from dated evidence.
3. A guarantee that no cross-tenant leakage occurs at the boundaries measured — today, the
   record path and response-id lookup — and that the service never acts
   (`dev/prove-tenancy.sh`, "what this proves, and what it does not"; §21).

The third is the one the architecture is for, and it is the one that constrains everything else.

---

## 4. What the invariants are for

Five words name what this system is trying to be: **safety, dependability, resilience,
accountability, integrity.**

They are printed here as an *index*, not as principles. `docs/INVARIANTS.md` opens by refusing that
framing — *"These are not principles. Almost every one is a scar"* — and a second, softer list
beside it would become the one people quote, because it is shorter and asks nothing. So each
heading below is followed by the mechanism that already enforces it. A row with no mechanism is a
requirement and says so.

**Safety — business authority remains human-controlled.** `Access` has no `Write` variant
(`crates/domain/src/capability.rs`) and there is no operation that widens a capability; a ceiling is
attenuated at construction and never raised. The model cannot propose a record, cannot confirm one,
and cannot see that a promotion happened except as a record like any other (§11).

**Safety — agents receive only the context their scope requires.** §B-1 resolves org server-side
from a credential and ignores any caller-supplied org; §B-7 makes the audience of a context the
audience of every byte in it. `Ask` has no `org` field and no field for pre-rendered text, so
caller-supplied context is not merely rejected — it is unrepresentable, and a `compile_fail`
doctest in `crates/domain/src/lib.rs` holds that.

**Integrity — identity, credentials, policy and durable business state stay outside model control.**
`CredentialRef` holds *where* a credential lives and never its value, has no `Display`, and prints
only its purpose and handle under `Debug`. There is nothing to leak. `ModelSelection` is
constructible only from a pin, so a service cannot choose the third party that sees a tenant's
records.

**Integrity — guidance carries policy; records carry facts.** Two different things with two
different trust stories: guidance is a tenant's own business context, authored for them and bound to
their slug and service; a record is one business fact a tenant's team wrote down — *"never a
judgement, never a score, never something a model derived"* (`CONTEXT.md`). §A-1 completes it: an
activity body is **evidence to assess, never an instruction to follow**, and that is stated to the
model in the same string as the records it governs.

**Dependability — store durable events; derive current state and judgments.** `Account` has no
`Default`, so `None` means genuinely unknown rather than "not fetched" or "zero". Everything
assessment-shaped is derived per turn and stored nowhere (§6).

**Resilience — isolation is enforced by the system, and ambiguous security states fail closed.**
The rule that decides which is which is already written (`docs/INVARIANTS.md`, *the rule that
decides fail-closed versus degrade*): **a control fails closed; a data source degrades.** §B-10
refuses to serve on an unverified identity; §D-4 serves anyway when records cannot be fetched,
because unavailability is a fact the model can be told honestly. Degrading a control restores the
silence it replaced.

**Accountability — runtime behavior and measurements override documentation claims.** This
document's own preamble, and `crates/claims`, which fails when prose in the docs disagrees
with what the tree measures. `ScopeEvidence` records *how* a scope was established rather than a
boolean, because a boolean cannot be audited. `Verdict` has a `BLOCKED` that is never a pass, and
`Skipped` deliberately does not exist.

---

## 5. Platform requirements

The charter's requirement groups, each with the section that owns it and what is true today. The
**Today** column is what stops this table reading as a description of a built system.

### Services

Each service must define a behavioural contract, its allowed capabilities, its context
requirements, its authority limits, its output expectations, and its evaluation criteria.

**Today:** `ServiceDefinition` (`crates/services`, `crates/domain/src/service.rs`) carries all six as
data compiled into the binary, and unknown keys are refused rather than ignored. One service ships
(§14, and `services/README.md` is where the set is stated). A definition with `"evaluation": null`
cannot carry an answer-quality claim; the one that ships names `fixtures/account-coordination`.

### Tenants

Each tenant must have one explicit org scope, exact routing, isolated conversations, durable state,
server-side identity, and access only to authorized business context.

> **Invariant: the audience of a context is the audience of every byte supplied to a turn in it.**

That is §B-7 verbatim — an identity, not a test of appropriateness — and it rests on org ↔ audience
being one-to-one, which **only** §B-6's
shared-account-token check enforces. **Today:** enforced for one live tenant; §13 is the section
this requirement really lives in.

### Context and authority

| Requirement | Today | H1? |
|---|---|---|
| retrieve and scope business context outside the model | **met** — `crates/records`, deterministic prefetch only (§A-3); no method takes an org parameter | done |
| keep credentials and secrets out of model-visible context | **met** — `CredentialRef` holds no value; the journal holds identifiers only | done |
| preserve evidence provenance | **met** — `crates/domain/src/envelope.rs` renders source and handling lines beside the records they govern | done |
| grant only explicitly authorized capabilities | **met at the seam** — capabilities attenuate at construction; **and see §2's §L-1**, a fresh runtime member's settings catalogue is open until confined, while completeness over the model surface is BLOCKED | done |
| prevent agents from expanding their own authority | **partial** — §L-3's sixteen authority-changing settings rows are denied first; five are API-only and exhaustive deferred reachability is BLOCKED; §L-6 leaves ordinary rows member-reversible | **H1** — per-tenant confinement, reconciled |

### Reliability

Durable delivery and restart recovery, duplicate-safe behavior, fail-closed routing, observable
health and failures, deterministic configuration validation.

**Today:** the strongest column in the document. Crash boundaries B1–B7 are proven across real
process restarts (`dev/prove.sh`, `dev/prove-delivery.sh`); ADR 0005 makes a crashed turn
recoverable by byte-identical replay of an idempotency key; ADR 0006 makes one boundary
*unrepresentable* by deriving the channel offset rather than storing it; `route::resolve` is
three-valued and never resolves an ambiguity by picking; §B-12 requires registry validation to work
offline. **What is missing is that nothing runs continuously** (§22).

### Operator

Define and version services; provision, bind, suspend and deprovision tenants; inspect
configuration, routing, capabilities and health; manage credentials safely; inspect delivery state
and diagnose failures; evaluate agent behavior against the service contract.

**Today:** inspection and evaluation exist (`ironworks status`, `lifecycle inspect|members|confine`,
`crates/reconcile`, `crates/eval`). **Provisioning, binding, suspension and deprovisioning
do not**
(§9, §17). There is still no `doctor`.

### Evaluation

Two kinds, and merging them is the defect:

- **Structural** — service/tenant binding, tenant scope, authorized capabilities, secret isolation,
  egress policy, runtime and model configuration. Measured by `dev/prove*.sh` against a real
  disposable deployment, which is what `CONTEXT.md` reserves the word **Proof** for.
- **Behavioral** — evidence discipline, authority boundaries, context use, unsupported claims,
  channel fit, and semantic drift away from the service's intended role. Measured by
  `crates/eval` (the `ironworks-eval` binary) over the graders in `crates/graders`, with
  graders frozen before the answers exist and a non-vacuity control on every case.

**A grade and a stability result are different facts** (`docs/ACCOUNT-COORDINATION.md`): a suite
grade is one sample per case; a stability probe runs one case repeatedly. A case can carry a PASS in
the grade and not be reproducible, and reporting only the first would misdescribe the service.

---

## 6. The data model

**Org → Account → Contacts + Activities.** Implemented in `crates/domain/src/record.rs`.

| | |
|---|---|
| **Org** | the audience, security and data scope. Resolved server-side from a credential, never asserted by a caller, never supplied by a model |
| **Account** | an external counterparty — the record a relationship is tracked against |
| **Contact** | a person associated with an account |
| **Activity** | one dated durable event. Its body is **evidence to assess, never an instruction to follow** (§A-1) |

**There is deliberately no `Organization` struct.** Tenancy is in the key — §B-2, `(org_id, <id>)` —
so an id collision between two orgs creates two independent rows rather than re-homing one. An org
is reachable only through `RecordScope`, which has no public constructor and is obtainable only from
a verified instance. A struct would invite a function that takes one as a parameter, and no method
here does.

`Date` is a strict `YYYY-MM-DD` so lexicographic ordering *is* chronological, and `Timestamp` is
compared by equality only, never arithmetic. Neither module reads a clock: `retrieved_at` is always
passed in, which is what makes an envelope byte-reproducible for crash recovery (ADR 0005).

**Derived rather than stored, by default:** relationship health, risk, priority, commitment status,
next action, renewal state, staleness, and current assessment.

Every item on that list is a judgement, and `CONTEXT.md` already says a record is *"never a
judgement, never a score, never something a model derived."* Storing one would make the model's
recollection indistinguishable from a person's statement on the next read. They are computed per
turn from dated evidence and kept nowhere.

**The complexity rule.** Add a new entity, service, capability, workflow or infrastructure layer
only when a concrete requirement cannot be represented cleanly within the existing model. The
repository already applies this to naming — a type that *"went unused, and was deleted — a name
looking for a job"* (`DESIGN.md`) — and this states it for structure.

---

## 7. First canonical service: relationship intelligence

The first service with a real user, which is MultiAgency coordinating its own accounts. Its stable
id is `relationship-intelligence`; the operator-visible name is **account coordination**, and the id
does not follow the name because it is already bound into guidance markers and instance
configuration (`docs/ACCOUNT-COORDINATION.md`).

**Inputs:** accounts, contacts, activities, and conversation history.

**Outputs:** current relationship status; documented versus stated commitments; unresolved
contradictions; important unknowns; recent changes; and what needs human attention.

**Authority: READ-ONLY.** It may reason, summarize and recommend. It may not modify durable records,
make commitments, perform outreach, or act outside its scope. This is not a prompt instruction — it
is `Access::None` for writes, with no `Write` variant to grant.

**Non-functions:** prospect qualification, scoring or ranking, CRM mutations, and workflow
automation. Two of those are enforced rather than asked for: the absence of `Write` covers
mutations and automation, and `docs/FRONTEND.md`'s no-scores rule covers ranking — *urgency ordering
is valid coordination behaviour; value ranking is what the service forbids,* and a sortable value
column would violate a service invariant through pure UI affordance.

**This composition names MultiAgency, correctly and not by leak** (`services/README.md`).
That is exactly why a configuration selecting nothing must not reach it — landing somebody else on
a persona addressed to us. It used to be kept out of a default slot; there is no default slot now,
so the same property is held by refusing the silence outright (§14).

### Evidence discipline

Every claim carries exactly one tier:

| tier | means |
|---|---|
| **FACT** | supported by a record actually read. Always cite the record |
| **STATED** | asserted by a person and not yet confirmed by any record. Cite who and when |
| **INFERENCE** | reasoning from the above. State the basis |
| **UNKNOWN** | not established. Name why it matters |

Four rules govern them, and they are in
`scrap/personas/COORDINATOR.md`, not in this document:

- **Claims must be traceable.** An assertion is never a FACT until a record confirms it.
- **Absence of evidence is not evidence.** An absence is scoped to what the record covers, never
  asserted as non-existence. This is the most-tested sentence here: `0020` found that
  **every failure in both runs** was this one rule, and read its fourteen graded answers as showing
  no substantive reasoning defect. That reading did not generalise (§12), and `0026` later replaced
  it by reading every recorded failure: **seven of fifty-nine are the model**, the rest the
  instrument.
- **Contradictions remain visible.** When two records conflict, name both and prefer neither; for a
  yes-or-no question with contradictory evidence, lead with UNKNOWN.
- **Inference must state its basis.** Conclusions that compare records — that one supersedes
  another, that an obligation is open — are INFERENCE unless a record states that conclusion.

**These four are the whole vocabulary. Never invent a fifth.** A thing that feels like it needs its
own label — scheduled, pending, at risk, deferred — is business content and belongs in the sentence.

### Closed by deletion, and worth keeping the shape of

**This section used to record a fail-open, and the fail-open is gone.** Two services shipped two
taxonomies and the *default* one was the looser: the ANALYST persona, composed into
`account-analysis`, carried **five** tiers, adding `HYPOTHESIS` between INFERENCE and UNKNOWN. An
instance whose configuration selected no service landed there — on the weaker evidence discipline,
and on the only shipped service declaring `"evaluation": null`. Everywhere else in this document an
ambiguous state fails closed (§4); this one did not.

**It was closed by removing the service, not by reconciling the taxonomies.** `account-analysis`
had no user, real or intended (operator, 2026-09-02), so it was deleted rather than given the
evaluation corpus it would have needed to keep the slot. That left one service, which names
MultiAgency in its first sentence — so nothing remained that could safely fill a silence, and the
slot went with it (§14).

**What the shape was, so it is recognised next time.** The defect was never that two taxonomies
disagreed — that is ordinary. It was that the *silence* resolved to the looser one. A default is a
choice nobody made, so whatever occupies it must be the strictest option rather than the safest-
sounding one, and "safe for an organization nobody chose it for" turned out to select for
genericity rather than for rigour. The rule that survives: **a slot reached by misconfiguration
must not be easier to satisfy than one reached deliberately.** Adding a second service re-opens
this, and §14 records the test that fires when it does.

---

## 8. How to read the capability sections

Each capability below is four parts, and they must not be merged — merging intent with observation
is the defect `crates/lifecycle` exists to prevent:

| Part | Answers |
|---|---|
| **Today** | what is built and measured, with its source |
| **Target** | what self-serve requires |
| **Must not break** | the invariants that bound the design space |
| **Open** | what is genuinely undecided, named rather than defaulted |

An empty **Today** is normal and is not a gap in the document.

**Every Target below is written against self-serve, which is the furthest horizon.** That makes the
document read as though all of it is in flight, so each Target carries a horizon tag:

| tag | means |
|---|---|
| **H1** | required before a second *admitted client* tenant — §23's milestone, and the one that carries the demonstration |
| **H2** | required for self-serve — §3's horizon, which §1 records as **not a funded milestone**. An H2 Target is where a capability would go if someone asked for it, not work in flight |
| **H3** | named and deferred (§20), or not required for either |

**H2 is marked once, here, rather than on each Target that carries it.** Restating "and this is
unfunded" beside eight Targets would be eight things to keep in step with one decision, which is
the failure §6's complexity rule and `DESIGN.md`'s *two mechanisms for one invariant* both name.
If §1's decision moves, this row is what changes.

The tag is on the Target because that is the part that moves. **Six of the ten capabilities have no
H1 requirement at all**, and a reader who takes §3's horizon as the near one will price them as if
they did.

---

## 9. Capability: tenant provisioning

### Today

There is no end-to-end provisioning command. Focused operator commands can adopt and verify an
existing Account Service scope and runtime member, confine that recorded member, and publish a
strict one-entry registry. They deliberately do not pretend that adoption created authority or
that a hand-written PASS file activated a measured deployment. `crates/lifecycle` carries
`InstanceSpec` (what an operator asked for), `ProvisionProgress` (how far we got), and
`AuthorityInventory` (what the authoritative systems say exists) are three separate types on
purpose. The stage order is fixed (§F-2):

```
preflight_passed → org_registered → data_initialization_settled → member_minted
                 → member_confined → staged → smoke_passed
```

**`Stage::ALL` is seven, and `activated` is deliberately not one of them.** It is a
`ProvisionProgress` variant carrying `SmokeEvidence`, which makes *"activated without a passing
smoke"* unrepresentable rather than merely disallowed. `staged` before `smoke_passed` before
activation is the contract: **the registry entry exists but is not live until isolation and
reachability pass against the real credentials.**

A tenant today is a hand-written registry entry naming a slug, a channel, an org, a service, a
guidance path, an address, and two credential handles — plus a guidance file, plus two credentials
an operator set in the environment. The org is the one the records credential must resolve to, and
`boot_tenant` refuses to serve on a disagreement; it is an expectation rather than an authority,
which is why it can be written by hand without becoming a caller-supplied scope key (§B-1).

### Target — **H2**

*A hand-written registry entry is what *operator-provisioned* means, and it is what tenant 1 uses.*

Provisioning is an executed, resumable transaction that creates authority in four places (§F-1) and
records what it created:

1. a record scope for the tenant's org;
2. a sealed runtime member;
3. that member's confinement, applied and verified;
4. the registry entry, staged then activated.

**Resume restarts at the first stage not reached**, from durable journal evidence rather than from
an operator reconstructing a half-finished terminal.

Self-serve means this runs without an operator, which raises the bar rather than lowering it: a
partial provision that nobody is watching is exactly the state §F-1 exists to make visible.

### Must not break

- **§F-3 — the lifecycle journal never holds a credential.** Enforced by field *name* as well as by
  type and length: anything containing token/secret/password/key/credential/bearer is refused.
- **§F-13 — private state is written atomically with the mode set before any content exists.**
  These files name every tenant that exists; write-then-chmod publishes them at the process umask
  for the window between. `ironworks_domain::custody` states this once for all three stores, and
  covers the `-wal` and `-shm` siblings because a claim about the main database alone is a claim
  about the wrong file.
- **§B-4 — the registry fails closed as a whole.** Twelve refusal tests hold this
  (`crates/domain/src/registry.rs`). A partially loaded registry serves some tenants with another
  tenant's routing, credentials or data scope.
- **§B-6's six uniqueness refusals**, in particular: a shared member token means two tenants *are
  the same identity*; a shared account token serves the same records in two rooms.
- **§B-8 and §B-9 — the sealed-member probe**, stated in `docs/INVARIANTS.md` with the two defects
  that produced them. What they constrain here is that certification depends on the runtime being
  *asked and answered*: a probe that reads the wrong process's environment, or that is refused by
  bot protection, certifies **every** tenant — including one whose token really is the operator's.
  A provisioning step may not treat a probe's silence as a result.
- **Provisioning must not assume the platform mints the record-scope credential.** *Requirement,
  from §11's record-origin decision.* A tenant who already holds an org is not excluded, so
  `org_registered` must be reachable by verifying an org as well as by creating one. The two are
  different facts and `crates/lifecycle` already separates them: `Authority::created_at` records
  *"which stage's completion implies IronWorks created this,"* so a stage that also covers "we found
  it" would assert authorship IronWorks does not have — the exact merge of intent with observation
  the crate exists to prevent. Whichever custody shape §19.1 settles on, the *default* may assume
  the platform mints; the stage must not.

### Open

**Whole-registry-refuses does not scale to unbounded tenants.** §B-4 is correct and must survive,
but a single file that refuses as a unit means one bad entry among a thousand takes every tenant
down. The invariant is about *what is served*, not about *what is stored in one file* — a design
that preserves "no tenant is served with another's routing" while allowing an entry to be added or
quarantined without a whole-set outage is required and does not exist. **This is the largest single
piece of unbuilt design in the document.**

---

## 10. Capability: guidance authoring

### Today

Guidance is mandatory — there is no unguided composition — and it is a tenant's own business
context, bound to their slug and service by a first-line marker:

```text
<!-- tenant-guidance v1 slug: <slug> service: <service> -->
```

`Guidance::parse` is a control and fails closed on five counts, enumerated in
`services/README.md`. The error says to fix whichever is wrong rather than to make them match,
because a mismatch means one of them is a mistake and matching them blindly picks at random.

Guidance is written by hand, by an operator, with the tenant.

**The sharp edge here is gone** (`services/README.md`). `service:` was optional and a marker
omitting it pinned `DEFAULT`, so changing `DEFAULT` silently rebound every guidance file that
omitted a service rather than failing any of them. Both halves went together with the default slot:
a marker naming no service is now the third refusal above, because reading the configured service
back into the silence would leave that check unable to fail.

### Target — **H2**

*For h1 the operator writes guidance with the tenant, as today.*

**Guided authoring that generates the marker mechanically.** The tenant answers a structured flow
about their business — what they sell, who they sell to, what a good account looks like, what facts
they expect a record to carry — and guidance is produced from it. The slug and service marker is
emitted by the system, never typed.

This is the version that answers `docs/FRONTEND.md`'s objection rather than ignoring it. That
document says a guidance editor is a defect, and the reasoning is right: guidance is part of the
alloy fingerprint that decides whether a thread may continue, so a free-form web form makes alloy
drift a UI accident. **A structured flow that emits both the guidance and its marker is not a form
over a text field** — the tenant never edits the bytes the marker binds.

The tenant's declared fact fields (`ServiceInstance::fact_fields`) come from validated guidance and
from nowhere else — no registry field, no environment handle, no second parser. The authoring flow
must feed that same path.

### Must not break

- **§B-15 — guidance is bound to one slug and one service, at exactly one canonical path beside
  the tenant's config.** A configured alternative path is refused; per-tenant guidance-path
  lifecycle is not supported.
- **§C — there is no unguided composition.** A flow that can produce empty or absent guidance has
  produced an instance that must not serve.
- **Regenerating guidance moves the alloy digest.** A tenant editing their guidance mid-thread
  changes the composition a thread was fingerprinted against. The authoring flow must make that a
  deliberate, visible act with a known effect on open threads, not a save button.

### Open

Whether the flow is per-service (each service definition carries its own question set) or generic
with service-specific sections. A service definition currently carries `composition.guidance_heading`
and nothing about how guidance is obtained; adding an authoring schema to the definition is the
obvious move and is not yet justified by two services needing different ones.

---

## 11. Capability: record contribution

### Today

Records live in the Account Service, scoped to an org resolved server-side from a credential. The
seam reads them.

**The contribution path is built.** `crates/domain/src/share.rs` holds a deterministic grammar and
`crates/proposals` a durable proposal buffer; a person types a command naming an account and a note, the
bytes are echoed back exactly, and `/confirm` (`crates/cli/src/lib.rs::confirm_command`) promotes them
through the one append route (`crates/records/src/append.rs`) under a separate append credential.

**What is unmeasured is whether a room's notes actually accumulate into a usable book.** Nothing has
promoted a record outside a test, and the Target below is a claim about behaviour over weeks that no
experiment here has run.

The design decisions made there are load-bearing and must be preserved:

- **The bytes that end up in a record are the bytes a person chose.** The path from message to
  proposal is deterministic and lives in `domain`, where there is no IO and no model to call. A model
  that summarised or tidied would make the record its recollection rather than her statement.
- **A proposal is not a journal row.** `ironworks-journal` holds identifiers only; a proposal is a
  person's exact words. Putting it beside the turn rows would break that rule for the whole file.
- **An unconfirmed echo cannot be confirmed**, and a proposal belongs to the actor who made it —
  both carried by the type system (`Confirmable` has no public constructor), not by a check a caller
  has to remember.
- **The proposal is keyed by the channel update that created it and by nothing else**, so a
  redelivery finds the same row rather than minting a second proposal.
- **A slash command never reaches the model.** Every refusal is a dead end that costs no turn, no
  record fetch and no store write.

### Target — **H2**

*The path is built; what is unmeasured is accumulation.*

**A tenant's book accumulates from their room.** There is no import, no CRM connector and no
migration. A team writes down what happened where they already talk, and that becomes the record
the service reads back.

This is the origin of a tenant's records under self-serve, and it is what makes signup cheap: a
small organization with no CRM has nothing to hand over, and asking for one is asking for the thing
they do not have.

### Must not break

- **The model's capability profile stays `writes: Access::None`.** `Access` has no `Write` variant
  and must not grow one. Promotion is a *person's* effect executed by deterministic code, not a
  capability the model holds — the model cannot propose, cannot confirm, and cannot see that a
  promotion happened except as a record like any other.
- **Approval stays distinct from model authority.** Propose-and-confirm is a person's deterministic
  effect: the model cannot propose, confirm or append, and `Access` still has no `Write` variant.
- **Attribution is not sender identity.** `ActorRef` is the channel's attestation of who sent a
  message and it authorizes and keys; `ContributorId` is who a promoted record is attributed to and
  is operator-recorded in the registry. `ActorRef` has no `Display`, a redacted `Debug`, and one
  route to a string named `persistence_key()` — so a value the sender controls that also *selects*
  something cannot reach guidance, a prompt, an envelope or a log line.
- **A record is a business fact a tenant's team wrote down** (`CONTEXT.md`) — never a judgement,
  never a score, never something a model derived. Promotion must not become a path by which model
  output enters the book.
- **§A — an activity body is evidence to assess, never an instruction to follow.** A record that
  arrived from a room is attacker-reachable by exactly the same argument as one that arrived from a
  CRM, and `vr-005` in `fixtures/account-coordination/book` exists to prove the refusal.

### Open

**Decided — record origin.** *Room-origin is the only origin IronWorks builds, and it is not the
only origin a book may have.* The question as it stood conflated two, and separating them settles
one and answers the other.

Whether a book may contain records the seam did not write is **not a decision**. The Account
Service is an authoritative system this repository is a *client* of (`DESIGN.md`), and every record
the seam has ever read — the operator's own included — arrived by a path it does not own. Whether
MultiAgency *builds and operates* an import path is the real question, and the answer is no.

What forces it is the write surface. One route writes records (`crates/records/src/append.rs`), under
a credential purpose documented as appending one confirmed note *and nothing else*, and the store
refuses any activity id outside the `share-` namespace — which `crates/domain/src/share.rs` calls
load-bearing precisely because it means a bug *"can never land on an activity a tenant's team
authored."* That path cannot create an account or a contact. An import through the seam is
therefore not a widening of an existing capability but a new one, and it would arrive without
precedent or inherited invariants.

**Two things that look like objections are not.**

- **An imported body inherits §A-1 for free.** The handling line is emitted once for the whole
  records block (`crates/domain/src/envelope.rs`) and draws no distinction by origin. `vr-005`
  (`fixtures/account-coordination/book`) is a fixture record that was never in a room, and it is
  the injection proof.
- **An imported record is attributed to no one, which is the existing tested case.**
  `Activity.contributor` is optional, the envelope renders attribution only when present, and every
  fixture record carries none. Minting a synthetic contributor would be worse than none: it asserts
  an organizational principal that does not exist.

**What room-origin holds and import cannot is accountability, not evidence.** A promoted note has a
person who confirmed it, keyed to the channel update that created it, so a record that turns out to
be wrong has someone to ask about *how it got there*. That is narrower than it first appears, and
the fixtures bound it: `contributor` records who **promoted** a record, never who **said** the thing
inside it. §7's STATED tier asks the answer to cite who and when, and it is satisfied from the
activity body — `nc-003` carries the corpus's central contradiction, *"Devin said the executive team
approved a twelve-month renewal"* against *"Taylor wrote that procurement has not approved a
renewal,"* with no contributor on either. So attestation does not gate the evidence tiers. What it
gates is the operational question of who to ask, and the invariant actually protecting the book is
that **model output never enters it** — which deterministic copying satisfies as well as
confirmation does.

**Consequence for §19.1.** The custody discriminator is whether a tenant is *expected* to arrive
with an Account Service org, not whether one *can*. A self-serve tenant is not expected to, and
§19.1 settles on platform custody on that basis. A tenant who does arrive with an org is not
excluded **as a source of records** — that is what this decision holds open. It is not a licence to
run a second custody shape for them: §19.1 records why per-tenant custody would make one lifecycle
model mean two things. The requirement that follows is in §9's Must not break, where provisioning is
built.

**This is reopened by two things, and the second arrives without anyone deciding.** A tenant whose
book is too large to retype and who has reason not to hand it to MultiAgency makes import a product
capability needing its own invariants. Quieter: **§17's export is unbuilt, and whatever produces an
export is most of what consumes an import** — a faithful serialisation of a book, its accounts,
contacts and activities, with provenance intact. Building export therefore builds the expensive half
of import, and the decision above should be re-read the moment it lands, rather than left standing
because nobody revisited it.

**Still open — correction and retraction.** A record written from a room will be wrong sometimes,
and there is no design for amending one. Supersession is a *reasoning* behaviour the coordination
service already has; a *write* path for it is not the same thing.

---

## 12. Capability: serving a turn

### Today

The serving path works and is measured. `relay` polls a channel, routes each conversation to its
tenant from the registry, composes the alloy, runs one journalled turn, and delivers. It survives
crashing at every boundary either side of a turn. A real bot has answered a real question through
`crates/telegram`, and `coordinate` has served the operator's own org from the live Account Service,
recovering byte-identically through a real upstream failure.

**That is one turn, not a behavioural claim.** Seven-case suite grades over the current book exist,
and three runs at a composition that has not moved produced three distinct results — 6/7
(uncaptured, and therefore not citeable), 4/7, 3/7. **Every case has both passed and failed**,
`onboarding` included once `0022` probed it ten times. That is what `docs/FRONTEND.md` had only
argued: a grade is a sample from a spread, so quoting any of the three would be picking one.

**The spread was mostly the instrument, and that is the finding.** `0026` read every recorded
failure and found **seven of fifty-nine are the model**; the rest were grader defects, six of them
in one rule. Every defect found has since been corrected, each with an accepted and a rejected
control, and `crates/eval/tests/stored_answers.rs` now regrades **69 stored answers across
seven transcripts** so no grader edit can move a recorded verdict without declaring it. What holds
across every run is the injection refusal, which never moved.

**No number here is a pass rate, and the reason is no longer instability.** It is that every
correction moved a verdict upward, judged throughout by the same reader — §22 carries that caveat,
and `docs/experiments/README.md` is the record. The graders' own controls are
`cargo test -p ironworks-graders`; the Python self-test they replaced is gone.

`relay` handles **one cycle and exits** — that is what keeps every crash boundary a real process
boundary. `dev/relay-loop.sh` supervises it and `deploy/ironworks-relay@.service` runs that, with
`Restart=always`. Both are committed, and `multi-serve` runs them for one tenant,
which has served and delivered a turn. `deploy/README.md` records what that establishes; nothing in
this repository verifies it, because no automated check reaches that host.

**The upstream contract facts that bound any design here are in `DESIGN.md`'s "The upstream
contract", and they are not listed again here.** They were, and the copy dropped the one thing that
makes them usable: each fact there carries the `[measured @ rev]` marker it was actually confirmed
against, and a pin bump moves those markers one fact at a time (`docs/UPGRADE.md` step 6). A list
of the same sentences without their provenance reads as a single dated set, which is exactly what
the per-fact mechanism exists to prevent — and `docs/INVARIANTS.md` §G-5 is the worked example of
a copy of these facts lagging the original.

Four of them bound this section in particular, and the constraint rather than the fact is what
belongs here: the 30-second refusal makes **recovery by idempotency-key replay** the only correct
response to a slow turn (ADR 0005), not polling; a 200 that can carry `status: "failed"` makes
**status parsing load-bearing** rather than optional; the per-user rate limit is shared with the
chat surface, so **serving concurrency is bounded by a budget this repository does not own**; and
a sticky `instructions` field means **persona bytes accumulate in thread history**, which is why
continuation is a policy and not a default.

### Target — **H2**

*H1 needs the loop running on a host, not concurrency.*

Concurrent serving across many tenants, with per-tenant fairness falling out of per-tenant identity
rather than being scheduled (see §13).

### Must not break

- **§D-4 vs §B-10 — the asymmetry is deliberate.** The record store degrades; a security check
  fails closed. An unverified instance means UNKNOWN and UNKNOWN refuses to serve.
- **§B-14 — a thread pointer carries its tenant.** Cross-user `previous_response_id` resume is
  *undefined* upstream, not a 404. Thread isolation is the seam's job and must never rest on the
  runtime.
- **Malformed-answer refusal.** `ironworks_ironclaw::malformed_answer` refuses delivery on control-token
  soup, which was journalled `state: complete` in `0016` run 1. Measured as a cold-start artifact
  (`0018`: 2 of 3 first turns after bring-up, 0 of 47 thereafter).
- **The one path from a key to an answer is `drive`.** A console, an API, or a second channel must
  invoke it, not reimplement it.

### Open

Whether a published or console-initiated turn can ever be synchronous. Today it cannot: concurrency
is not built, `relay` is one cycle, and a turn outlasting 30 seconds returns 503.

---

## 13. Capability: isolation and confinement

**This is an engineering conviction, not a market fact: everything else can be rebuilt, and this
cannot be recovered once it is wrong.** The distinction is worth the sentence. No prospect has
asked for it (operator, 2026-09-02) — and §19.2 records that the one sentence a buyer of isolation
would want, *the operator cannot read your records*, is false by construction under all three
custody shapes, because the Account Service is MultiAgency-operated. So what remains supportable is
that **no other tenant can reach your records**, which every vendor asserts, and that **the turn
journal holds identifiers only**, which is narrower and unusual but is a claim about one store.

Held as a conviction it is still the right thing to build first, for the reason above. Held as a
wedge it is unevidenced, and §19.1 already names the only change that would make it one.

**And the capability the service sells is not the differentiator either.** Upstream ships
`commitment-digest`, `commitment-triage`, `delegation-tracker` and `decision-capture` as
first-party skills at the pin, installable, free, and described in their own words as composing
summaries of open commitments, deadlines and pending signals — which is
`relationship-intelligence@2`'s territory stated plainly. They read through `memory_tree` and
`memory_read`, the `ironclaw.memory.*` surface confinement disables and that §L-2 measured
recovering this tenant's records unprompted. So a prospect who reads both trees finds the
capability shipped by the runtime this product is built on. What upstream does not ship with it is
the tenancy boundary, the tool floor and the evidence trail, and those are what is measured here.
The reading is
`docs/experiments/0051-the-probe-and-the-served-host-resolve-authority-differently.md`, finding 5;
it measured nothing, and it does not need to — the skills are in the pinned tree.

### Today

Isolation is enforced at the identity boundary, and the runtime gives exactly one primitive:

- **§B-1** — a credential resolves to exactly one org, server-side. Any caller-supplied org is
  ignored. `ScopeEvidence` records *how* a scope was established, because a boolean cannot be
  audited.
- **`ContextBinding::new` refuses anything but a record-scope credential** — a runtime member
  credential in that slot is a cross-wiring, and the two must never be the same.
- **§B-13** — measured: another tenant requesting a response id gets 404.
- Confinement is implemented and proven live in `ironworks_lifecycle::confinement`: fresh →
  DRIFTED, applied → VERIFIED, one tool re-enabled → DRIFTED.

**The measured confinement facts are `docs/INVARIANTS.md` §L-1 through §L-11, and they are not
restated here.** They were, as an eight-bullet copy carrying its own row counts — and a count is
the one thing about that catalogue that cannot be copied safely: it varies by *caller* and by
*booted profile* both (`docs/experiments/0039-the-catalogue-has-two-axes.md`), so a bare number
lifted out of §L loses the two qualifiers that make it true. `docs/AUDIT-BRIEF.md` §6 tells an
auditor exactly that, which a second copy here was busy contradicting.

What belongs in this section is what those facts **constrain about the product**, which §L does not
say and should not:

- **Confinement is a per-tenant obligation, not a one-time setup step.** §L-4 and §L-6 together
  mean an applied policy can be reverted by the credential that applied it, so the target is
  continuous reconciliation — see "Confinement is reconciled continuously" below.
- **No capability claim may be stated over the executable surface.** §L-7 scopes the verified claim
  to the settings catalogue, so a requirement written against "every tool the model can reach" is
  unmeasurable as posed and must be rewritten or marked BLOCKED.
- **Approval-off is spent, not configured.** §L-9 makes it one-way per member, which is why every
  proof that writes it mints its own rather than borrowing one.
- **The deny-first ordering is free and stays.** §L-3's partial reachability measurement is not a
  reason to confine less.

Two independent deployment findings once prevented that member-level result from establishing
isolation between two sandbox-backed client control planes. The repository now has a fail-closed
deployment contract for both, but it has not been exercised on a deployment host:

- **Daemon authority is separated in the unit design.** Each `ironclaw-<client>` user manager runs
  its own rootless daemon, with a private home, socket and image/data store
  (`deploy/user/ironworks-docker.service`). The root-owned client environment names that account's
  `/run/user/<uid>/docker.sock`, the IronClaw unit grants no `docker` supplementary group, and the
  startup preflight rejects any other endpoint (`deploy/ironworks-ironclaw@.service`;
  `deploy/check-ironclaw-instance.sh`). It also rejects an OS account that was
  independently added to the shared `docker` group (`deploy/check-ironclaw-instance.sh`).
- **Object-name inputs are explicit and checked.** IronClaw derives the workspace key, container,
  proxy and network names from `(tenant_id, user_id)` alone
  (`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/user_key.rs:24-59`). Both
  identities default to `reborn-cli`
  (`ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/commands/serve.rs:142-151`;
  `ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/runtime/mod.rs:1322-1327`). The client config
  template supplies both values, and the startup preflight requires tenant, default owner,
  authenticated WebUI user and systemd instance to equal the same client slug
  (`deploy/ironclaw-client.config.toml.example`; `deploy/check-ironclaw-instance.sh`).

`dev/prove-deployment-isolation.sh` detects both conditions across two live instances. It creates
same-named positive-control objects in both daemons and exercises each socket as the other service
user. The older workspace and tenancy proofs still do not speak for this boundary. Unique names do
not narrow daemon authority, and separate daemon authority does not excuse default identity.

### Target — **H1**

*This is the capability that gates a second tenant.*

**One sealed, confined runtime member per tenant, and one distinct record-scope credential per
tenant.** This is not a design choice — §B-6 already requires it, and §L-6 makes it the only
arrangement in which confinement is enforceable at all.

Three properties fall out of it rather than needing separate machinery:

1. **Quota is isolated.** The 30 req/60s limit is per user, i.e. per runtime identity. Per-tenant
   members mean one tenant cannot exhaust another's budget. A shared member would make quota
   exhaustion a cross-tenant availability channel, and a fairness scheduler would then be
   compensating for an isolation defect rather than solving a scheduling problem.
2. **Confinement is per-tenant and independently reconcilable.** Drift on one member is one
   tenant's incident.
3. **Thread reachability is bounded by identity**, which §B-13 measures.

**Confinement is reconciled continuously, not applied once.** §L-4 and §L-6 together mean a
confined member is a state that can be left, silently, by anything holding the credential. The
reconciler already reports DRIFTED; at self-serve scale it must run per member on a schedule and
raise, not merely report on demand.

**Per-tenant journal and serving loop.** Today one journal serves the whole process and the safe
cursor is the lowest open update across every tenant — §E-17 of `docs/INVARIANTS.md` is where that
rule is stated and ADR 0006 is why (`crates/channel/src/lib.rs::safe_cursor`). So one tenant's
stuck turn holds the offset for every other tenant, including the operator's own coordination.
**This paragraph is the one statement of that consequence in this document**; the three places
below that gate on it refer back here rather than restating it. The requirement is stated as a property, so the mechanism can move:

> **No tenant's failure is observable by another tenant** — not in latency, not in delivery, not in
> quota, not in whether their turn happens at all.

ADR 0009 chooses a channel-credential-aligned per-tenant serving cell: one tenant's channel stream,
adapter, one-entry registry, journal and one-cycle-at-a-time relay supervisor share a failure domain
with that tenant's runtime. That is the mechanism by which the seam's blast radius equals the
runtime's. It is a decision, not evidence that the mechanism is built.

### Must not break

- **§B-3 — every isolation check needs a positive control.** An empty store passes every cross-org
  test. This is why `a_composition_naming_the_operator_exists_or_the_default_check_is_vacuous` exists
  beside its negative, and any new isolation check inherits the obligation.
- **§B-5 — no ambient tenant.** There is no single-tenant fallback. The removed `SALES_GROUP_ID`
  fallback hand-built a tenant with no guidance-validated persona and silently served a client group
  the internal composition.
- **§B-2 — tenancy is in the key**, `(org_id, <id>)`, so an id collision between orgs creates two
  independent rows rather than re-homing one.
- **§B-11 — registry org id is metadata until authenticated.** Only the authenticated startup
  resolution sets it verified, and a thread may not be loaded before it is.
- **§B-12 — registry validation must work offline.** Every check is local; runtime probes belong at
  startup, not at load.
- **§L-3's ordering invariant** survives automation: applying confinement in catalogue order leaves
  a window in which the work is reversible by the thing being confined.

### Open

**The remaining mechanisms stay open before admitting a second client tenant.** They must not be
compressed into one "multi-tenant" item, because they protect different failure classes. This does
not prohibit a two-instance synthetic deployment used to measure those mechanisms; that test pair
is required evidence, not an admitted tenant.

**Decided, not built — availability.** ADR 0009 settles the mechanism without weakening the Target:
*no tenant's failure is observable by another tenant.* Today's arrangement still satisfies neither
the mechanism nor the property — it has one journal and one shared cursor, per the Target above.
Implementation and measurement remain an availability gate on a second admitted client tenant
(§23, step 5), not on synthetic test fixtures. The decision closes §I's single-poller cap once
deployed; it does not accept that cap as a production risk.

**Open — live deployment isolation proof.** The user unit, configuration preflight and two-client
probe now encode the two independent gates described under Today. No deployment host is reachable
from this repository, so rootless-worker compatibility, cgroup enforcement and cross-socket denial
have not been measured live. Under §F-14 the deployment claim is BLOCKED, not PASS. Testing is not
blocked: two synthetic instances may be provisioned to run
`dev/prove-deployment-isolation.sh` through each serving path. What remains gated is attaching a
second client's credentials, records or channel relay.

**`dev/prove-workspace.sh` is a second, separate BLOCKED and not merely unrun.** It needs a service
whose definition declares `runtime_tools: "sandbox"`, and **no shipped service declares one** —
`relationship-intelligence` is read-only records at `none`, as `account-analysis` was before it was
cut. `ServiceInstance::new` attenuates a turn to that ceiling, so the capability is gone before the
runtime is asked and no catalogue state restores it. The script now checks this before spending a
turn rather than reporting a sandbox failure for a definition reason. Whether the sandbox path
deserves a service is a product decision nobody has taken.

**Closed — the model.** If a proposed design requires sharing a runtime member or a record-scope
credential between tenants, it is refused by §B-6 and there is no discussion to have.

---

## 14. Capability: the service catalog

### Today

One service ships — `relationship-intelligence@2`, tabled with what it composes in
`services/README.md` — and a tenant's registry entry must name it.

Definitions are data, compiled into the binary by `crates/services/build.rs`, which is what lets
`ironworks_services::digest` honestly fingerprint what a binary will actually send. Unknown keys are
refused rather than ignored.

**There is no default.** §7 records the fail-open the `DEFAULT` slot was and why deleting
`account-analysis@1` closed it. What matters here is the mechanism that replaced it: **the slot was
removed rather than refilled**, so a configuration naming no service is refused, `--service` is
required, and a guidance marker omitting `service:` is an error rather than an assertion that the
file meant the default.

### Target — **H1**

*The catalog itself needs nothing for H1. What needed settling has been settled by deletion.*

**Per-tenant selection already works, and now it is mandatory.** A tenant's entry names a service;
the platform decides nothing for them. New services earn their place when a tenant needs a
composition this one does not give them — not to make the set look fuller. Adding a second service
does not create a default; selecting it remains explicit.

Under self-serve, service selection moves into the console and must stay in agreement with the
guidance marker, which is the second half of the binding.

### Must not break

- **The absent slot stays absent** (`crates/services/src/service.rs::naming_no_service_is_refused_rather_than_defaulted`).
  An empty name is unknown rather than a wildcard. A neutral service would not weaken that rule;
  reintroducing a default would be the decision that changes it.
- **A composition written for exactly one organization may name it; nothing that chose nothing may
  reach it.** `relationship-intelligence` names MultiAgency correctly, not by leak.
- **Two things must agree and are deliberately separate**: the instance's configuration selects the
  service, and the tenant's guidance declares it in its marker. A mismatch means one of them is a
  mistake; matching them blindly picks at random.
- **Capabilities are a ceiling attenuated at construction**, and there is no operation that widens
  one. An effect capability cannot even be named in a definition.
- **The model pin is the only source of a model.** There is no `model_policy` field and a service
  cannot choose its own model — `ModelSelection` is constructible only from a pin. Model selection
  decides which third party sees a tenant's records.

### Open

**The service id does not name the job.** `relationship-intelligence` is carried for compatibility
— it is bound into guidance markers and live instance configuration, so renaming it is a migration
rather than an edit (`services/README.md`). Code refers to it as `service::ACCOUNTS`, which means
one id has three spellings: the wire id, the constant, and the persona `COORDINATOR.md`. That is
tolerable at one tenant and is a migration decision the moment there are two.

*Closed since the last revision:* the `service:` optionality in the guidance marker, which was a
live hazard the moment more than one service was in real use. It went with the default slot.

---

## 15. Capability: channels

### Today

`crates/telegram` works — a real bot answered a real question, first attempt, with the
channel confirming the chunk. It serves the same `/poll` and `/send` contract `devchannel` does, so
**the seam cannot tell a real channel from the fixture and nothing on the serving path changes to
reach one.** The `channel` crate is Telegram-shaped in its mechanics — monotonic update ids, offset
acknowledgment, plain text with no parse mode, chunking at `DEFAULT_CHUNK_BYTES`.

Answers arrive markdown-stripped, deliberately (§E-14). The FACT / STATED / UNKNOWN structure that
carries the product's argument reaches the room as flat prose.

### Target — **H2**

*Tenant 2 can arrive on telegram.*

**Telegram and Slack.** Telegram is the working adapter and the segment thesis; Slack is the second,
and it is what proves the contract is a contract rather than one integration with a seam-shaped
wrapper.

The requirement is on the boundary, not on the adapters: **the `/poll` and `/send` contract stays
the only integration point.** An adapter that needs a change on the serving path has found a defect
in the contract.

Slack brings constraints Telegram does not, and they belong to the adapter:

- threading semantics that are not a flat monotonic update stream;
- workspace-level installation and OAuth, where Telegram has a bot token;
- its own message-length and formatting rules under the same markdown-stripped output policy;
- an actor identity of a different shape, which `ActorRef` must accept without becoming richer —
  it authorizes and attributes, and it must still never render.

### Must not break

- **§E-14 — markdown is stripped.** The split is not two products: conversation in the channel,
  anchored claims and journal state and digests on a page.
- **Delivery evidence is per channel and drives recovery.** `Uncertain { acknowledged_chunks: n }`
  means a second copy may appear, and that is a fact a surface must state per turn rather than
  paper over with a paragraph about idempotency.
- **In a group, the registry entry must name the address that summons the bot**, or every message
  in the room costs a turn. An empty address refuses the whole registry.
- **A second channel does not get a second route into serving code.** Every tenant comes from the
  registry, whichever channel they arrive on.

### Open

Whether the per-tenant journal (§13) is per tenant or per tenant-and-channel once a tenant can be
reachable on two.

---

## 16. Capability: the console

### Today

Nothing. `site/` publishes one static page and a 404, generated rather than authored, serving
recorded transcripts labelled with run date and digest. It is read-only over a read-only store and
is a prospect surface, not a tenant surface. The generator **refuses to publish rather than to
build**: `crates/site` recomputes the on-disk book fingerprint, compares it with the one recorded in
each transcript, and on a mismatch exits **3** with `BLOCKED` — the generator worked and the
evidence did not permit publishing. That refusal is the design working, and it is a runtime verdict,
not a compile error. The pins are current (`0021`, `0018`), both fingerprinting
`0f1291ddd6c78fcb`.

### Target — **H2**

*§20 already forbids a console ahead of the capabilities it would invoke.*

**An authenticated, channel-agnostic web console — the self-serve front door.** The console is where
a tenant exists as an organization; the channel is where their team talks. It carries:

1. signup and provisioning status, showing the lifecycle stages honestly including a partial one;
2. guided guidance authoring (§10), emitting the marker mechanically;
3. service selection, kept in agreement with the marker;
4. channel connection for Telegram and Slack;
5. their own journal view — ids, states and digests, and no content;
6. deprovisioning and export (§17).

### Must not break

- **§ FRONTEND — it must not join ids back to content.** The journal holds identifiers only. A page
  that fetches the text to be helpful reintroduces exactly what the store was designed not to hold.
  Rendering that restraint *is* the privacy proof.
- **No scores and no leaderboard.** Urgency ordering is valid coordination behaviour; value ranking
  is what the service forbids. A sortable value column would violate a service invariant through
  pure UI affordance.
- **Record text renders as quoted evidence, never in the product's voice.** Activity bodies are
  attacker-reachable — no HTML, no live links.
- **If the console acts, it acts by invoking the same commands.** There is exactly one path from a
  key to an answer, and a rendered surface must not become the second, looser one.
- **A button the seam cannot honour teaches a tenant that the surface overstates.** Every control
  ships behind the capability it invokes, not ahead of it.

### Open

Whether the prospect page and the console are one artifact or two. They have different readers and
different authentication, and today's page is generated from experiment transcripts rather than
served from live state — folding them would make the generated page a runtime surface.

---

## 17. Capability: deprovisioning

### Today

`ironworks tenant export` now writes an owner-only, complete, scoped `ironworks-book-v1` artifact
and digest manifest without printing record content. There is deliberately no deprovision command:
the attempted implementation created a second JSON ledger beside lifecycle state and encoded
residual authority as prose, which could not satisfy §F-4 through §F-14. The Account Service
administration response types remain available for contract work, but no destructive orchestration
is exposed until the real residual-authority ledger and external cross-org controls exist. There
is no accepted-risk verdict ahead of that ledger: a caller-supplied reason would not establish that
an operator recorded a classification. **The ledger itself does not exist here.**

**§F-4 and §F-5 are the shape this inherits:** deleting a member does not revoke its token, so a
deprovisioned tenant leaves an authenticating session for up to 365 days, and the residual ledger
records that remainder in four fields with no token material. *Deprovisioning is a process with a
remainder, not a state.*

**Under platform custody there is a second remainder, and those four fields cannot express it.**
They describe a *runtime member*, which expires. A deprovisioned tenant also leaves an org full of
their business records in a store MultiAgency operates and holds the credential for — **no ledger
row, no expiry, and no measured end.** §F-4's framing was written about a 365-day session; under the
custody shape §19.1 settles on it is true of the records indefinitely, which is a strictly worse
remainder than the one this model was built for. Export (Open, below) is what bounds it.

### Target — **H2**

*Obligated by §19.1's custody decision, but it gates no tenant.*

Tenant-initiated deprovisioning that completes the parts that can complete, records the remainder,
and tells the tenant the truth about it — including the 365-day figure.

At self-serve scale churn is continuous, so the residual ledger is **a standing operational load
rather than an edge case**: a growing set of `ACTIVE_RISK` entries, each blocking promotion, each
requiring either a measured revocation probe or an operator classification with a recorded reason.

### Must not break

**§F-6, §F-7, §F-8, §F-9, §F-10, §F-11, §F-12 and §F-14**, each stated once in
`docs/INVARIANTS.md` and deliberately not restated here — they were, and a copy of an invariant is
a second thing to keep in step with it.

What this section owns is why *these* eight: **every one of them is a way a gate reports green
without the thing it gates being true.** A residual session's validity confused with its meaning;
`REVOKED` asserted rather than measured; a waiver with no recorded reason; a classification lost on
re-record; an expiry recomputed until it stops describing the token; a listing read by `grep`
instead of by exit code; a check that could not run counted as a pass. That is the shape of every
defect this capability has produced so far, which is why the design below **records** rather than
asserts, and why the ledger is the artefact rather than the gate's answer.

### Open

Export now provides the faithful copy platform custody requires. What remains undesigned is what a
tenant is owed beyond it: whether deletion is offered, how deletion is measured rather than
asserted (§F-7's lesson), and what the ledger row for a *records* remainder would hold.

---

## 18. Capability: operator observability

### Today

`ironworks status` reports the turn journal, read-only, opening no runtime, record store, guidance
or channel. `crates/reconcile` translates lifecycle intent into observed authority and orders
findings by the verdict ranking `CONTEXT.md` defines and `DESIGN.md`'s "Verdicts" section explains:
the operator sees the sharper fact first, and an inspection either measured something or reports
why it could not.

Where `lifecycle` and `reconcile` sit relative to the serving path is `DESIGN.md`'s "Layering",
which is the copy Cargo and the control both check.

### Target — **H2**

*The closest to done of any capability.*

The same discipline across many tenants: what authority exists per tenant, what we recorded
creating, what drifted, and what is outstanding — with the aggregate never being a number that
hides a `Fail` inside a `Blocked`.

### Must not break

- **The three lifecycle concepts stay apart.** `InstanceSpec`, `ProvisionProgress` and
  `AuthorityInventory` answer different questions and live in different places. Merging any two of
  them is how the reference's worst crash windows became invisible. A stage named `org_registered`
  reads as an assertion about the world; it is a note about ourselves.
- **No accepted-risk result exists before its ledger.** Otherwise a caller could turn unmanaged
  authority green with an unrecorded sentence.
- **The stores hold no content.** A reader who cannot see a single sentence of a conversation can
  still learn the whole client list from a world-readable file, which is why custody is stated once
  in `domain` rather than three times as a filesystem chore.

**The five things a lifecycle operation must never do.** Provisioning (§9), suspension and
deprovisioning (§17) each own part of this; observability is where a violation becomes visible, so
the list lives here. Each maps to an invariant that already exists —
the mapping is the point, because a prohibition with no mechanism is a wish.

| A lifecycle operation must never | Caught by |
|---|---|
| silently create duplicate authority | §B-6's uniqueness refusals — a shared member token means two tenants *are* the same identity |
| revoke authority it did not create | §F-7 — `REVOKED` is set only by a probe that measured a rejection, never asserted by hand |
| leave ambiguous routing active | §B-4, the registry refuses as a whole; `route::resolve` returns `Ambiguous` rather than picking |
| lose durable delivery state | `DeliveryState` is a join-semilattice — an attempt can only sharpen what is known, never lower it |
| expose one tenant's state to another | §B-2, tenancy is in the key; §B-13, measured — another tenant asking for a response id gets 404 |

**Deprovisioning is a process with a remainder, not a state** (§17), so the second row is the one
that bites: a lifecycle that reports completion it did not measure is worse than one that reports
`BLOCKED`, because §F-14 exists to keep a check that could not run from ever reading as a PASS.

### Open

Nothing pressing. This capability is the closest to done of any in the document.

---

## 19. Two decisions and what they obligate

Both were raised, costed, and decided rather than defaulted. Record origin is settled in §11; the
custody decision below is downstream of it, and this section records the obligations that follow.

### 19.1 Custody

**Constraint, not preference.** §L-6 means a tenant must never hold their own runtime member
credential — holding it lets them re-enable every **ordinary** tool denial confinement applied; the
two locked `operator_config_set_*` rows are the measured exception. That half is settled.

**Decided: platform custody.** The platform mints and holds both credentials per tenant, and
**MultiAgency is a custodian of tenant records and must say so.**

Three shapes were costed. What settles it is that only one of them keeps provisioning a single
unattended transaction, which is what self-serve means.

**Split custody does not add a provisioning stage — it changes what one means.** `Stage::OrgRegistered`
is *"an Account-Service org credential exists and resolves"*, and `Authority::created_at` records
*"which stage's completion implies **IronWorks created this**."* Under split custody IronWorks would
*verify* an org it did not create, so completion would assert authorship it does not have — the
merge of intent with observation `crates/lifecycle` exists to prevent (§18). Concretely,
`Authority::OrgRegistration` leaves `Authority::ALL` and the constant goes **5 → 4**, with org
registration moving to `AuthorityInventory` as observed-only. Two further stages move with it:
`data_initialization_settled` disappears, because IronWorks holds no credential that can write to
the tenant's org; and `smoke_passed` gains a failure mode IronWorks cannot fix, because it now
depends on a credential the tenant can revoke mid-provision. Preflight gains a blocking human step,
so provisioning becomes two transactions with a person in between.

**Split custody is also not the privacy boundary it looks like.** The Account Service is
MultiAgency-operated — `deploy/README.md` records that the one live operator turn *"read its records
from that local service"*, part of the Python fleet. A tenant holding the record-scope credential
holds an API token to a store MultiAgency runs. That is a **control** boundary, not a
**confidentiality** one, and §19.2 now says so.

**Custody + revocation changes no stage at all**, which is the argument for choosing platform
custody now rather than a reason to call them the same thing. It is the destination, reachable
later without re-provisioning anyone — but today it claims exactly what platform custody claims,
because §17's export and a *measured* revocation probe (§F-7: `REVOKED` is set only by a probe that
measured a rejection — the platform revoking a credential it minted and holds is otherwise an
honour system) are both unbuilt.

**What this obliges.** Export stops being a courtesy (§17, Open): it is what bounds the records
remainder platform custody creates, and it is named in §23's trailing paragraph.

**Per-tenant variation is not free, and §11 should be read with this.** §11 records that a tenant
arriving with their own org is not excluded. That remains true of *record origin*; it is not a
licence to run split custody for one tenant and platform custody for another. `Authority::ALL` is a
compile-time constant, so a per-tenant split would make `created_at` return different answers for
different tenants — one lifecycle model meaning two things depending on who is asking, which is the
same merge as above wearing a configuration flag. A second custody shape is a second provisioning
path, and it must be decided as one rather than arrived at per tenant.

The three shapes as costed, in descending order of how much they ask of the tenant:

| | Tenant holds | Consequence | Provisioning |
|---|---|---|---|
| **Platform custody** *(chosen)* | nothing | Frictionless signup. MultiAgency is a data custodian and must say so | one unattended transaction; all seven stages keep their meaning |
| Split custody | record scope | A tenant can revoke unilaterally. Costs frictionless signup — they need an org, and under §11's record-origin decision their book still starts empty | two transactions with a human between; `Authority::ALL` 5 → 4 |
| Custody + revocation | nothing, but can export and revoke | Platform custody plus export and a measured revocation probe, neither built | identical to platform custody |

**What no shape buys.** None of the three supports *"the operator cannot read your records."* That
sentence is false by construction under all of them, because the store is MultiAgency-operated —
custody of a *credential* is not custody of the *data*. Split custody would have moved where it
becomes false, not made it true. §19.2 records this directly rather than leaving it to be inferred.

**Reopened by** the Account Service ceasing to be MultiAgency-operated — third-party or
tenant-hosted — at which point split custody becomes a real confidentiality boundary and its
provisioning cost is worth paying. Also by a tenant whose regulator forbids their records sitting in
a vendor-operated store: that tenant cannot be served under this shape at any price.

### 19.2 The privacy claim

Downstream of custody, which §19.1 has now settled as platform custody.

What is supportable *today*: **no other tenant can reach a tenant's records** (§B-1, §B-6, §B-13,
measured); **the turn journal holds identifiers only** — never message text, answer text, records or
credentials.

What is **not** supportable today, and must not be claimed:

- **"The operator cannot read your records."** False by construction, **and not because of the
  custody shape chosen** — it is false under all three. The Account Service is MultiAgency-operated:
  `deploy/README.md` records that the one live operator turn *"read its records from that local
  service,"* part of the Python fleet. Custody of a *credential* is not custody of the *data*, so
  split custody would have relocated the falsehood rather than removed it. **Nothing in §19.1 was
  held in reserve here.** Only a store MultiAgency does not operate changes this sentence.
- **"We store no conversation content."** True of the turn journal and false of the system. Two
  stores hold a person's words: `crates/proposals` keeps the exact bytes of a proposal — `body TEXT NOT
  NULL`, immutable after creation, on the operator's disk — and a promoted note becomes an activity
  in the Account Service, which is conversation content in a store we operate. **The claim is only
  ever "the turn journal holds identifiers only,"** and it must be scoped to that store by name.
- **Anything resting on the model pin as an audited fact.** `model` is echoed on create but not on
  retrieval, so which model produced a stored answer is unverifiable after the fact, and whether the
  pin is honoured at all is a property of the provider. **The pin fails silently rather than
  closed**, and that is structural.
- **The TEE claim itself, until `MODEL_PIN`'s own OPEN item is resolved.** The catalogue reports
  `is_ready: false` for every TEE-attested model including this one. That must be resolved before
  making a privacy claim to anyone outside this repository. It is written down in `MODEL_PIN` and it
  has not moved.
- **Anything describing the whole confinement policy as locked.** §L-6: ordinary settings remain
  member-reversible; the two `operator_config_set_*` rows are the measured locked exception.
- **Completeness of confinement over the executable surface.** §L-7: **BLOCKED**. The measured
  settings catalogue and captured capability surface differ, and exhaustive fresh-member deferred
  reachability could not be completed.
- **"Only we can do this."** The capability is upstream and first-party: `commitment-digest`,
  `commitment-triage`, `delegation-tracker` and `decision-capture` ship in the pinned tree's
  `skills/`, free and installable, and one of them describes itself as composing summaries of open
  commitments and deadlines (§13). A demonstration that leads with what the service *does* is
  answered by the runtime it runs on. What is supportable is narrower and is the same list this
  section already holds: the tenancy boundary, the tool floor and the evidence trail — each scoped
  to what was actually measured.

---

## 20. Out of scope

Named so they are decisions rather than omissions.

- **Cross-org visibility of any kind.** §19's vendor model settles this; a sharing primitive is a
  different product with different invariants.
- **Effect execution.** `Access` has no `Write` variant, and the model never acts. The record
  contribution path in §11 is a person's effect, not the model's.
- **A guidance text editor.** §10 replaces it with guided authoring; a free-form editor over the
  bytes the marker binds is a defect, not a feature.
- **Model choice as a tenant-facing option.** The pin is the only source of a model.
- **A fairness scheduler.** Per-tenant runtime identity isolates quota at the runtime; a scheduler
  over shared identity would compensate for an isolation defect (§13).
- **Provisioning UI ahead of provisioning.** Every console control ships behind the capability it
  invokes.
- **An IronClaw fork, a patch, or a competing runtime.** This is the one rule (`README.md`): a
  change that would require editing IronClaw belongs upstream. A fork would also void §2 — the
  boundary table is only meaningful while the left column is somebody else's code.
- **A generic workflow or graph orchestration framework.** §A-3: the seam supplies context, it does
  not reason. Context selection is deterministic prefetch only, and a second agent loop beside
  IronClaw's is an unbounded authority nobody specified.
- **A generic CRM or knowledge graph.** §6's complexity rule settles it. The data model is four
  entities because four is what the service needs; a fifth arrives when a concrete requirement
  cannot be represented in the existing four.
- **A self-service agent marketplace.** The catalog is open and per-tenant selection works (§14),
  but a service earns its place because a tenant needs a composition neither existing one gives
  them — not to make the set look fuller. A marketplace inverts that.
- **A replacement for enterprise identity infrastructure.** IronWorks resolves a credential to an
  org and binds an actor to a contributor. It is not an IdP, and `ContributorId` is deliberately
  *not* a runtime member precisely because nothing here can prove which runtime account an actor
  paired to.

---

## 21. Success criteria

Eight statements that would mean the thing works. Each carries what it would take to hold it, and
what holds today. **Met** here means *measured*, never *implemented* — the distinction §8 draws
between Today and Target is the same distinction, and this table is where it is easiest to lose.

| Criterion | State |
|---|---|
| a real private tenant operates end to end | **partial, and the remaining gap is duration rather than machinery.** One tenant answers over a real channel from a supervised loop: `multi-serve` runs `ironworks-relay` and `ironworks-telegram` for `relationship-demo`, which polled a real room, resolved scope server-side, composed over its records, answered from a confined member and delivered, leaving no unresolved turn. What is still missing is a tenant *operating* — days of traffic, not one turn. `dev/prove-host.sh` now verifies that host's units, live confinement and registry digest, but it observes no turn: the account of what was polled, composed, answered and delivered is still reported rather than checked |
| tenant and audience scope are selected deterministically | **met** — §B-1 resolves org server-side; `route::resolve` is three-valued and refuses to resolve an ambiguity by picking |
| only authorized context reaches the agent | **met** — `Ask` has no `org` field and no field for rendered text; `compile_fail` doctests hold that an unverified instance cannot build a turn at all |
| secrets remain outside model-visible context | **met** — `CredentialRef` holds where a credential lives, never its value; the journal holds identifiers only |
| reasoning over real business records stays grounded | **not established, and the word that blocks it is "real."** On the book that exists, this is measured. `0026` read all fifty-nine recorded answers at one composition over one ten-record book: thirty-seven pass, and of the twenty-two that do not, **seven are the model, fourteen the instrument, one is ambiguous**. `0028` held out a sample and scored **9/10 against 10/10 in training**. Four stability probes (`0022`, `0023`, beside `injection` 4/10 in `0019`) put no case on a stable verdict, and `0021` scored a composition that never moved three different ways. Every grader defect found has been corrected — `0024`–`0027`, `0029`, `0030`, `0034`, `0038` — each with two-sided controls, and **none is open**; `0038` is the most recent and found its defect by measuring every rule's firing rate over 227 answers rather than by reading answers, which is why it found a rule that had never graded anything. **What is not established is the criterion's own word: the book is ten invented accounts, so nothing here measures transfer to a tenant's own records, and no further reading of this corpus can.** That is a gate a real tenant moves and a grader pass does not |
| delivery survives expected failures and restarts | **met** — B1–B7 proven across real process restarts, asserting byte-identity of duplicates rather than counts |
| no cross-tenant leakage occurs | **met at the boundaries measured** — §B-13 measured; `dev/prove-tenancy.sh` runs seven checks. §B-3 is the caveat that keeps this honest: an empty store passes every cross-org test, so each check carries a positive control |
| human business authority remains intact | **met** — `Access` has no `Write` variant, and no operation widens a capability |

Two of the eight are the ones to watch, and they fail differently. **The first is no longer blocked
on anything anyone has to build.** The supervision runs on `multi-serve` and has served, so what
separates *partial* from *met* is a tenant using it over time — which arrives by waiting and
watching rather than by work. The honest caveat is that nothing here can see that host: every claim
about it is reported, and the first automated check that reaches it would be worth more than another
turn.

**The fifth is blocked by the corpus, and this used to be written the other way round.** It said
the gate was the instrument — that the grader was not yet something a verdict could be quoted from,
"not because a defect is open but because the rate at which reading finds them is not the rate at
which they exist." That sentence cannot be discharged by any amount of reading, which makes it a
gate nothing satisfies rather than a criterion. It also named `0030` and `0034` as the most recent
corrections after `0038` had already landed, which is what a claim does when nothing re-measures it.

The instrument is a **standing obligation**, not the gate. The argument that another defect exists
is still good and still specific — `absence_as_negative` alone accounted for six, the next turned up
in `ownership_default` the moment anyone read the answers that *passed*, and `0038` found a third
kind by asking which rules had never graded anything. Every search in a direction nobody had tried
has returned something. That is a reason to keep searching whenever the instrument is touched; it is
not a reason to withhold what has been measured.

**What actually gates the criterion is its own first word.** The corpus is one synthetic book, drawn
partly from real coordination patterns and partly invented (operator, 2026-09-02). No reading of
invented accounts establishes grounding on real ones, so this moves when a tenant's own records go
through the service and not before — which makes it a criterion §23's first track reaches, not its
second.

---

## 22. The negative space

Published here for the same reason `README.md` and `docs/FRONTEND.md` publish theirs: a buyer who
hands over their relationship record selects for vendors who state limits unprompted, and an
internal document that flatters itself is worse than no document.

What is not built, stated as of the tree this document sits in:

- **There is no end-to-end provisioning or deprovisioning command.** Adoption, confinement,
  strict registry publication and complete export are separate, narrow operator operations.
  External org/member creation, measured activation, destructive Account Service qualification
  and the residual-authority ledger remain open.
- **The serving loop runs on one host, and most of what this repository says about it is now
  measured.** `multi-serve` runs `ironworks-relay` and `ironworks-telegram` for one tenant, which
  has answered and delivered. What remains *un*measured is the limit worth publishing: the org's
  account count and the channel are still operator-reported, no automated check reaches them, and
  every further run of `dev/prove-host.sh` is an operator decision taken in the moment rather than
  a thing that happens. The proof's score and what it did not establish are in `DESIGN.md`'s
  "Status" block, which is where a current figure lives; quoting it a second time here would be a
  number nothing derives.
- **Two tenants exist and neither has been used in anger.** The operator's own org has answered one
  real question about real records from the workstation; `relationship-demo` has answered on the
  host. One turn each is not a tenant operating.
- **No number from the evaluation suite is a pass rate, and the instrument is most of what the
  probes measured.** Runs at a composition that never moved produced different scores; every case
  has both passed and failed; most recorded failures turned out to be the grader rather than the
  model. `docs/experiments/README.md` carries the arc in full, one line per measurement, and is the
  only place the counts and the open-defect status are stated — they have moved before and a copy
  here would be the thing that lags. §21's fifth criterion is where this bears on the product.

  Two limits are what this section owns, and neither is a number. **Every direction anyone has
  searched has returned a defect** — one rule accounted for six, the next turned up the moment
  anyone looked the other way, and the one after that came from a third kind of search — so assume
  the next direction does too, and treat "no defect is currently open" as a statement about where
  we have looked. And **all of it describes one synthetic book at one composition**: what it cannot
  describe is a real tenant's records, a limit no further reading of this corpus removes.
- **`writes` is `Access::None` everywhere**, and the contribution path in §11 is built — grammar,
  proposal buffer, `/confirm` and the single append route. **What does not exist is any measurement
  that a room's notes accumulate into a usable book**, which is the claim §11's Target actually
  makes.
- **Confinement completeness over the executable surface is BLOCKED** (§L-7), and `0032` widened the
  gap rather than closing it: resolving a parked tool call reaches `builtin__result_read`, which is
  **outside the catalogue confinement governs**.
- **`completed` no longer means answered.** `0032` measured a parked run reporting `completed`, so
  the status enum's one advancing variant is not by itself evidence of an answer.
- **Every reading of the graders so far is one person's.** `0026` and `0034` searched in opposite
  directions and both were read, judged and validated by the same reader. That is a limit on the
  readings, not on the instrument: the defects are mechanical, reproducible from the transcripts,
  and each is pinned by controls that run.
- **The TEE readiness flag is unresolved** (`MODEL_PIN`, OPEN).
- **The whole-registry-refuses design does not scale** and its replacement is not designed (§9).

Local figures hold — `cargo test --workspace` and `cargo clippy --all-targets` — and **the numbers
belong in `DESIGN.md`'s status block, not here.** Nothing gates a figure in either file: the status
block asserts no test total any more, precisely so there is no number to keep in step, and the
alloy digest, the envelope size and the per-script check counts are left unverified on purpose. So
a figure written *here* is true exactly as long as someone is watching. Twice it was not. Two mechanisms for one invariant is one mechanism and one thing to keep in sync
(`DESIGN.md`), and the same is true of a measurement: re-measure, or read the one place that is
checked.

---

## 23. What to build first

Not a schedule. This is the ordering for admitting a second client, not an authority boundary
around fixtures: synthetic identities, runtimes and relays may be created earlier when they are the
instrument needed to test a later step. A test deployment carries no client credentials or records
and makes no production claim merely by existing.

**The list is not a chain.** An earlier version of this section said each item was blocked by the
one above it, and that is false for five of the six adjacent pairs: `crates/cli` does not depend
on `ironworks-graders`, so the serving and grader stacks are disjoint, and steps 1, 3 and 4 share no
artifact. What is real is one gate — step 5 before admitting a second client — plus two safety
orderings that §23 already states in the steps themselves.

**Two tracks, not one queue.** Steps 1, 3, 4 and 5 gate *admitting a second client*. Step 2 gates
*publishing an answer-quality claim*, which is a different milestone reached on its own schedule.
Reading them as one list is what makes answer quality look like it blocks deployment.

**This is a plan, not a hypothetical.** A second tenant is named and in view (operator,
2026-09-02), which is what makes the ordering below worth getting right rather than worth
shortening. It carries no date, so it is an ordering and not a schedule.

**The milestone these steps reach is a second client tenant, operator-provisioned** — not
self-serve. “Second tenant” in this ordered list means an admitted client with real credentials,
records and a channel relay. It does not mean a disposable synthetic identity or runtime created to
test the deployment; step 4 necessarily creates two of those before its proof can run. Self-serve is
the milestone after, and §3's horizon is the right long-range target rather than this list's. Naming
the nearer one matters because signup, the console and guided authoring sit in the trailing paragraph
below, and a reader who takes §3's horizon as this list's destination will expect them here.

1. **Run the serving loop for one client and no more.** The supervision is
   `deploy/ironworks-relay@.service` over `dev/relay-loop.sh`, and `deploy/README.md` owns the
   procedure. The constraint that makes this a step rather than a detail: **the cursor is shared
   across every admitted tenant** (§13, step 5), so one client relay is safe and two are not.
   Synthetic relays used to measure step 5 are not prohibited.
2. **Decide what the page publishes.** *Parallel — this gates a claim, not a tenant.* The grader
   work this step used to ask for is done: `0024` settled the port, `0025`, `0027` and `0029`
   corrected the four rule defects and the ordering defect, `0026` read all fifty-nine recorded
   answers and separated the seven the model owns from the fourteen the instrument did, and `0028`
   held out a sample that scored 9/10 against 10/10 in training. `0030`, `0034` and `0038` closed the
   last three named defects, `0038` by measuring firing rates rather than reading answers.
   `stored_answers` keeps the offline half rigorous by refusing to let a correction move a recorded
   verdict silently, and `0016` remains the precedent for why the arithmetic cannot be redone in
   prose. `0034` also removed the reason to defer this on grounds of bias: the upward-only asymmetry
   was a one-directional search, and reading the other direction found a leniency defect.

   **The one judgement left is settled by §22 rather than by another measurement.** What the page can
   honestly show is the spread and the defect history together — the instability, the split between
   model and instrument, the held-out result, and the list of corrections with none open. A single
   clean figure would be the less credible page, not the more: §22 publishes the negative space
   because a buyer handing over their relationship record selects for vendors who state limits
   unprompted. The page is therefore reachable now, and what it must not do is mint a pass rate,
   which is the one thing `docs/experiments/README.md` says no number here is.
3. ~~**Resolve the operator-only binding deferral**~~ (`docs/ACCOUNT-COORDINATION.md`). **Done**,
   and by a route this step did not anticipate. It was to end before one of two production events;
   it ended instead because ADR 0010 settled that the service varies per client, which retires the
   premise that a shipped composition is always the operator's own. It landed as `org` on a
   registry entry: the records credential must resolve to that organization or `boot_tenant`
   refuses service. **Enforced, not measured:** nothing has provoked the refusal on a live path.
4. **Provision a synthetic pair and measure Docker isolation (§13).** *Sizeable now that the host is
   reachable (operator, 2026-09-02) — this was the one item that could only be sized as unknown.*
   The repository now provisions separate rootless daemons and rejects default/mismatched
   identities. On the deployment host,
   `dev/prove-deployment-isolation.sh` must still show that each control plane lacks authority over
   the other's Docker resources and that the same object name can exist independently in both
   daemons; `dev/prove-workspace.sh` must pass through each client to prove the rootless daemon still
   supports IronClaw's real worker/proxy path. A feature-gated qualification binary now carries a
   synthetic sandbox service and bound guidance for that proof; the production binary and service
   catalogue reject it. These are independent requirements; unique naming
   does not narrow daemon authority, and daemon separation does not excuse duplicate names within
   one daemon. Creating the synthetic pair is part of this step; admitting the second client is what
   must not happen before the live proofs pass.
5. **Qualify the per-tenant journal and loop (§13).** The templated relay and Telegram units now
   give each serving cell a strict one-entry registry, channel endpoint, journal and proposal
   store. This remains the availability gate on the second admitted client: synthetic tests must
   still produce PASS showing that one cell's stopped adapter, blocked turn, rate limit and open
   cursor do not change the healthy cell's polling or delivery. **Admitting a second client tenant
   must not land before that PASS.**
6. ~~**Settle the `DEFAULT` slot (§14, §7).**~~ **Done, 2026-09-02, by deletion.** Kept rather than
   deleted because it is the one item here that only ever existed because a second tenant does —
   and what re-opens it is a second *service*, not a second tenant.
7. **Execute operator provisioning (§9)**, under the platform-custody decision recorded in §19.1.
   The narrow adoption, confinement and publication operations exist; creation of external
   authority, resumable orchestration and the real smoke/activation path do not.
8. ~~**Redesign registry refusal for scale (§9, Open).**~~ **Mechanism closed by cell topology.**
   Production uses the unchanged registry parser with exactly one entry per cell, so one malformed
   tenant refuses only that staged cell without weakening whole-registry fail-closed behaviour.

Everything else — the console, Slack, guided authoring — is buildable in
parallel, and the contribution path is already built. None of it is *gated* by the steps above; what
the steps gate is admitting a client onto it.

**Export belongs in that parallel set, and it is not optional.** Its seam-side command and strict
response validation now exist, but the external Account Service contract is not yet qualified. No tenant has asked for it
(operator, 2026-09-02) — it is an obligation this project took on rather than a requirement it was
given, which is exactly why it sits here and not in a numbered step. It does not block a second
tenant; §19.1's platform-custody decision is what created it.
Without export a deprovisioned tenant's records stay in a store they hold no credential to, with no
ledger row and no expiry (§17), and *"their records are theirs"* is a sentence with no mechanism
behind it. Building it also builds most of what an import would consume, which is the quieter
reopener §11 names.
