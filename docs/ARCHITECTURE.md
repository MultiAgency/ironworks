# ironworks architecture

How the MultiAgency system actually works. The repo is **`ironworks`** — the foundry
that builds and runs everything below; the system is **MultiAgency**. Fleet topology
validated live; the multi-tenant product architecture verified against the IronClaw
source and proven live at the pinned rev (see `multi/verify/`).

## Two planes, one rule

Everything here runs the **official ironclaw binary** (the harness / agent-OS),
**unmodified — no fork, no patches**, at the rev recorded in the repo-root
`IRONCLAW_PIN` file (the version of record; bumped only via `deploy/UPGRADE.md`).
Everything MultiAgency-specific is config, data (personas), and scripts wrapped
*around* the harness, so pulling upstream is a clean rebuild, never a merge.

On that rule sit two planes:

1. **The product — `multi/`.** One Multi agent serving many clients, each sealed in
   their own account on a **single multi-tenant ironclaw instance**. Persona and
   business context are injected **per turn** by the channel seam. This is the
   client-facing system of record.
2. **The internal control plane — the fleet.** Instance-per-agent single-tenant
   ironclaw instances for the crew's own coordination, persona design, and fleet ops.
   Internal only; no client data.

## The product: `multi`

**Multi-tenant ironclaw gives DATA isolation, not per-account personas** (traced in
the IronClaw source — see `multi/README.md` for the citations):

