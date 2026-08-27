# Multi-tenant product interface

`multi/` serves organization-scoped tenants through one unmodified IronClaw runtime. Each tenant
has a sealed member, mandatory guidance, a service definition, and Account Service credentials
held only by the trusted seam. Internal and external services follow the same loader and turn path.

## Trusted adapter

The seam may authenticate a group, resolve a tenant, deterministically select bounded records,
fetch organization-scoped context, compose instructions, invoke IronClaw, and deliver the result.
It may not plan, score, choose actions, run model-generated retrieval, persist model-authored
facts, or duplicate the runtime's agent loop.

The IronClaw request contains scoped business facts but no Account Service host or credential.
Registry loading fails closed on missing tokens or guidance, duplicate groups, unknown services,
or slug/service-marker mismatch. Instructions are composed every turn. Registry changes require
a bridge restart, and a change that alters what a persisted conversation was composed under
refuses startup until that conversation is explicitly reset
([`../deploy/README.md`](../deploy/README.md) § Tenant lifecycle).

### Why the adapter exists, and why not a tool extension (do not reopen)

The obvious alternative is a WASM tool extension calling the Account Service directly, so the
model gets named business tools and the host injects the credential. **That is impossible on this
runtime by deliberate upstream design**, and the seam is what replaces it.

The extension sandbox denies private-IP egress unconditionally for any networked WASM capability.
Read at the rev in `IRONCLAW_PIN`:

- `crates/extensions/ironclaw_extension_host/src/capability_surface.rs:203` —
  `deny_private_ip_ranges: has_egress_targets`. Declaring *any* egress target turns the guard on;
  there is no way to declare a target and opt out.
- `crates/substrates/ironclaw_network/src/policy.rs` (`is_private_or_loopback_ip`, plus the
  resolver check) — the transport then refuses any private, loopback or link-local address
  **after DNS resolution**, so a public name pointing at a private IP fails too.
- **No operator exception exists.** Every other assignment of that field upstream is a hardcoded
  `true`; the only computed forms are the two above. It is not settable from config or env, so it
  cannot be turned off without patching core.

The Account Service sits on a private address, so a WASM extension cannot reach it: the request is
blocked before it leaves the sandbox and the service never sees it. Extension egress is built for
**public** API hosts; a private internal service is unreachable *by design*, not by
misconfiguration.

Read the consequence the right way round — this is a **feature the product depends on**. The agent
is deliberately denied authority to reach the operator's private infrastructure; the trusted layer
holds the credential and the private-network reach and hands the agent only scoped context.

The only supported way to make the extension route work would be exposing the Account Service on a
publicly routable endpoint purely to satisfy the guard, trading a real isolation property for a
cosmetic one. Don't. A prototype was built and taken to this exact blocker before the seam replaced
it; these citations are what that cost to learn.

Key files:

- `seam/registry.py`: who may be served — tenant configuration, validated fail-closed, offline.
- `seam/envelope.py`: which records a turn is given, and how they are rendered. No I/O.
- `seam/account_service.py`: the tenant's own records, fetched as the tenant. The only
  place the account token is put on a request, and where org scope is bound and enforced.
- `seam/context_ingress.py`: the IronClaw client and one turn's orchestration.
- `seam/services.py` and `persona.py`: validated ordered composition.
- `seam/redact.py`: shared credential-shape redaction.
- `seam/bridge_state.py` and `bridge_core.py`: SQLite delivery state machine.
- `seam/telegram_bridge.py`: registered-group routing and polling.

## Isolation rules

These hold the tenant boundary on a shared instance. Each has a probe under `verify/`.

- **Never set `IRONCLAW_REBORN_DEV_SECRET__*` on the multi-tenant instance.** That surface is
  **tenant-wide, not per-account**: the runtime resolves a keyed tool's credential caller-first
  and then falls back to a single tenant-shared admin-managed scope, so one seeded secret becomes
  usable by *every* sealed tenant's turns. Per-tenant credentials ride the seam
  (`~/.agency/clients/`), never the instance environment. The env allowlist in
  `instance/docker-compose.yml` is load-bearing and must stay free of them.
  Probe: `verify/test_tenant_shared_secret_probe.py`.
