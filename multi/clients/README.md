# Tenant registry — schema only (the data lives OUTSIDE the repo)

The seam (`multi/seam/registry.py::load_clients`) and the bridge
(`telegram_bridge.py`) read the tenant registry from `CLIENTS_DIR` (default
`~/.agency/clients`): **one file per tenant, `<slug>.env`, chmod 600, never committed**
(matches the `~/.agency` secrets convention; the repo's `*.env` gitignore covers any stray).

**A note on the word.** The concept is a **tenant**, which is what the rest of the tree calls it.
The `client`-shaped identifiers here — `CLIENTS_DIR`, `CLIENT_SLUG`, `CLIENT_NAME`, the
`client-guidance v1` marker, `load_clients`, `ClientConfig`, and this directory's name — are
machine formats validated fail-closed against files already on disk. They stay as they are until
a coordinated rename, and nothing in prose should be read as a second concept.

Files are written by `multi/provision/provision.sh` — hand-editing is fine, the format is
plain `KEY=VALUE` lines (`#` comments allowed):

| key | required | meaning |
|---|---|---|
| `CLIENT_SLUG` | no (defaults to filename) | stable id, lowercase `[a-z0-9-]` |
| `SERVICE` | no (defaults to `account-analysis`) | which **service definition** this tenant runs (`multi/services/`). Must agree with the `service:` field in the guidance file's first-line marker, or the whole registry refuses to load |
| `CLIENT_NAME` | no | display name |
| `ORG_ID` | no (metadata) | **Not authoritative.** The authenticated organization is whatever the Account Service resolves from `ACCOUNT_TOKEN`, server-side; the bridge binds that at startup and re-checks it on every read. This key is recorded for humans and nothing scopes data or conversation continuity by it |
| `ACCOUNT_TOKEN` | **yes** | the tenant's Account-Service org token (identity implies org) |
| `IRONCLAW_TOKEN` | **yes** | the sealed IronClaw member token the seam uses **on the tenant's behalf** — never given to the tenant (see token custody below) |
| `IRONCLAW_USER_ID` | no (metadata) | the sealed account's user id |
| `TELEGRAM_GROUP_ID` | for the bridge | the tenant's private group (negative id) |
| `ACCOUNT_BASE` | no | per-tenant Account-Service base URL override |
| `MODEL` | no | compatibility-only; if present it must equal the repository `MODEL_PIN`. An off-pin value makes the whole registry refuse to load before the bridge can serve |
| `NAME_STOPWORDS` | no | comma-separated words too common in this book's domain to identify an account alone (e.g. `lark,larkmerch` — see below) |
| `FACT_FIELDS` | no | compatibility control for legacy per-partner fact ordering and gap lists. **Tri-state — absent, present-and-empty, and listed have distinct behavior; see below** |
| guidance path | fixed | `<slug>.guidance.md` beside the registry env file. Non-default `GUIDANCE_FILE` paths are rejected because provisioning and deletion intentionally have no general per-tenant path lifecycle. |

**Tenant business guidance is mandatory and fail-closed.** Every registry tenant must have a
guidance file at the canonical `<slug>.guidance.md` path beside its env file (template:
`multi/clients/GUIDANCE.template.md`), slug-bound by its first-line
marker `<!-- client-guidance v1 slug: <slug> -->`. The seam composes that tenant's SERVICE
parts (`multi/services/`) with that guidance — never another tenant's guidance (marker
mismatch refuses to load), never a composition the guidance was not written for, and never
silently absent (a missing file makes the whole registry refuse to load, so a misconfigured
tenant can't run ungoverned). Guidance is tenant data: it lives beside the tokens (chmod 600,
never committed) and is model-visible every turn — no credentials in it, ever.

The marker may also bind the service:

    <!-- client-guidance v1 slug: acme service: account-analysis -->

An absent `service:` field pins the **default** (`account-analysis`), so every file written
before service definitions existed keeps working and keeps pinning the tenant-generic
composition. Whatever it pins, the registry's `SERVICE=` key must agree — which is what makes
moving a tenant onto MultiAgency's internal composition take two deliberate edits in two files
rather than one mistyped key. Full reference: `multi/services/README.md`.

The seam loads every `*.env` at startup; a tenant with both required tokens becomes a
`ClientConfig`, and the bridge maps `TELEGRAM_GROUP_ID -> tenant`. **Restart the bridge after
adding or changing a tenant** — the registry is read once at startup.

