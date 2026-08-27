# Incident response

Procedures for the failures that have no in-product remedy. Everything here is destructive or
disruptive by design, so each one states its blast radius before its steps.

Routine operations are not here: provisioning is `multi/provision/provision.sh`,
deprovisioning is `multi/provision/deprovision.sh`, health is `./deploy/ironworks doctor`, and
a pin bump is `deploy/UPGRADE.md`.

---

## 1. A sealed member token has leaked

**Trigger.** A registry `.env` was copied off the host, pasted into a chat, committed, read by
a third party, or handled by anyone who is not the operator. Also: a laptop with
`~/.agency/clients/` on it was lost.

**Why this is the emergency it is.** Whoever holds a member token holds that tenant's agent
scope, and can re-enable `builtin.http` on their own bearer — per-bearer tool state is
reversible by the bearer — which restores wildcard public egress and lets a turn exfiltrate
that tenant's private book. Handing out a token does not weaken the confinement, it voids it
(`docs/ARCHITECTURE.md` § Isolation and composition).

**What does NOT work, and why.** Deleting the user does not revoke the token. Suspending the
user does not revoke the token. There is no route that revokes it. All three are measured, not
assumed — `docs/IRONCLAW_RUNTIME_CONSTRAINTS.md` § `ironclaw-1`, and
`multi/verify/test_session_revocation.py` re-measures them on demand. **Do not "deprovision and
move on."**

### The only immediate global containment: rotate the session signing key

The signing key is derived from the operator token: `serve.rs` builds
`session_signing_secret` from the resolved `IRONCLAW_REBORN_WEBUI_TOKEN` value, and
`SignedTokenSessionStore::from_operator_secret` hashes it with the tenant id into the HMAC key.
So **changing `WEBUI_TOKEN` invalidates every session token on that instance at once** — the
leaked one, and every other tenant's.

**Blast radius: every tenant on the instance stops working until re-provisioned.** This is a
full-fleet outage, deliberately. Weigh it against what the leaked token can reach.

1. **Decide, and write down why.** One tenant's leak costs every tenant a re-provision. If the
   leak is contained (a token that never left the host, a file with a bad mode and no evidence
   of access), the ledger + expiry route may be the right call — record that decision.
2. **Stop the bridge.** `sudo systemctl stop bridge`. Clients are down from here.
3. **Mint a new operator token and rotate it** in `multi/instance/.env` (`WEBUI_TOKEN`), then
   recreate the instance. **Recreate through the egress overlay, or the runtime comes back on a
   routed network** — recreating from the base compose file alone is exactly what
   `egress-control.sh rollback` does, so an emergency would silently remove containment:
   ```
   docker compose -f multi/instance/docker-compose.yml \
                  -f deploy/egress/docker-compose.egress.yml \
                  up -d --force-recreate ironclaw
   ```
   Keep the old value until step 6 confirms the rotation took.
4. **Confirm every old session is dead.** Any tenant's previous member token must now be
   refused: it fails the HMAC check under the new key.
   ```
   curl -s -o /dev/null -w '%{http_code}\n' \
     -H "Authorization: Bearer <an old member token>" \
     http://127.0.0.1:3020/v1/responses/resp_00000000000000000000000000000000
   # 401 = the rotation took. 404 = it did NOT — the bearer is still accepted; stop and diagnose.
   ```
5. **Re-provision every tenant's sealed member.** Each needs a new member, a new confinement,
   and a new registry entry. Run `multi/provision/provision.sh` per tenant; it is transactional,
   so a partial failure compensates rather than leaving a half-tenant.
