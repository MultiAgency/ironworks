# Client registry — schema only (the data lives OUTSIDE the repo)

The seam (`multi/seam/context_ingress.py::load_clients`) and the bridge
(`telegram_bridge.py`) read the client registry from `CLIENTS_DIR` (default
`~/.agency/clients`): **one file per client, `<slug>.env`, chmod 600, never committed**
(matches the `~/.agency` secrets convention; the repo's `*.env` gitignore covers any stray).

Files are written by `multi/provision/provision.sh` — hand-editing is fine, the format is
plain `KEY=VALUE` lines (`#` comments allowed):

| key | required | meaning |
|---|---|---|
| `CLIENT_SLUG` | no (defaults to filename) | stable id, lowercase `[a-z0-9-]` |
| `CLIENT_NAME` | no | display name |
| `ORG_ID` | no (metadata) | Account-Service org (the token implies it; recorded for humans) |
| `ACCOUNT_TOKEN` | **yes** | the client's Account-Service org token (identity implies org) |
| `IRONCLAW_TOKEN` | **yes** | the sealed IronClaw member token the seam uses **on the client's behalf** — never given to the client (see token custody below) |
| `IRONCLAW_USER_ID` | no (metadata) | the sealed account's user id |
| `TELEGRAM_GROUP_ID` | for the bridge | the client's private group (negative id) |
| `ACCOUNT_BASE` | no | per-client Account-Service base URL override |
| `MODEL` | no | per-client model override |
| `NAME_STOPWORDS` | no | comma-separated words too common in this book's domain to identify an account alone (e.g. `lark,larkmerch` — see below) |
| `FACT_FIELDS` | no | comma-separated keys this partner's rows are expected to carry, in reading order (e.g. `cycle,allocation,work_order,delivery`). Drives the per-partner gap list — see below |
| `GUIDANCE_FILE` | no (defaults to `<slug>.guidance.md` beside the env file) | path to this client's business guidance |

**Client business guidance is mandatory and fail-closed.** Every registry client must have
a guidance file (template: `multi/clients/GUIDANCE.template.md`), slug-bound by its
first-line marker `<!-- client-guidance v1 slug: <slug> -->`. The seam composes the
client-generic analyst persona (`agent/identity/ANALYST.md` + `skills/account-analysis/`)
with that guidance — never MultiAgency's own company knowledge, never another client's
guidance (marker mismatch refuses to load), and never silently absent (a missing file makes
the whole registry refuse to load, so a misconfigured client can't run ungoverned).
Guidance is client data: it lives beside the tokens (chmod 600, never committed) and is
model-visible every turn — no credentials in it, ever.

The seam loads every `*.env` at startup; a client with both required tokens becomes a
`ClientConfig`, and the bridge maps `TELEGRAM_GROUP_ID -> client`. **Restart the bridge after
adding or changing a client** — the registry is read once at startup.

Deprovisioning: delete the client's env AND guidance files here, remove its token entry from
`~/.agency/account-identities/identities.json` (takes effect immediately — hot-reloaded, no
restart), and restart the bridge. The sealed IronClaw account keeps existing server-side; no
account-revocation API is documented yet — rotate by re-provisioning if a token is compromised.

## Token custody (invariant)

**Both tokens in a registry file belong to the seam, not to the client.** They are the
credentials the bridge presents *on that client's behalf*; the client interacts only through
their Telegram group. No client, partner, contributor, or end user ever receives a member
token, and no API path lets them present their own.

This is not caution, it is load-bearing: the member's no-egress guarantee is a **per-bearer
tool disable**, and per-bearer state is reversible *by the bearer*. A token holder can
re-enable `builtin.http` and exfiltrate that client's private context to any public host —
ironclaw's builtin policy grants wildcard **public** egress by default, so nothing else in the
system would stop it. Handing out a token voids the confinement rather than weakening it.

Full statement and its consequences: `docs/ARCHITECTURE.md` § Token custody.

## Book shape is per partner (`FACT_FIELDS`)

The `accounts` table's fixed columns encode one theory of what matters — a B2B sales account
(`industry`, `employees`, `economic_buyer`, `decision_process`). Most books are shaped
differently: funded lines, grantees, programmes, venues. So each row also carries a `facts`
JSONB bag holding whatever keys **that** relationship needs, and the registry declares which
of them are expected.

`FACT_FIELDS` does two things: it sets the order the keys are read into the turn, and it
defines the **gap list** — a declared key with no value is reported to the analyst as
genuinely unknown. That is the whole point: for a funded line, `work_order: missing` is money
that may never have reached a builder, while `economic_buyer: missing` is noise. Reporting
noise every turn teaches the reader to skim the one line that carries the value.

With no `FACT_FIELDS`, the seam asserts no gaps of its own and falls back to the service's
sales-column list. Silence is honest; an invented gap list is not.

Keys beyond the declared set still render (nothing recorded is hidden) — they simply are not
treated as expected. Explain in that client's guidance what each key means; the analyst reads
the names, not a schema.

## Domain stopwords (`NAME_STOPWORDS`)

The resolver treats a single distinctive word as naming an account. What counts as distinctive
is **per book**: in one client's book every account name carries the sponsoring org's own name,
which is also a common word in that domain, so it appears in nearly every sentence — yet it
matched one account name and sent a book-wide question to that single row (observed live).

List the words that carry no identifying information in that book. They still resolve an
account when the full name is given, or when a second word from the name also appears; they
just never resolve one on their own.
