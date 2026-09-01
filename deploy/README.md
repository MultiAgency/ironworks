# Operator runbook

This is the current lifecycle and recovery authority for the product and single-tenant fleet.
IronWorks runs official IronClaw at `IRONCLAW_PIN`; it contains no runtime fork.

## Read-only console

```sh
./deploy/ironworks doctor [--offline] [--json]
./deploy/ironworks tenants status
./deploy/ironworks tenant inspect <slug>
./deploy/ironworks bridge status
./deploy/ironworks egress status
./deploy/ironworks account-db migration-status
./deploy/ironworks service validate
./deploy/ironworks release verify [--offline-only] [--with-evidence ARTIFACT.json]
```

Exit `0` means all evaluated checks passed, `2` means failure, `3` means a guarantee could not be
evaluated, and `64` means invalid usage. Blocked is never a pass.

`./deploy/ironworks test` runs the whole local/CI quality gate from here — the same command CI
runs. It is the one subcommand outside the verdict contract above: it reports the gate's own
result, pass or fail, not a console verdict.

### Certifying promotion takes two environments

`release.promotable` requires repository-hygiene gates **and** a proved egress boundary. No single
machine can supply both: the gates need a git checkout, and `/opt/ironworks` is a file copy, so
`git ls-files` fails there; the boundary can only be proved against the running gateway, which CI
does not have. Run each half where it can be answered, and carry the first to the second:

```sh
# 1. in the repository (or CI) — answers what needs a checkout
./deploy/ironworks --json release verify > /tmp/repo-evidence.json
scp /tmp/repo-evidence.json <host>:/tmp/

# 2. on the serve host — answers the boundary, and reads the rest from the evidence
./deploy/ironworks release verify --with-evidence /tmp/repo-evidence.json
```

Evidence is accepted **only** when both artifacts carry the same `tree_fingerprint`
(`deploy/lib/tree_identity.py` — a content hash over the tracked files, computed from
`git ls-files` in a checkout and from `DEPLOYED_MANIFEST.sha256` on a deployed copy, re-hashing
the files on disk either way so a post-deploy edit moves it). It fills only checks the local run
could not evaluate, never overwrites a local FAIL, and imports only a PASS. A mismatch is refused
rather than reconciled: combining a green repository against one commit with a boundary proved on
another would certify a release that never existed.

The `live.*` legs stay BLOCKED until someone runs them, which costs model calls — so exit `3`
after a successful composition is normal and means "promotion is certified, these proofs are still
unrun", not "something is broken".

## Loading a tenant's records

A tenant's book is staged **before** provisioning, outside the repository, and provisioning seeds
it as step 2 of 5. Put one JSON file per account under `~/.agency/account-data/<slug>/`, in the
candidate shape — `record_id`, `account{}`, `contacts[]`, `activities[]`, matching
`deploy/account-intel/data/schema.sql` and the committed examples under
`deploy/account-intel/data/candidates/`. `provision.sh` then calls
`deploy/account-intel/data/seed-real.sh`, which registers the org identity and seeds the rows.

A tenant with no staged directory is still valid: the org is created empty, because org existence
is what scopes rows. The service tells the team the book is empty rather than inventing content.

Records are facts and guidance is policy; they are separate files with separate approval paths and
must not be merged. Real records never enter the repository. To load or correct a book after
activation, stage the files and run `seed-real.sh <slug>` directly; to mint an org credential
without data, use `deploy/account-intel/data/register-identity.sh`.

## Tenant lifecycle

Prepare approved slug-and-service-bound guidance and private source records, then:

```sh
multi/provision/provision.sh <slug> "<display name>" <group-id> --service <service> --dry-run
multi/provision/provision.sh <slug> "<display name>" <group-id> --service <service>
```

Use `--status` and `--resume` after interruption. Never hand-promote a staged registry entry.
Restart the bridge after a registry change, inspect the tenant, and verify one own-organization
reply plus silence in an unregistered chat. A restart is what picks a registry change up, but it
is no longer sufficient on its own: a change that alters what a tenant's persisted conversation
was composed under refuses startup until it is reset, as below.

Persisted conversations are bound to the service, service version, full SHA-256 of the exact
model-visible composed instructions, effective model, and a context-policy hash covering
`FACT_FIELDS`, plus the Account Service's authenticated organization id and normalized base URL.
The bridge resolves the org from `/list_accounts` at startup; registry `ORG_ID` remains metadata.
A bridge restart with the same identity resumes the existing response chain. A service/version,
persona, guidance, safety-tail, model, `FACT_FIELDS`, organization-scope, or Account Service
endpoint change refuses startup rather than continuing under incompatible history; stripped
guidance comments and same-org token rotation do not change the identity.

The token map is hot-reloaded, so the seam also re-checks the returned org on every Account
Service read. A live token-to-org repoint is a compatibility failure, never a records-outage
degradation — the bridge refuses the turn rather than serving it with no records.

Stop the bridge, wait for its process to exit, inspect the stored and intended non-secret
identities, then explicitly reset:

```sh
./deploy/ironworks tenant reset-thread <slug>
./deploy/ironworks tenant reset-thread <slug> --confirm <slug>
```