6. **Prove the fleet is back.** `./deploy/ironworks doctor` must return 0, and
   `./deploy/ironworks tenants status` must show `auth accepted` for every tenant. There are
   four verdicts, not two: `rejected` (HTTP 401/403 — the instance refused the token) is a FAIL,
   but `unreachable` (nothing answered) and `undetermined` (the instance answered something that
   never reached the auth decision — a 5xx, a proxy's 502, a 429) both report **BLOCKED** and
   exit 3. Neither is green and neither is a refusal: they say the token was not measured. Do not
   read one as a pass or re-provision on one as though it were a refusal — settle the instance
   first, then re-probe. Recreating
   the runtime in step 3 also touches the egress boundary: confirm `./deploy/ironworks egress
   status` still reports **VERIFIED** before letting clients back in.
7. **Restart the bridge**, then confirm one real reply in each tenant group.
8. **Record it.** The leaked token's slug, when it leaked, what it could reach, the rotation
   time, and the tenants re-provisioned. `~/.agency/residual-authority.json` is not the record
   for this — a rotation makes those entries moot, so drop them explicitly
   (`python3 deploy/lib/lifecycle.py residual drop <slug>`) once step 4 passes.

### If you decide NOT to rotate

Then the honest statement is: *that token authenticates until its recorded expiry, and the
containment is that nobody else is believed to hold it.* Keep the ledger entry, keep
`./deploy/ironworks doctor` red, and do not describe the tenant as "revoked" in any report.

---

## 2. The operator token has leaked

Strictly worse than (1): the operator token **is** the signing key, so a holder can mint a
valid session for any user id, read across accounts, and re-enable any tenant's tools.

Same procedure as (1), with no "decide not to rotate" branch. Rotate immediately, then audit
`~/.agency/clients/` file modes and every copy of `multi/instance/.env` (including host
backups, which carry the secret master key and the ciphertext together — see the header of
`multi/instance/docker-compose.yml`).

---

## 3. A tenant is receiving another tenant's data

**Stop serving first, diagnose second.**

1. `sudo systemctl stop bridge` — the bridge is the only path from a Telegram group to a
   tenant's credentials, so stopping it stops the exposure.
2. Capture the evidence before changing anything: the group id, the reply, and
   `./deploy/ironworks tenants status --json`.
3. The three things that can produce this, in the order worth checking:
   - **A duplicate `TELEGRAM_GROUP_ID`** across two registry entries. `load_clients` fails
     closed on this, so it should be impossible to load — but check the file set directly.
   - **A cross-wired registry entry** (one tenant's `ACCOUNT_TOKEN` beside another's
     `IRONCLAW_TOKEN`). `./deploy/ironworks tenant inspect <slug>` shows which org each
     tenant's token resolves to.
   - **A shared credential surface on the instance.** Confirm the MT container carries no
     `IRONCLAW_REBORN_DEV_SECRET__*` (`multi/README.md` § Isolation rules) —
     `multi/verify/test_tenant_shared_secret_probe.py`.
4. Do not restart the bridge until `multi/verify/test_two_clients.py` and
   `multi/verify/test_adversarial_cross_org.py` both pass against the live instance.

---

## 4. The Account Service is down

Not an emergency: the seam degrades deliberately. `_catalog_or_degraded` serves the turn with
no records and tells the model that records are briefly unavailable, so clients get a working
chat and an honest caveat rather than a stack trace.

What to check, in order: `./deploy/ironworks doctor` (which separates "instance healthy" from
"account service healthy"), then the service log for a `health check failed ref=…` line — the
`ref` in the client-visible 500 body matches it. The body deliberately carries no detail; the
log has it.

If the identities file is the problem, the service keeps the **last good map** rather than
401-ing every tenant at once, and logs why. That is by design; fix the file and it reloads on
the next request with no restart.

**A credential that now resolves to a different organization is not this failure.** Because the
identity map hot-reloads, a token can begin answering for another org while the service is
perfectly healthy. The seam re-checks the authenticated org on every read and fails that turn
closed; it does not degrade to a no-records answer, because serving one organization's room under
another's scope is the outcome degradation would hide. Treat it as §3, not as an outage.

---

## 5. The bridge will not start after a deploy

**Three causes, one symptom.** All show `activating, never active`, because `bridge.service` is
`Restart=always` with `RestartSec=5`, so a refusal at startup is a five-second retry loop rather
than a clean stop. `systemctl is-active` cannot tell them apart. **The journal can, and they want
different responses — one is "wait", the other two are "do something".** Read the line before
acting:

```
systemctl is-active bridge     # -> activating, never active   (both causes)
journalctl -u bridge -n 20     # -> the line below decides which
```

**A. `cannot reach … Refusing to serve` — the instance is not answering.** The commoner one.
Before serving, the bridge verifies every tenant's token is a sealed member and not the operator,
by asking the runtime rather than its own environment
(`context_ingress.assert_no_member_is_the_operator`). It cannot ask an instance that is down, and
an unverified identity is not a verified one, so it fails closed.

*Do nothing to the bridge.* Bring the instance up — `curl -sf http://127.0.0.1:3020/api/health` —
and the next retry starts it on its own. Expect this after a host reboot (docker being up is not
the container being ready) and during any deploy that recreates the runtime, including step 3 of
§1 above. Restarting the bridge by hand changes nothing; it is already retrying.

**B. `ValueError: … pre-versioning 'supplied' list` — the one-time state migration.** Deliberate:
the migration into the durable store REFUSES a `~/.agency/bridge-threads.json` written before
per-account versioning rather than coercing it, because coercing silently discards a live group's
conversation.

The error prints the exact migration. Back the file up preserving its mode
(`cp -p ~/.agency/bridge-threads.json{,.bak-$(date -u +%Y%m%dT%H%M%SZ)}`), run it, restart.
**Never delete the state file to clear the error** — that resets every group's `prev`, which is
the outcome the refusal exists to prevent. Operator procedure: `deploy/UPGRADE.md`, under the
bridge step of the bump.

**C. `persisted conversation is incompatible (…)` — the deploy changed what a live conversation
was composed under.** Not a fault: the bridge binds each conversation to its service, version,
composed instructions, model, `FACT_FIELDS` policy, authenticated organization, and Account
Service endpoint, and refuses to continue one under history it no longer matches. The message
names the categories that changed and the exact reset command.

Decide whether the change was intended. If it was — a persona edit, a guidance revision, a service
move, an Account Service repoint — stop the bridge, wait for the process to exit, and reset that
tenant: `./deploy/ironworks tenant reset-thread <slug>` to see the mismatch, then the same command
with `--confirm <slug>`. The conversation restarts; the delivery journal survives. If it was *not*
intended, the refusal has just caught a misconfiguration — revert it rather than resetting, or
you will paper over the thing that changed. **Never reset to clear the error before you know which
of the two it is.** A same-organization token rotation does not trip this; a cross-org repoint or
an endpoint change does.

Migrated v1 rows carry no recorded composition and refuse for the same reason on the first start
after the schema bump. That is expected once, per active conversation.

A fourth possibility, if none of those lines appears: `TELEGRAM_BOT_USERNAME is not set`, which the
bridge also refuses to start without — without it the bot matches no @mention and would run
"healthy" while deaf in every group.

---

## 6. A client says the agent answered twice, or lost their question

Both are expected in narrow, documented circumstances, and the bridge records which one
happened. Start with:

```
./deploy/ironworks bridge status            # forward progress, in-flight work, blocked updates
```

- **Two identical answers.** The residual duplicate-delivery crash window: the bridge crashed after
  Telegram accepted the message but before it recorded that. The duplicate is byte-identical
  and cost no second model call. Nothing to repair.
- **Two DIFFERENT answers to one question.** This must not happen any more. If it does, it is a
  regression in the delivery state machine — capture `bridge status --json`, the group id and
  the approximate time, and treat it as a defect rather than an incident.
- **"I lost track of that request while restarting."** A `RECOVERY_BLOCKED` update: a turn may
  have run and the pinned runtime offers no way to find out
  (`docs/IRONCLAW_RUNTIME_CONSTRAINTS.md`, and
  the measurement in `multi/verify/test_responses_recovery.py`). The bridge deliberately did not
  run a second one, and the client was told to wait for operator reconciliation before repeating
  it. Inspect the stored error code and decide explicitly whether a new request is appropriate;
  do not edit the journal to force replay (`docs/BRIDGE_DELIVERY.md`, Recovery).
- **No answer, with `DELIVERY_RECONCILE` in bridge status.** The original model result is stored,
  but Telegram delivery was partial or uncertain. Stop the bridge and run
  `./deploy/ironworks bridge redeliver <update-id> --confirm <update-id>`. This may duplicate a
  chunk Telegram already accepted, but it cannot execute the model again.

## 7. The agent stopped answering after the egress boundary was applied

**This is fail-closed working, not an outage to fix by removing the control.**

```
./deploy/ironworks egress status
```

- **FAILED, "gateway is not running"** — the runtime is intact and contained; it simply cannot
  reach the model provider. Start the gateway: `docker compose -f multi/instance/docker-compose.yml
  -f deploy/egress/docker-compose.egress.yml up -d gw`.
- **FAILED, "attached to routed network(s)"** — the runtime was recreated without the overlay.
  Re-apply: `./deploy/egress/egress-control.sh activate --confirm`.
- **RUNNING but not VERIFIED** — the boundary is in place and unproved. Prove it:
  `./deploy/egress/egress-control.sh verify`.
- **Turns fail while the gateway is up** — check the gateway's decision log for a DENY naming a
  destination you did not expect (`docker logs <gateway>`). A provider change would show as a
  denied hostname. Update the allowlist deliberately, and re-read
  `docs/EGRESS_CONTAINMENT.md` § Residual risk before widening it.

**Rolling back removes a security control.** It requires
`./deploy/egress/egress-control.sh rollback --i-accept-unrestricted-egress` and records a
degraded state that `doctor` reports until the boundary is restored. Prefer an unserved client
over an exfiltratable book: the first is recoverable.

## 8. A provisioning run died halfway

`multi/provision/provision.sh` compensates on failure and prints a residual-authority report,
so the usual answer is "read what it said." If the process was killed outright (a closed
laptop, an OOM) the trap did not run and the journal is the record:

```
./multi/provision/provision.sh <slug> --status
```

A tenant whose registry entry is still **STAGED** was never servable — the seam's `*.env` glob
does not descend into `.staging/`. Continue it with `--resume`, or tear it down with
`deprovision.sh`, which reads a staged entry through the same path as a live one.
