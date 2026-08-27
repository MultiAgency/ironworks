# Product direction

This document owns product intent and non-goals. It does not claim that a feature is deployed or
that an operational control currently passes; code, tests, service definitions, and operator
commands remain authoritative for state.

## Current intent

IronWorks is MultiAgency's application and operational layer around official, unmodified
IronClaw. It supplies organization identity, service composition, trusted business records,
tenant lifecycle, evaluation, and security controls without becoming a second agent runtime.

The product model is **managed agentic services**: an organization gets a scoped service that
MultiAgency operates on its behalf, not a runtime it configures or an agent toolkit it builds
with. The multi-tenant path is that model. MultiAgency also operates standalone IronClaw agents
where they fit, but instance-per-agent is not the IronWorks product model and is not what the
product is described as.

Relationship intelligence for MultiAgency's own business runs on the canonical multi-tenant
IronWorks product path. It derives current commitments, obligations, changes, contradictions,
risks, and matters needing a person from durable relationship records. The external
account-analysis service remains a supported composition on that same path for organizations that
need evidence-based qualification and discovery.

Internal and external services use the same tenant loader, guidance binding, composition path,
sealed-account boundary, and operator controls. Internal use does not earn a bypass.

## Current maturity of the service model

Managed agentic services is the product model. The current implementation supports that model over
one shared capability, data, confinement, and runtime architecture; service-specific
infrastructure is not yet generalized. This section exists so that distinction is stated once,
here, rather than implied everywhere.

What is implemented and operated today: tenant provisioning and deletion, service-to-tenant
binding, per-turn composition, per-bearer confinement, network containment, delivery guarantees,
and an operator console over all of it. What is not: a per-service infrastructure contract. Shipped
service definitions share one capability, data, confinement, and runtime shape, and primarily vary
reasoning behavior and bound guidance. Service-specific connectors, tools, permissions, schemas,
and lifecycle are direction, not current functionality.

Concretely: both definitions declare the same frozen capability block, read the same Account
Service under the same schema, are confined by the same script, and pin the same model. What a
service definition selects today is a reasoning objective and the guidance bound to it. A service
needing a different capability shape would not be a new manifest — it would be a change to the
frozen boundary, which is a product decision rather than configuration.
[`multi/services/README.md`](../multi/services/README.md) records which manifest fields are
enforced and which only describe.

## Domain direction

- An **organization** is the business-data scope: one `org_id`, resolved server-side from a
  credential the caller cannot assert, owning that organization's accounts, contacts, and
  activities. It scopes records; it is not by itself the application boundary.
- A **tenant** is the composite application boundary that ties an organization credential to
  group routing, one sealed runtime member, mandatory guidance, one service definition, and
  thread state. Isolation is a property of the tenant; record scoping is a property of the
  organization, and the two are one-to-one by construction rather than by definition.
- An **account** is a counterparty or other relationship-bearing entity, not a project.
- A **contact** is a person associated with an account; an account may accurately have none.
- An **activity** is dated durable evidence, such as an agreement, payment, report, meeting,
  statement, decision, allocation, or note.
- Guidance carries policy and interpretation rules. Records carry facts.
- Current state is derived from records. Do not maintain a parallel store of model-authored
  conclusions as truth.
- Corrections append evidence identifying what they supersede; they do not silently rewrite the
  record.

Add a domain entity or activity kind only when a current reasoning need cannot be represented by
the existing model.

**What the schema models, and what it does not.** `organizations`, `accounts`, `contacts`, and
`activities` are tables in `deploy/account-intel/data/schema.sql`. The last four rules above are
not: supersession, correction-by-append, deriving state rather than storing it, and the
facts/policy split are an authoring convention for whoever writes records, plus a reasoning rule
in the service personas. There is no `supersedes` column and nothing enforces the convention —
the model derives supersession by reading dated activities in order. Say "we write records this
way," not "the system represents this."

## Product boundaries

IronWorks does not:

- fork, patch, or vendor IronClaw;
- operate as a sales pipeline, workflow engine, general ontology, or agent marketplace;
- let the trusted seam reason, score, choose business actions, or execute model-generated
  retrieval plans;
- autonomously write business records, contact people, provision tenants, or make commitments;
- expose member bearer tokens or private Account Service credentials to tenants;
- treat a model, provider, or pin change as a configuration-only event.

The model pin is part of the product promise. A change requires renewed behavioral, isolation,
and security evidence through the current upgrade and verification procedures.

## Design test

A proposed capability belongs in IronWorks when it serves a current organization-scoped use,
keeps facts in durable records, keeps judgment in the model turn, preserves human authority, and
works around the unmodified pinned runtime. Otherwise, defer it until a concrete requirement
justifies expanding the product.