The first command reports the mismatch and exact confirmation command without resetting. The
confirmed command clears the conversation pointer, supplied-context versions, freshness/orphan
state, timestamps, and old compatibility identity. It preserves the delivery journal and Telegram
cursor. `drop_thread` remains the deprovisioning operation and destroys the journal too.

**One residual limitation.** The Account Service exposes no stable service-instance or data-set
identifier, so the normalized base URL is the closest available endpoint identity. IronWorks
therefore cannot detect a different backend or a replaced data set appearing behind an unchanged
URL: the org id still matches, the base still matches, and the conversation continues over records
that are no longer the ones it was built on. Repointing an Account Service base at different data
is an operator action that must be paired with an explicit reset, because nothing will catch it.

Deletion is deliberately explicit:

```sh
multi/provision/deprovision.sh <slug>
multi/provision/deprovision.sh <slug> --execute --confirm <slug>
```

Preserve the reported residual authority, restart the bridge, and run `doctor`. Backups are not
rewritten; after restoring a pre-deletion snapshot, repeat deletion before serving.

## Fleet-agent handoff

The single-tenant fleet is a supported adjunct and shared operational tooling, not the canonical
IronWorks product path ([`../README.md`](../README.md) § What else is in this repository). A fleet
agent is one instance, volume, bot, and baked persona; it has no tenant registry entry, no
organization scope, no guidance binding, and no service definition, and the boundaries in
[`../SECURITY.md`](../SECURITY.md) do not describe it. It shares the pins, `deploy/lib/`, and the
upgrade runbook, which is why it lives here.

A human supplies and approves:

```text
name:       persona name
slug:       lowercase infrastructure identifier
audience:   who may use it
purpose:    one-line job
boundaries: confidentiality, prohibited actions, and escalation
tone:       voice and formality
tools:      required capability beyond chat; default none
origin:     source of the request
```

The operator drafts a persona, obtains approval, and provisions it:

```sh
TELEGRAM_BOT_TOKEN="$(cat ~/.agency/<slug>.token)" \
TELEGRAM_BOT_USERNAME=<bot> PROVISION_FROM_ENV=1 \
PERSONA_SOURCE=agent/identity/<PERSONA>.md AGENT_NAME=<Name> PURPOSE="<purpose>" \
  ./deploy/provision-agent.sh <slug>

./deploy/doctor.sh <slug> --deep
```

`PROVISION_FROM_ENV=1` prevents accidental inheritance from another agent. A human performs every
stage; agents do not provision, edit, or message other agents. Never replace a fleet volume's
persisted secret-store master key.

## Runtime pin upgrade

The procedure of record is [`UPGRADE.md`](UPGRADE.md). Follow it start to finish; it carries the
delta-reading method, the entrypoint and single-writer invariants, the state-compatibility
measurement, the ordered replacement, and the full proof list.

Four things it is worth knowing before you open it:

- A pin change is not complete until confinement and containment are **re-proved**. Between the
  build and a passing `multi/verify/test_egress_closed.py`, every member may hold network
  authority it did not have yesterday, and nothing else will tell you.
- The egress verification stamp is bound to the image id, so a rebuild invalidates it by design.
- Do not resume external service until `./deploy/ironworks egress status` reports `VERIFIED`.
- Roll back only before new durable state is written, or after compatibility is mechanically
  established. Bridge delivery state normally requires fixing forward.

## Incident response

The procedures of record are [`../docs/INCIDENT_RESPONSE.md`](../docs/INCIDENT_RESPONSE.md) —
eight scenarios, each stating its blast radius before its steps, with the exact commands. Read it
rather than improvising from the summary below.

Four rules that govern all of them:

- **Stop serving before diagnosing** suspected cross-tenant exposure or an active credential leak.
  The bridge is the only path from a group to a tenant's credentials.
- **Deleting a member is not revoking its bearer.** The only immediate global containment is
  rotating the operator token, which is the session signing key and takes every tenant down until
  re-provisioned. Weigh that against what the leaked credential reaches, and record the decision
  either way.
- **Recreate through the egress overlay.** Recreating from the base compose file alone is what
  `egress-control.sh rollback` does, so an emergency recreate that omits it silently removes
  containment.
- **Fail-closed behaviour is not an outage to fix by removing the control.** A stopped gateway
  means inference stops while containment holds; that is the design working.

## Host recovery and backups

Restore repositories at a named reviewed revision, private mode-`0600` configuration, database
dumps, and `~/.agency` before starting writers. Start Account Service, contained runtime, then
bridge; require pin, doctor, egress, bridge, backup-timer, watchdog, isolation, and real-reply
checks.

Serve-host backups use `multi/serve/multi-backup.sh` and private `~/.agency/backup.env`. Keep the
repository and password separate, escrow the password, and regularly restore into a fresh
temporary target. A snapshot listing is not a restore test.

Supporting ownership: `account-intel/` is the private data service, `egress/` is the network
boundary, and `lib/` contains shared operator libraries. `secretary/` is MultiAgency's own front
desk — a separate application on the pinned runtime, not part of the product path.