Editing `SERVICE`, `MODEL`, `FACT_FIELDS`, `ACCOUNT_BASE`, the guidance file, or anything else
that changes what a tenant's live conversation was composed under makes the bridge **refuse to
start** rather than continue that conversation under history it no longer matches. Reset it
explicitly — `./deploy/ironworks tenant reset-thread <slug> --confirm <slug>` — which clears the
conversation and keeps the delivery journal. Rotating `ACCOUNT_TOKEN` for the *same* organization
is not such a change and needs no reset. Procedure:
[`../../deploy/README.md`](../../deploy/README.md) § Tenant lifecycle.

Deprovisioning: run `multi/provision/deprovision.sh <slug> --execute --confirm <slug>`. It
removes the registry env and guidance, deregisters the org token (hot-reloaded, immediate),
deletes the sealed IronClaw account, deletes the tenant's rows in one transaction, and then
**probes whether the former member token still authenticates**. It exits **3** while it does —
which, on the pinned runtime, it always will: nothing revokes a signed session, and the expiry
is recorded in `~/.agency/residual-authority.json` (no token material) so
`./deploy/ironworks doctor` keeps failing until it lapses. If a token may have LEAKED, do not
wait: follow the credential procedure in `deploy/README.md`. Restart the bridge to drop in-memory routing.

## Token custody (invariant)

**Both tokens in a registry file belong to the seam, not to the tenant.** They are the
credentials the bridge presents *on that tenant's behalf*; the tenant interacts only through
their Telegram group. No tenant, partner, contributor, or end user ever receives a member
token, and no API path lets them present their own.

This is not caution, it is load-bearing: the member's no-egress guarantee is a **per-bearer
tool disable**, and per-bearer state is reversible *by the bearer*. A token holder can
re-enable `builtin.http` and exfiltrate that tenant's private context to any public host —
IronClaw's builtin policy grants wildcard **public** egress by default, so nothing else in the
system would stop it. Handing out a token voids the confinement rather than weakening it.

The trust-boundary statement and its consequences are in the root `README.md` and `SECURITY.md`.

## `FACT_FIELDS` compatibility behavior

`FACT_FIELDS` preserves behavior for records and registry entries that use the Account Service's
legacy flexible `facts` bag. It is not the forward domain model; new concepts should follow
the domain direction in [`../../docs/PRODUCT_DIRECTION.md`](../../docs/PRODUCT_DIRECTION.md)
and require a concrete use.

`FACT_FIELDS` does two things: it sets the order the keys are read into the turn, and it
defines the **gap list** — a declared key with no value is reported to the analyst as
genuinely unknown. That is the whole point: for a funded line, `work_order: missing` is money
that may never have reached a builder, while `economic_buyer: missing` is noise. Reporting
noise every turn teaches the reader to skim the one line that carries the value.

### Three states, not two

`FACT_FIELDS` is **tri-state**, and the middle state is the one that is easy to miss:

| in the env file | means | gap line rendered |
|---|---|---|
| key **absent** | *undeclared* — this tenant has said nothing | the service's **legacy sales-column list** (`budget`, `timeline`, `decision_process`, `economic_buyer`, `stated_problem`) |
| `FACT_FIELDS=` | *declared empty* — "this book has no gap shape" | **none at all** |
| `FACT_FIELDS=a,b` | *declared* — read `a` then `b` | only the declared keys that are absent |

Use `FACT_FIELDS=` to opt out of the legacy fallback. Do not invent a placeholder key. An absent
key retains the fallback only for compatibility with existing registry entries.

Keys beyond the declared set still render (nothing recorded is hidden) — they simply are not
treated as expected. Explain in that tenant's guidance what each key means; the analyst reads
the names, not a schema.

**A declared key must be one the tenant's data source can actually emit.** A mismatch does not
error — it makes every account report a gap that can never close, on every turn, and trains the
reader to skim the one line that carries the value. Reconcile the declaration against the source
before seeding, not after.

## Domain stopwords (`NAME_STOPWORDS`)

The resolver treats a single distinctive word as naming an account. What counts as distinctive
is per record set, so common domain or sponsor words belong in `NAME_STOPWORDS`.

List the words that carry no identifying information in that book. They still resolve an
account when the full name is given, or when a second word from the name also appears; they
just never resolve one on their own.
