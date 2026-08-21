# MultiAgency trusted context ingress (application adapter)

**Not a second agent runtime. Not an orchestrator platform.** This is the thin, trusted
application layer that supplies business context to IronClaw before a turn.

```
Telegram group → trusted MultiAgency ingress → private Account Service context → IronClaw reasoning → group response
```

## Boundary (locked)
- **IronClaw** orchestrates the reasoning / conversation (thread continuity, multi-turn, the group discussion).
- **MultiAgency ingress** supplies trusted business context *before* the turn.
- The ingress **may:** authenticate the caller, resolve org/user identity, resolve the referenced
  account or bounded candidate set, fetch private Account Service context, build the context envelope,
  invoke IronClaw, return the response.
- The ingress **must not:** plan, reason, score, qualify, decide next actions, run model-generated
  fetch commands, become a workflow engine, or duplicate IronClaw's agent loop. Context selection is
  **deterministic prefetch only**.

## Why it exists (don't reopen)

The obvious design is a WASM tool extension that calls the Account Service directly, so the model
gets named business tools and the host injects the credential. **That is impossible on this
runtime, by deliberate upstream design**, and the seam is what replaces it.

IronClaw's extension sandbox denies private-IP egress unconditionally for any networked WASM
capability. Verified in the source at `IRONCLAW_PIN`:

- `crates/extensions/ironclaw_extension_host/src/capability_surface.rs:203` —
  `deny_private_ip_ranges: has_egress_targets`. Declaring *any* egress target turns the guard on;
  there is no way to declare a target and opt out.
- `crates/substrates/ironclaw_network/src/policy.rs` (`is_private_or_loopback_ip`, and the
  resolver check in `resolver.rs`) — the transport then refuses any private, loopback or
  link-local address, after DNS resolution, so a public name pointing at a private IP fails too.
- **No operator exception exists.** Every other assignment of that field in the tree is a
  hardcoded `true`; the only computed forms are the two above. It is not settable from config or
  env, so this cannot be turned off without patching core — which this repo does not do.

The Account Service lives on a private Docker IP, so a WASM extension cannot reach it: the
request is blocked before it leaves the sandbox and the service never sees it. WASM extension
egress is built for **public** API hosts; a private internal service is unreachable *by design*,
not by misconfiguration.

Read the consequence the right way round — this is a **feature the product depends on**. The
agent is deliberately denied authority to reach the operator's private infrastructure. The
trusted app layer holds the credential and the private-network reach, and hands the agent only
scoped context. **The account token and Account Service host live only in the ingress — never in
the IronClaw request** (which carries only `{model, instructions, input, previous_response_id}`;
`input` is business facts, no secret).

The only supported way to make the WASM route work would be exposing the Account Service on a
publicly-routable endpoint (e.g. a tunnel) purely to satisfy the guard — trading a real isolation
property for a cosmetic one. Don't. A prototype extension was built and taken to this exact
blocker before the seam replaced it; it is not in the tree, and the paragraph above is what it
cost to learn.

## Files
- `context_ingress.py` — resolve → fetch → package → call IronClaw → return. Importable (`turn(thread, text)`),
  and runnable directly as the **verification oracle** (runs the frozen hero flow through backend context).
- `telegram_bridge.py` — the channel side: long-polls one bot, relays sales-group messages into
  `context_ingress.turn`, posts replies. One message in, one reply out.

## Verified (isolation)
- **identity-implies-org** end-to-end: sales token → own org; rival token → 0; caller cannot assert an org
  via `X-Org-Id`; unknown token → 401.
- **envelope injected once:** turn 1 (prioritization) supplies the candidate set; later turns supply nothing
  (thread history + human-added facts carry it).
- **frozen hero flow materially equivalent** through backend context (Northwind ranked with FACT/INFERENCE
  discipline; clean reassessment on the budget/Zendesk facts; honest Apex deprioritization; handoff to
  MultiAgencyHQ without authoring a work order).
- **no private credentials enter IronClaw** (the account token/host are used only in `_svc`, never in
  the IronClaw request).