- Accounts are sealed per `(tenant, user)`: cross-account requests return `404`, and
  member turns get no code execution (`SecureDefault`). Proven behaviorally on the
  compliant path (`multi/verify/test_two_clients.py`, 11/11) **and** the hostile path
  (`multi/verify/test_adversarial_cross_org.py`, 5/5 — an injected exfiltration turn
  as client A gets zero of client B's data).
- `SecureDefault` removes code execution but NOT network egress: a fresh member still
  ships `builtin.http` with a compiled-in wildcard egress policy (no config knob narrows
  it — it is `include_str!` in ironclaw). So while a member cannot reach *another* client's
  data, an un-confined member could exfiltrate *its own* client's private context to an
  arbitrary host under prompt injection. We close this at provisioning:
  `multi/provision/confine-member.sh` disables every egress/write/escalate tool per-bearer
  and probes fail-closed (`provision.sh` refuses to provision a member it cannot confine).
  This holds only under the **token-custody invariant** below. Clients minted before the fix
  must be back-filled with `multi/provision/confine-existing.sh`; re-run it (and the
  egress-closed check) after any ironclaw pin bump, since the tool taxonomy can change — that
  is a **gate on the bump, not a follow-up** (`deploy/UPGRADE.md` step 6).

### Token custody (invariant — do not break)

**Every member token belongs to the seam. No client, partner, contributor, or end user ever
receives one, and no API path lets them present their own.**

The member's no-egress guarantee is a **per-bearer tool disable**, and per-bearer state is
reversible *by the bearer*. Whoever holds a member token can re-enable `builtin.http` and
exfiltrate that client's private context to any public host. Nothing else in the system stops
it: ironclaw's compiled-in builtin policy grants **wildcard public egress** by default
(`dev_wildcard`; only private-IP ranges, loopback, link-local, and cloud metadata are blocked
— which is why the private data layer is safe from the model, but the public internet is not).
So handing out a token does not weaken the confinement, it **voids** it.

Two consequences to keep in view:
- The isolation story is only as strong as this invariant. Any future feature that gives a
  partner direct API access — their own integration, a self-serve dashboard, a webhook —
  breaks it, and must be designed against this constraint rather than around it.
- Because the control is per-bearer and procedural, the durable version is **network-level
  default-deny egress** around the instance (which needs exactly one destination:
  `cloud-api.near.ai`). That survives tool changes, pin bumps, and custody mistakes. It is
  deliberately deferred until real records exist to protect (backlog: trigger is the first
  real records landing in any book).
- There is **no shipped path to give each account its own persona**: on the
  hosted-multi-tenant profile the instance bakes no persona at all, and per-account
  personas would require patching core. **We do not patch core.**

Therefore the persona lives in the **channel/backend seam**, not the instance. Per
turn, the seam sends to the client's sealed scope:

    instructions = the persona                       (re-sent EVERY turn — a one-time
                                                      injection drifts; proven in
                                                      multi/verify/test_injection*.py)
    input        = ACCOUNT RECORDS envelope          (this client's data, fetched
                                                      org-scoped from the Account
                                                      Service) + the user's request

Two persona compositions, deliberately disjoint (`multi/seam/persona.py`):

- **Internal** (MultiAgency's own book): `agent/identity/ACCOUNT_INTELLIGENCE.md` +
  the `company-knowledge` and `account-intelligence` skills.
- **External clients**: the client-generic `agent/identity/ANALYST.md` + the
  `account-analysis` skill + that client's **mandatory slug-bound business guidance**
  (`~/.agency/clients/<slug>.guidance.md`). Guidance is validated and **fails closed**
  — a client never runs with no guidance, with another client's guidance, or with
  MultiAgency's internal composition.

Component map (detail of record: `multi/README.md`):

    multi/instance/    the multi-tenant ironclaw instance definition (compose + .env.example)
    multi/seam/        the product core: context_ingress.py (trusted broker),
                       persona.py, telegram_bridge.py + unit suites
    multi/provision/   provision.sh (org + sealed account + registry) /
                       provision-client.sh (the sealed-account mint primitive) /
                       deprovision.sh (operator deletion CLI)
    multi/clients/     client-registry SCHEMA + guidance template
                       (live registry data: ~/.agency/clients/, never in-repo)
    multi/verify/      the reproducible proofs (see its README for the full index)

Two supporting live services remain under `deploy/` (entangled with shared tooling):
`deploy/secretary/worker/` (the front-desk secretary, a Cloudflare Worker) and
`deploy/account-intel/data/` (the Account Service — the org-scoped client-data layer).

### Product topology

```
   Telegram (client group)
        │
        ▼
   telegram_bridge.py ── one bridge process, one bot (accepted SPOF)
        │  chat.id → ClientConfig (registry ~/.agency/clients/<slug>.env; fail-closed)
        ▼
   context_ingress.py ── per turn: instructions = persona (client-specific, fail-closed)
        │                          input        = org-scoped context envelope + request
        ├───────────────► Account Service  127.0.0.1:8443  (org-scoped data, token-implies-org)
        ▼
   multi-tenant ironclaw instance  127.0.0.1:3020
        └─ one sealed account per client  (tenant/user-scoped memory, threads, secrets)
```

### Product runtime flow (a message → a reply)

1. A member posts in their client's Telegram group (@mention or reply to the bot).
2. The bridge routes by `chat.id` — never by message content — to exactly one
   registered `ClientConfig`; unregistered groups fail closed.
3. The seam composes that client's persona (fail-closed guidance) and fetches only
   that client's org-scoped context from the Account Service.
4. The turn runs in the client's **sealed account** on the MT instance; the reply
   returns through the bridge to the group.

## The control plane: the fleet

Each internal agent is its **own stock ironclaw instance**: its own container,
volume, Telegram bot, persona, and hostname — the fleet's isolation model.

Why instance-per-agent rather than one shared bot routing many groups? Because the
stock binary can't: a channel turn's `(agent, project)` scope comes from a single
installation default and shared-conversation scoping was deliberately retired
upstream. Per-group isolation on one bot would need a harness change we chose not to
make. Instance-per-agent gets the same isolation, stronger (OS + harness), with zero
code — the trade is a container + bot per agent, fine at this scale.

<!-- The fleet table is parameterized deliberately, not provisionally. Live hostnames,
     bot handles and ports stay in ~/.agency/agents/*.env per the .gitignore policy that
     infra addresses are never committed. Note that <slug>/<your-domain>/<your_bot> is the
     repo-wide convention — it also carries the script usage strings — so filling in live
     values here alone would leave the tree inconsistent with itself. -->

| Agent | Container | Port | Host | Bot | Role |
|---|---|---|---|---|---|
| **Multron** | `ironclaw-<slug>` | `127.0.0.1:<port>` | `<slug>.<your-domain>` | `@<your_bot>` | internal contributors' agent → **fleet coordination** |

(Live values — hostnames, bot handles, ports — stay in `~/.agency/agents/*.env`, per
the `.gitignore` policy that infra addresses are never committed. Multiplex — persona
design — and Ops — fleet-ops copilot — were retired: their personas and bot
tokens are kept, and `provision-agent.sh` revives either if a need appears.)

This table is the control plane **of record**, not an inventory of what is running. Retired and
frozen experiments can leave an `ironclaw-*` container up long after their directory left the
repo, and a container with no `~/.agency/agents/<slug>.env` is checked by nothing — which is
precisely what `doctor.sh`'s whole-fleet coverage check exists to surface. Read the box, not
this table, for what is live.

Multron is the fleet's **internal control plane** — it coordinates the crew.
**Client-facing work is the `multi`
product** (previous section). `MULTI.template.md` remains the fleet-era per-group
onboarding template, stamped out by `provision-agent.sh` when a dedicated single-tenant
group agent is wanted.

### Fleet topology

```
   Telegram (contributor / operator)
        │  t.me/<bot>   (DM or group @mention)
        ▼
   Telegram Bot API ──signed webhook──► cloudflared (one named tunnel)
                                         per-agent ingress: <slug>.<your-domain> → localhost:<port>
        │                                        │
        └────────────────────────────────────────┤
                                                  ▼
        ┌──────────────────────┬──────────────────────────────────────────────┐
        │ ironclaw-<slug>:port  │ one instance per agent; add or remove one     │
        │ vol ironclaw-<slug>-  │ with provision-agent.sh. The control plane    │
        │ data                  │ of record is ONE agent: Multron.             │
        │ persona: SOUL.md      │                                              │
        └──────────────────────┴──────────────────────────────────────────────┘
   every fleet instance: image ironclaw:main (built at the IRONCLAW_PIN rev) ·
   profile hosted-single-tenant-volume · state in its own /data/ironclaw-reborn volume
```

## Personas — the full inventory

`agent/identity/` ships **seven** files. This section is the exhaustive inventory of record,
and the **only** place that count is written down — every other doc names the directory and
points here, precisely so adding or removing a persona means editing one list instead of
hunting for stale numbers in four files. Grouped by plane:

**Product (channel-injected per turn by `multi/seam/persona.py` — baked nowhere):**

- `ACCOUNT_INTELLIGENCE.md` — Multi's proven account-intelligence persona
  (MultiAgency's internal composition).
- `ANALYST.md` — the client-generic analyst persona; external clients get this plus
  their own slug-bound guidance (never the internal composition).

**Control plane (baked into each fleet instance's system prompt by
`provision-agent.sh`):**

- `SOUL.md` — Multron, the internal contributors' agent (a live prompt: editing it
  implies a persona re-install on the running instance).

**Frozen experiment (persona kept with its experiment):**

- `MULTIMEDIATOR.md` — the vidgen contributors' agent (`deploy/vidgen/`). Frozen as an
  investment, not as a process: the instance still runs, so this is a live prompt too.

**Template + shared tail:**

- `MULTI.template.md` — the parametrized per-group default (`{{AGENT_NAME}}` /
  `{{PURPOSE}}` slots) for fleet-era group agents.
- `_operational-tail.md` — Response Style / Computation / Files / **Safety**,
  appended to every *baked* persona so the guardrails always ride along.
- `_safety-tail.md` — the product-path counterpart: appended by
  `multi/seam/persona.py` to **every** channel-injected composition, internal and
  client-facing alike, so a client turn can never run without it. One tail per
  composition path; neither is optional.

Not in the seven, and deliberately — each exists on disk but is gitignored, and
`provision-agent.sh` builds from a local copy via `PERSONA_SOURCE`, so nothing here is lost
operationally:

- `IDEA_SCOUT.md` — partner-specific commercial content. Personas written for one partner
  are not published.
- `MULTIPLEX.md` (persona design) and `OPS.md` (fleet-ops copilot) — **agents we retired.**
  An inventory that ships them describes a fleet that no longer exists, and a persona nobody
  runs is a persona nobody notices going stale.
- `INTAKE.md` — the receiving desk from the retired cross-instance handoff PoC. Its tooling
  left this repo, so the persona documented a thing a reader could not find.

Two composition paths, one invariant: the repo file is the source of truth; a running
instance holds only a composed copy. `provision-agent.sh` composes `PERSONA_SOURCE` +
the tail and installs it at `…/system/prompts/default-system.md`; the product seam
composes per turn and sends via `instructions`.

## Tooling (`deploy/`)

- **`provision-agent.sh "<slug>"`** — stands up an isolated fleet agent in one
  command: container + volume + persona install + DNS + tunnel ingress + Telegram
  config + activation + webhook verify. Env-driven (`PERSONA_SOURCE`, `AGENT_NAME`,
  `PURPOSE`, `AGENT_HOSTNAME`, `TELEGRAM_BOT_TOKEN`, `TELEGRAM_BOT_USERNAME`). One
  manual step remains: add the bot to the group **as admin** (see § Member access).
- **`doctor.sh [<slug>] [--deep]`** — end-to-end health per fleet agent (container,
  API, persona, public reachability, signed delivery, ingress, bot token, webhook
  registration; `--deep` asserts the pinned Telegram auth model — device-link build).
  Reads `~/.agency/agents/*.env`.
- **The product's provisioning lives with the product**: `multi/provision/`.
- Subdirectory status (live vs frozen): see `deploy/README.md`.

## Member access (fleet instances)

Members reach a fleet agent through IronClaw's native Telegram bot, which
`provision-agent.sh` configures. **Adding the bot to a group as admin does NOT make the group
served** on the pinned rev. Each person links their own Telegram account individually from the
Telegram extension card in the Web UI; linking cannot be done from chat. Turns then run as the
invoking member (run-acts-as-invoker), and an actor the instance cannot admit **fails closed**
(`BindingRequired`) — the access gate is intact.

Admin-in-group *was* presence admission before this rev, and that stale belief misled a fleet
build: the bot was admin in a group, every @mention returned "Link this Telegram account…", and
no users were created. Measured corroboration: the `multron` instance has exactly ONE admitted
user, which is what per-person linking predicts and group presence-admission would not.

**What changed.** Upstream #7464 switched Telegram's `[channel.connection]` strategy from
`WebGeneratedCode` — a lightweight host-minted proof code behind `t.me/<bot>?start=<code>`, which
the retired `deploy/intake/provision-user.sh` drove — to `DeviceLink`. The generic pairing routes
still exist (`.../{extension_id}/pairing/{mint,status,unpair}`, wired in `channel_pairing.rs`);
Telegram simply registers no pairing service now (it declares `method="device_link"`), so
`pairing/mint` **404s for `extension_id=telegram` specifically** — a per-extension registry
decision, not a removed route (`deploy/migrate-image.sh` and `doctor.sh --deep` both treat that
404 as the expected healthy answer).

Device-link (`/api/reborn/product-auth/device-link/{start,poll,input,cancel}`) is MTProto
**account** linking — the agent acting *as* the user's Telegram account, via
`auth.ExportLoginToken`, DC migration and SRP 2FA. It is a multi-step interactive flow, not a
pre-mintable link, so **there is no per-member DM intake link on the stock pinned binary**. It is
confirmed wired and working: a completed `device-link-v1` binding exists on a live instance
(verified in `ironclaw-multimediator`'s store). Its identity is namespaced
(`<installation>:device-link-v1:`) and resolved per-caller, so a member's run-as-invoker identity
and an operator's linked-account MTProto session stay strictly separate — a member turn cannot
drive the operator's linked session (verified in `device_link_channel_identity.rs` +
`channel_identity.rs`).

**It is the wrong tool for onboarding a contributor GROUP — a fit gap, not a bug.** The driver
for #7464 was the linked-*account* tools, which need a real MTProto session a proof code cannot
provide; chat identity now rides on top of that heavier ceremony. So (1) **weight** — every
person MTProto-links their whole account just to chat; and (2) **fit** — device-link connects one
user's own account, it is not a many-people chat-onboarding mechanism. Whether a device-linked
member is even *served* for group @mentions is **unverified**: live tests on `ironclaw-multron`
showed silence, but every run was confounded (operator-identity first, churned state, an
unconfirmable mention, and the turn-admission record rolled off the ~87-line operator-log ring).
The code path (`ProviderIdentityActorResolver` on the `device-link-v1:` keyspace, 30s cache)
suggests it should resolve, so it is neither confirmed working nor broken — and that verdict
barely matters, because even if it works it is the wrong shape.

**The v1.3.0-aligned way to serve many people is the multi-tenant model**: the seam bridge
(`multi/seam/telegram_bridge.py`) is `AdminManagedChannels`-shaped — operator-configured bot,
`chat.id` binding, one thread per group, no per-person linking — which is what the product
(`multi/`) uses. Reserve device-link for the single-user "connect my own account to a fleet
agent" case; `deploy/enable-device-link.sh` turns it on, needs MTProto api creds, and a bot-only
instance cannot even start the flow.


## Isolation model

1. **Between fleet agents** — instance-per-agent: separate containers, volumes, and
   state stores. One agent's process cannot see another's data.
2. **Within an instance** — per-member: turns run **as the invoking member**
   (run-as-invoker), with per-user memory/threads scoped by the harness.
3. **Between clients on the product instance** — one MT instance, one sealed account
   per client: cross-account reads 404, no code-exec, no token crossover — proven on
   the compliant and hostile paths plus the tenant-wide surfaces
   (`multi/verify/test_tenant_shared_*.py`, `test_catalog_parity.py`,
   `test_member_admin_negative.py`). Operational rules for keeping it that way:
   `multi/README.md` § Isolation rules.

> Turn/dispatch diagnostics go to an internal Operator-Logs layer, not stderr —
> `docker logs` looks empty during a live conversation. Diagnose with `doctor.sh` and
> observable Telegram checks (`getWebhookInfo`, `getUpdates`).

## Secrets

- Per fleet agent: `~/.agency/agents/<slug>.env` (operator token, webhook secret,
  hostname, bot username, container name; chmod 600) and `~/.agency/<slug>.token`
  (bot token). `provision-agent.sh` writes the env file and never echoes secrets.
- Product: `multi/instance/.env` (from the committed `.env.example`; minted on the
  host) and the client registry + guidance under `~/.agency/clients/`.
- Nothing above is committed — `.gitignore` covers `*.env` / `*.token` / `*.key`.

## Known limits

**Fleet (stock-binary, verified live):**

- **No per-group agent/project scope routing** on one bot — retired subject-routing;
  would need an upstream seam. Hence instance-per-agent.
- **No shared-group `outbound_deliver`** — a turn can only deliver to the caller's
  own targets, and shared groups aren't targets. Anything a turn produces surfaces in
  the conversation, not auto-posted elsewhere.
- **No per-channel capability floor** — a turn gets the full installed tool surface;
  confine by which tools an instance installs.
- **Group reception** — a bot in a basic group with privacy-mode ON silently drops
  @mentions; make it an **admin** (which upgrades the group to a supergroup).
- **Fresh-hostname webhook lag** — a new DNS CNAME can hit Telegram's
  negative-DNS-cache for a few minutes; the webhook registers on the next restart and
  self-heals on retry.

**Product (multi-tenant profile, verified in source):**

- **No per-account persona upstream** — the MT profile bakes no identity; the seam's
  per-turn injection is the only persona path, and it must re-send every turn
  (a once-only injection drifts).
- **No extension or skill lifecycle on the MT profile** — extension lifecycle
  commands fail closed on `hosted_multi_tenant`/`production`, and there is no
  multi-tenant profile upstream that has them. The seam owns Telegram; any future
  "install an extension on the MT instance" plan is blocked by profile.
- **Tenant-wide surfaces exist and are kept dormant by configuration** — the
  tenant-shared credential scope and shared skill/tool catalog serve every sealed
  client identically; the instance env allowlist is load-bearing
  (`multi/README.md` § Isolation rules).

## Repo layout

```
IRONCLAW_PIN       the deployed upstream rev — the version of record
MODEL_PIN          the model of record for every agent (and why that model)
agent/identity/    the persona files (inventory and count: § Personas above)
agent/config/      agent-level config notes
deploy/            fleet + service tooling (status of every subdir: deploy/README.md)
  provision-agent.sh · doctor.sh · UPGRADE.md · lib/ · secretary/ ·
  account-intel/ · vidgen/ (FROZEN)
docs/              this file · agent-spec.md
multi/             the product (map: § The product above; detail: multi/README.md)
skills/            skill files injected into product personas
```

The image is built from ironclaw's **own** `Dockerfile` at the `IRONCLAW_PIN` rev
(ironworks keeps no Dockerfile of its own). Pull upstream = bump the pin via
`deploy/UPGRADE.md`, rebuild, re-provision. No merge, no fork.

## One-line summary

ironworks ships **`multi`** — one Multi agent serving many clients, each sealed in
their own account on a single **unmodified multi-tenant ironclaw instance**, persona
and per-client context injected fresh every turn by the seam — operated by an
internal **fleet** of isolated single-tenant agents coordinated by Multron, all on
the pinned official binary with **zero fork**.
