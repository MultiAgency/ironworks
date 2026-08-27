# Service definitions

A **service definition** is the reusable half of a MultiAgency-operated agent service, written
down as data. A **tenant** is the other half.

```
service definition   what the service IS      committed here (multi/services/*.json)
tenant configuration who runs it, and where   ~/.agency/clients/<slug>.env
tenant guidance      their policy and rules   ~/.agency/clients/<slug>.guidance.md
tenant secrets       their credentials        the same .env, chmod 600, never committed
tenant business data their account book       the Account Service, org-scoped
deployment state     threads, cursors, journal ~/.agency/bridge-threads.db
```

Those six stay separate on purpose. A change to what the service *is* is a release; a change to
who runs it is provisioning; a change to their guidance is a conversation with that
organization. Collapsing any two of them makes one of those three things silently do another.

Internal and external compositions both run through
`load_clients → ClientConfig → Thread → turn`. Being internal grants no bypass:
`multi/seam/test_services.py` requires guidance, slug binding, and a tenant-neutral serving path.

## The two that ship

| service | audience | composition | who runs it |
|---|---|---|---|
| `account-analysis@1` | external | `ANALYST.md` + `skills/account-analysis/` | every client organization; the **default** for a registry entry with no `SERVICE=` key |
| `relationship-intelligence@1` | internal | `RELATIONSHIP_INTELLIGENCE.md` + `skills/relationship-record/` | MultiAgency, on its own relationship record |

**Do not add a service to make the catalogue look fuller.** A new service
earns its definition when a real tenant needs a composition neither of these gives them.

## What varies between services today, and what does not

A service definition is the reusable half of a service, and it is a real abstraction: the two
that ship compose different persona parts, pursue different reasoning objectives, and bind to
different tenants through the same loader. What they do **not** yet vary is everything below the
prompt. Both declare the same `capabilities` block — a test freezes it identical across every
definition — read the same Account Service under the same `data_schema`, are confined by the same
`tool_policy` script, and pin the same model.

So today a service definition selects **a reasoning objective and the guidance bound to it**.
Service-specific connectors, permissions, tool surfaces, data sources, and lifecycle are the
intended direction and are not implemented. A service needing a different capability shape would
not be a new manifest; it would be a change to the frozen boundary, which is a product decision
rather than configuration. Product intent and its current maturity are in
[`../../docs/PRODUCT_DIRECTION.md`](../../docs/PRODUCT_DIRECTION.md).

**This is not a plugin manifest.** Read the field table below as a record of that: what it
enforces is composition, and the fields that sound like capability configuration — `capabilities`,
`tool_policy`, `data_schema` — record a shared invariant rather than configuring one per service.

## The fields

Three kinds, and the difference matters when you change one. **Enforced** fields change behavior.
**Validated** fields are checked but nothing branches on them at serve time. **Descriptive**
fields are read by no code at all — they record an invariant for a human, and editing one changes
nothing.

### Enforced — the serving path reads these

| key | meaning |
|---|---|
| `service` | the id, which must equal the filename; load fails if they disagree. It is what a registry `SERVICE=` key and a guidance marker name, and both must agree with it. |
| `persona_parts` | repo-relative files, composed in order. Validated at load and read at compose time: a renamed part fails here, not in front of a client. |
| `guidance` | `required` — the only supported value; any other value refuses to load. An optional mode would be a fail-open path and there is no use for one. |
| `guidance_heading` | the model-visible section title the tenant's guidance is injected under. |
| `safety_tail` | appended last to every composition. |
| `model_policy` | `pin` — the only supported policy. The registry loader rejects both process-level and per-tenant off-pin `MODEL` values before the bridge can serve. |

### Validated — checked at load, but nothing branches on them per turn

| key | meaning |
|---|---|
| `version` | a positive integer, validated at load and carried into `<service>@<version>` for the operator console, release artifact, and persisted thread compatibility identity. A mismatch refuses thread continuation until explicit reset. **It is compatibility-bound metadata, not an independent lifecycle:** two versions cannot coexist, and there is no version rollout or migration machinery. Bump it to mark a composition change a human should notice, not to expect staged rollout behavior. |
| `audience` | `internal` or `external`; any other value refuses to load. Nothing branches on it at serve time. `ironworks doctor` asserts the DEFAULT service is `external`, so a forgotten `SERVICE=` key can never land a tenant on an internal composition. |
| `capabilities` | the declared shape: `account_records: read-only`, `writes: none`, `egress: none`, `outreach: none`. `test_services.py` asserts every definition declares exactly this, so the frozen boundary is machine-checked rather than restated in prose. It is an assertion about the product, **not** a switch: no code grants or denies anything from this block. What actually holds the boundary is the read-only Account Service surface, `confine-member.sh`, and the network boundary. |
| `evaluation` | the suite that measures whether this service's answers are any good, as a repo-relative path — or `null`, meaning none does. Required, and load-validated: a path that is not on disk refuses to load, so a suite cannot be named after it is moved or removed. Nothing is *run* from here; `multi/eval/run_eval.py` is invoked directly, and a declared path is a claim about which suite covers this service, not a hook. |

### Descriptive — read by no code

| key | meaning |
|---|---|
| `tool_policy` | which script confines a member of this service. Provisioning invokes `confine-member.sh` by path (`provision.sh`, `confine-existing.sh`); it does not read this field, so changing it confines nothing differently. |
| `data_schema` | the schema the tenant's book is expected to match. Nothing validates a book against it. |
| `title`, `summary` | prose for a human reading the definition. |

## Binding a service to a tenant takes TWO agreeing edits

The registry entry names it:

```
SERVICE=relationship-intelligence
```

...and the guidance file's first-line marker names it too:

```
<!-- client-guidance v1 slug: multiagency service: relationship-intelligence -->
```

If they disagree, the registry **refuses to load** — the whole registry, not just that tenant.
A guidance file with no `service:` field pins the **default** for compatibility.

This is the same slug-binding trick that already stops one client's guidance reaching another,
applied to the composition. The failure it exists to prevent is a single mistyped key moving an
external tenant onto MultiAgency's internal composition — which would put our own company
knowledge in front of a client. One edit cannot do it; two deliberate edits in two files can,
which is what a genuine service change looks like.

## Adding one

1. Write the persona parts. Prove the composition in a test before any tenant runs it.
2. Add `multi/services/<name>.json`. `./deploy/ironworks service validate` must pass.
3. Add the evaluation cases that say what a good answer looks like for THIS service, or declare
   `"evaluation": null` and know that nothing measures its answers. `multi/eval/` composes the
   DEFAULT service and grades account-qualification cases; it measures the account-analysis claim
   and does not generalise for free. `relationship-intelligence@1` declares `null` for exactly
   this reason. Its structural tests — the composition and objective assertions in
   `multi/seam/test_services.py` — are not a substitute: they prove which text composes, never
   whether an answer is any good.

   **Do not write a suite to fill a `null`.** The `null` is information, and a suite built to
   remove it measures nothing anyone claimed. Add one when there is a product claim about that
   service's answers and cases that actually test it.
4. Provision a tenant with `--service <name>`, and put `service: <name>` in its guidance
   marker. Preflight validates the pair before it creates any authority.

## What a service definition is not

It is not a plugin manifest, and `multi/services/` is not a marketplace. Nothing here is
customer-authored, customer-selectable, or loaded from outside the repo: these are product
decisions, committed and reviewed like any other. The customer-controlled surface is their
**guidance**, which is data injected into a prompt — not code, not capabilities, not tools.