- **no network authority in a member turn** — but this is NOT inherited from IronClaw: a fresh member
  ships `builtin.http` with a compiled-in wildcard egress policy, so each sealed member is CONFINED at
  provisioning (`multi/provision/confine-member.sh`) to a read-only, no-egress tool surface. Without
  that step a prompt-injected turn could POST this client's private context to any host (verified, then
  closed). The confinement is per-bearer and fail-closed; re-run it if IronClaw's tool catalog changes.
- **deterministic, prefetch-only** (name match, else the whole book; no model-generated fetch).

Run the oracle:
```sh
# store up first:  bash deploy/account-intel/data/dev-up.sh (from repo root) — it injects the dev identities
IRONCLAW_API=<instance-api> IRONCLAW_TOKEN=<operator-token> \
ACCOUNT_BASE=http://127.0.0.1:8443 ACCOUNT_TOKEN=mia_sales_token \
  python3 context_ingress.py
```

---

# Operator runbook — go live in the private Telegram sales room

This is the exact procedure for bringing the seam up **locally**, by hand. For the 24/7
deployment — systemd units, off-box backups, watchdog — see `multi/serve/`, which supersedes
running these steps in a terminal.

## Secret handling (read first)
- The **bot token**, **IronClaw operator token**, and **Account Service token** are provisioned **locally** as
  environment variables (or an untracked env file). **Never commit, log, print, paste into chat/reports, or
  screenshot them.**
- `.gitignore` covers `.env` / `*.env` / `secrets*` / `*.secret` / `*.key` / `*.pem`. Keep secrets in env vars or
  under `~/.agency/` (outside the repo). Prefer sourcing an untracked file, `chmod 600`, over typing inline.
- The dev **org tokens** are injected by `deploy/account-intel/data/dev-up.sh`
  (`mia_sales_token` → org), NOT by the compose file — compose only reads
  `${ACCOUNT_DEV_IDENTITIES:-}`, which `prod-up.sh` leaves empty. They are org identifiers for a
  loopback-only demo stack, **not** real secrets: `dev-up.sh` refuses to run on a host that
  already has registered identities, and `prod-up.sh` asserts they are dead in prod. Replace
  them with a secret store before any external exposure.
- The processes print **no credentials**: startup prints only the group id; error output is redacted
  (`telegram_bridge._redact` strips the bot/account/IronClaw tokens from any error string).
- The `getUpdates` URL contains the bot token — do **not** paste that command or its output anywhere shared.

## 1. Telegram group + bot
1. Create a bot with **@BotFather** → obtain the **token** and **username**. Store the token locally only.
2. Create a **private** Telegram group for the MultiAgency sales team.
3. Add the bot to the group and make it an **admin** (basic groups with privacy-mode ON silently drop messages;
   admin upgrades to a supergroup and guarantees the bot receives messages).

## 2. Obtain the group chat id
- Send one message in the group, then run **locally** (not in any shared session):
  `curl "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/getUpdates"` and read `message.chat.id`
  (a negative number, e.g. `-100…`). The token is in that URL — keep the command and output private.

## 3. Set local environment (a shell you do not share)
Set (ideally by sourcing `~/.agency/sales-ingress.env`, `chmod 600`):
```
TELEGRAM_BOT_TOKEN      # from BotFather (SECRET)
TELEGRAM_BOT_USERNAME   # e.g. <your_bot>  (no leading @)
IRONCLAW_API            # the (multi-tenant) instance API base — shared by every client
ACCOUNT_BASE            # http://127.0.0.1:8443
```
**Multi-client mode (the product):** per-client credentials come from the registry
`CLIENTS_DIR` (default `~/.agency/clients/*.env`, one file per client, written by
`multi/provision/provision.sh` — schema in `multi/clients/README.md`). Each client file maps
`TELEGRAM_GROUP_ID → (ACCOUNT_TOKEN, IRONCLAW_TOKEN)`; the bridge routes each group to its
client's thread and credentials.