- **Nothing tenant-specific may be seeded into the shared skill and tool catalog**, which is
  identical for every tenant. Probe: `verify/test_catalog_parity.py`.
- **The registry and the instance must agree on who exists.** A member the registry has never
  heard of is invisible to every registry-enumerating tool, including confinement and the egress
  proof. Probe: `verify/test_registry_reconciliation.py`.
- **Tenant-shared mounts are not readable or writable from a member turn**, by alias, traversal,
  or absolute path. Probe: `verify/test_tenant_shared_mount_probe.py`.

## Layout and local checks

- `clients/`: registry and guidance schema; live data is under `~/.agency/clients/`.
- `instance/`: runtime compose definition and environment template.
- `services/`: committed service compositions.
- `provision/`: activation, confinement, and deletion.
- `serve/`: private-host units, watchdog, and backup automation.
- `verify/`: isolation, recovery, and behavior proofs.

```sh
cd multi/seam
python3 test_services.py
python3 test_client_guidance.py
```

## Running the product path locally

Use private, mode-`0600` environment and registry files throughout. The 24/7 deployment is
`serve/`; these steps are for local work.

1. **Bot and group.** Create a bot with BotFather and a private group, then add the bot **as an
   admin** — a basic group with privacy mode on silently drops messages, and admin upgrades it to
   a supergroup so the bot actually receives them. Obtain the group's chat id from a message in
   it. **The `getUpdates` URL contains the bot token; never paste that command or its output
   anywhere shared.**
2. **Account Service.** From `deploy/account-intel/data/`, run `dev-up.sh` (postgres, service and
   seed, idempotent), then `curl -sf http://127.0.0.1:8443/health`. Dev org tokens are injected by
   `dev-up.sh`, not by the compose file; they are identifiers for a loopback-only stack, and
   `prod-up.sh` seeds none and asserts they are dead.
3. **Registry.** Per-tenant credentials come from `CLIENTS_DIR` (default `~/.agency/clients/`),
   one file per tenant, written by `provision/provision.sh`. **There is no single-group fallback:
   an empty registry fails closed.**
4. **Bridge.** `cd multi/seam && python3 telegram_bridge.py`. **Restart it after provisioning or
   changing a tenant** — the registry is read once at startup. It resolves each tenant's
   authoritative organization from the Account Service at startup, and refuses to start if a
   tenant's persisted conversation was composed under a different service, version, instruction
   set, model, `FACT_FIELDS` policy, organization, or Account Service endpoint.
5. **Confirm the boundary.** In a registered group, an @mention answers with that tenant's data
   only. In any other chat or DM it does not answer at all.

**Stopping and restarting.** Ctrl-C and `systemctl stop` are both graceful: the bridge finishes
the update in hand, bounded by `TURN_BUDGET_SECONDS`, and leaves the rest of the batch
unacknowledged for the next process. **Wait for the old process to exit before starting a
replacement** — one bot token allows one `getUpdates` poller, and two overlapping pollers steal
each other's updates while the room receives nothing.

**A restart onto a newer seam may refuse to start, once.** If `BRIDGE_STATE` predates per-account
versioning, the one-time migration raises rather than coercing it: coercion would derive "no
context has ever been supplied" for a thread that has had some, nulling `thread.prev` and
silently dropping a live conversation. The message prints the exact migration — back the file up
with `cp -p` first, preserving its mode, then run it. **Never delete the state file to clear the
error**, which is the outcome the refusal exists to prevent. Operator steps:
[`../deploy/UPGRADE.md`](../deploy/UPGRADE.md).

Broader proof prerequisites and commands are in [`verify/README.md`](verify/README.md).
