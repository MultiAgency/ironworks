# Service definitions

A **service definition** is the reusable half of a service, held as data: which persona parts
melt into the alloy, in what order, under which guidance heading, closed by which safety tail.
A tenant is the other half. Two tenants on the same service differ only in their records and
their guidance — there is no branch anywhere on *who* an instance belongs to.

Terms here are the ones in [`../CONTEXT.md`](../CONTEXT.md), which is opinionated on purpose.
*Alloy*, *melt*, *scrap* and *guidance* all mean something narrower than their ordinary reading.

Everything in this directory is compiled into the binary by `crates/services/build.rs`, alongside
the `scrap/` tree it names. Nothing is read from disk at run time, which is what lets
`ironworks_services::digest` honestly fingerprint what a binary will actually send.

## The one that ships

| service | composes | who runs it | evaluation |
|---|---|---|---|
| `relationship-intelligence@2` | `personas/COORDINATOR.md` + `skills/relationship-record/` | MultiAgency, coordinating its own accounts | `fixtures/account-coordination`, graded by `crates/graders` |

`relationship-intelligence` is the stable machine id for MultiAgency's product-facing **account
coordination** service. It reads an account's dated record in time order and derives what is
currently true across onboarding, delivery, renewal and offboarding — what is committed,
outstanding, superseded, contradictory, stale, or needs a person. It does not act; it is
read-only, and the frozen capability shape is machine-checked.

**Do not add a service to make the set look fuller.** A new definition earns its place when a
tenant needs a composition this one does not give them.

A second definition, `account-analysis@1`, shipped until 2026-09-02 and was deleted with the
default slot it existed to hold. `docs/PRD.md` §7 records why, and the rule it left behind.

## There is no default service

`ironworks_services::service::DEFAULT` is gone. A configuration that names no service is **refused**,
not filled in: `cli` requires `--service`, `crates/domain`'s registry already required the field,
and a guidance marker that omits `service:` is now an error rather than an assertion that the file
meant whichever service was default. The value that fills a silence is the value nobody chose.

`crates/services/src/service.rs::naming_no_service_is_refused_rather_than_defaulted` holds the
boundary: the empty name is not a wildcard; it is unknown like any other. Whether a future service
is organization-specific or neutral does not weaken that rule — neither makes silence a choice.

## Why account coordination keeps the `relationship-intelligence` id

The v2 composition names the operator-facing job more plainly: account coordination. The service
id remains `relationship-intelligence` because it is already bound into guidance markers and live
instance configuration. Renaming that authority-bearing identifier would create migration work
without changing the dogfooded capability. A later rename needs an explicit migration decision;
it is not part of proving the behavior.

## Why this is a service and not a skill

The runtime ships the same capability. `skills/` in the pinned tree carries `commitment-digest`,
`commitment-triage`, `delegation-tracker` and `decision-capture` as first-party skills, and the
first of those composes summaries of open commitments, deadlines and pending signals — account
coordination, described in upstream's own words. Anyone can install them.

**A skill is prompt text the runtime selects; a service definition is the thing the seam refuses
around.** A skill has an activation block, a token budget and a trust level, and none of those is
an authority boundary — an installed skill is excluded from model-selected activation and is
otherwise the same instructions with the same tool access as the person who placed it. What a
service definition carries and a skill cannot is on the other side of that line: the org binding
the registry refuses without, the capability profile, the fixed composition whose digest a turn
records, and the confinement floor written as the member before any of it runs. The two are not
competing formats. One is what the model reads; the other is what decides whether a model reads
anything at all.

The consequence is a positioning rule rather than a design one, and it is written where a claim
gets made: `docs/PRD.md` §13 and §19.2. **The capability is not the differentiator** — the boundary
it runs inside is, and it is the half that is measured.

## Why `relationship-intelligence` names MultiAgency

`personas/COORDINATOR.md` says "You are **MultiAgency's account-coordination analyst**" in its
first sentence, and names the operator throughout; the skill splits outstanding commitments into
what waits on MultiAgency and what waits on the counterparty.

**Those references are correct, not leaked.** This is the composition MultiAgency runs on its own
relationship record — `CONTEXT.md` records that we run as a tenant of our own system, on the same
path a tenant that is not us uses, with no privilege. A composition written for exactly one
organization may name it. What it must never do is be reached by a configuration that chose
nothing, because that lands somebody else on a persona addressed to us — and since there is no
longer anything to reach it *by* default, that is now a refusal rather than a slot policy.

This has been mistaken for a defect more than once — read cold, six hardcoded organization names
in a shared corpus look exactly like tenant identity that escaped. They are reviewed content, not
runtime authority. The tenant registry's `org` binding is the separate machine control that stops
a records credential for another organization from being served.

## The fields

Every key is required except `evaluation`, and unknown keys are refused rather than ignored —
definitions are compiled in, so a key that does not parse is a typo, never a forward-compatible
addition from someone else's version. Contrast `crates/ironclaw`, where the rule is inverted.

| key | what it does |
|---|---|
| `service` | the id, which must equal the filename; a disagreement refuses to load |
| `version` | a positive integer, carried into `<service>@<version>` for operator-facing identity. Metadata that marks a composition change a human should notice — there is no rollout, migration, or coexistence machinery behind it |
| `responsibility` | what the service is accountable for, in one sentence. Non-empty is enforced |
| `composition.parts` | repo-relative scrap paths, melted in this order. Validated at load, so a renamed part fails at startup rather than in front of a tenant |
| `composition.guidance_heading` | the model-visible section title the tenant's guidance is injected under |
| `composition.safety_tail` | melted last, always. One tail across every service, and a test asserts it |
| `capabilities` | the ceiling an instance is attenuated to at construction (`ServiceInstance::new`). Not a description: `Access` has no `Write` variant, so an effect capability cannot even be named here |
| `evaluation` | the suite that measures whether this service's answers are any good, or `null` meaning none does. A claim about coverage, not a hook — `crates/eval` is the driver |

## Deliberately absent

`crates/domain/src/service.rs` carries the reasoning; the short form is that a field with one legal
value is noise, and a field nothing branches on is a comment. So there is no `model_policy` (the
pin is the only source of a model), no `title`/`summary` pair (one `responsibility`, because the
word implies a boundary where "summary" invites marketing copy), and no `audience`.

`audience` is the one worth knowing about: upstream it fed an assertion that the default service
was the external one. With no default there is no split left to assert. There is also no local
`addresses` metadata: duplicating an organization name from the prose would only prove that the two
strings agree, not that either one matches the server-resolved records authority. The registry's
`org` binding is the control for that question.

## Binding a service to an instance

Two things must agree, and they are deliberately separate:

- the instance's configuration selects the service — `--service <name>` on `compose`, `turn` or
  `redeliver`, required, with
  nothing to fall back to;
- the tenant's guidance declares it in its first-line marker:
  `<!-- tenant-guidance v1 slug: <slug> service: <service> -->`.

`Guidance::parse` is a control and fails closed on all five counts: no marker, a marker bound to
another slug, a marker naming no service, a marker bound to another service, or content too thin
to be real. The error says to fix whichever is wrong rather than to make them match, because a
mismatch means one of them is a mistake and matching them blindly picks at random.

**Reading the configured service back into an omitted `service:`** would leave the binding check
unable to fail, which is the one property a control may not have. That is why a silent marker is
refused rather than filled in.