There is no single-group fallback: an empty registry fails closed. (The legacy
`SALES_GROUP_ID` + env-pair mode was removed — it hand-built a client with no
guidance-validated persona, serving the internal composition to a client group.)
Internal dev flows construct a `ClientConfig` explicitly with `compose_persona()`
(see `context_ingress.py`'s `__main__` hero flow).

## 4. Start the Account Store
```
cd deploy/account-intel/data && ./dev-up.sh          # postgres + Account Service + seed (idempotent)
curl -sf http://127.0.0.1:8443/health   # expect {"ok": true}
```

## 5. (Optional) Start the trusted context ingress in isolation
```
cd multi/seam && python3 context_ingress.py   # runs the hero-flow oracle against backend context
```

## 6. Start the Telegram bridge
```
cd multi/seam && python3 telegram_bridge.py    # long-polls the bot; prints "serving <slug>@<gid>, …"
```
The IronClaw instance is used via its **API** (`IRONCLAW_API`), not its own Telegram extension. No IronClaw-level
pairing — each private group **is** that client's trusted room; the group→client mapping picks the org token and
the sealed IronClaw token; all members of a group share that group's reasoning thread. ONE bot + ONE process
serves every client group (a bot token allows one `getUpdates` poller) — isolation lives in per-thread
credentials and the sealed accounts, and the trade-off (shared SPOF; the bot token reads every client group) is
recorded in the bridge docstring. **Restart the bridge after provisioning or changing a client** — the registry
is read once at startup.

## 7. Verify the bot only responds in registered client groups, each with its own data
- In a registered group: `@<username> ping` → it responds — with THAT client's org only.
- In **any other chat or DM**: send a message → it does **not** respond (`summoned()` requires a registered group).
- Two-client isolation is proven end-to-end by `multi/verify/test_two_clients.py` (11 checks).

## 8. Stop / restart cleanly
- **Stop:** Ctrl-C the bridge. Conversation continuity is **persisted** per group
  (`BRIDGE_STATE`, default `~/.agency/bridge-threads.json`, chmod 600) — a restart resumes every
  client's thread. The Account Store (Postgres) persists across bridge restarts.
- **Restart:** re-run step 6 (env still set). To restart the store: `cd deploy/account-intel/data && ./dev-up.sh`
  (idempotent — client org tokens live in the hot-reloaded `~/.agency/account-identities/identities.json`
  and survive any restart; the env identities are just the dev/demo base).
- **A restart onto a newer seam may refuse to start, once.** If `BRIDGE_STATE` predates
  per-account versioning (`"supplied"` is a list of ids, not a `{id: updated_at}` map),
  `_load_threads` raises rather than coercing it — coercion would derive `ever_supplied=false`
  for a thread that has had context, which nulls `thread.prev` and silently drops a live
  conversation. The raised message prints the exact one-time migration; back the file up with
  `cp -p` first (preserving 0600), run it, start again. Never delete the state file to clear the
  error — that resets every group's `prev`, which is what the refusal exists to prevent.
  Full runbook: `deploy/UPGRADE.md` step 5.3.

## What a live two-human run must show
Run it twice, each with a different sanitized prospect, summoning via `@<your_bot> …` or a reply
to the bot. The shape:

```
Human A: @<your_bot> prepare us for <prospect>   → ingress fetches context → IronClaw briefing
Human B: <adds a new fact or correction>
Human A: @<your_bot> what changed?               → IronClaw reassesses from the shared thread
Human:   Decision: ADVANCE | HOLD | DEPRIORITIZE → acknowledged in prose only
```

What must hold: the agent answers from backend-supplied context rather than generic advice; a
fact one human adds is incorporated for the other; the human's decision stays authoritative; and
no prospect is contacted and no cross-room action is attempted. Record friction as you go, and
do not fix anything mid-run unless it blocks safe operation.

On ADVANCE, the prospect intake is **human-gated and manual**: a human takes the conversation forward
directly. (The per-prospect MultiAgencyHQ instance this flow originally routed to is retired — the
front desk is now the secretary Worker in `deploy/secretary/`.) Internal sales-room context must
**not** cross into any prospect-facing room. Record friction first; do
not add fixes during the live test unless something blocks basic safe operation.

## Terminology (locked)
- **trusted context ingress** / **MultiAgency application adapter** = correct. "orchestrator platform" = **incorrect**.
- **IronClaw owns reasoning and conversation.** **MultiAgency ingress owns trusted private-context supply.**

## Do not add (frozen boundary)
WASM retrieval · public Account Service · model network access · opportunities · persisted assessments ·
lifecycle stages · activity writes · workflow engine · events · work items · autonomous sourcing · web research ·
automated outbound · automatic prospect-room creation · agent-to-agent comms · a second Sales Intelligence agent.
