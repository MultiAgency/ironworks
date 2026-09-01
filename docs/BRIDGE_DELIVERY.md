# Bridge delivery semantics

This document owns the current delivery guarantee, recovery behavior, rollback rule, and health
semantics for the Telegram bridge. The implementation is in `multi/seam/bridge_core.py` and
`multi/seam/bridge_state.py`.

## Guarantees

| Property | Guarantee |
|---|---|
| Model execution | At most once under bridge-controlled replay; the outcome may be unknowable if the process dies before a response ID is durable |
| Telegram delivery | Complete only when every chunk is acknowledged; otherwise retained for retry or explicit reconciliation |
| Re-delivery | Always uses the original stored response; acknowledged chunks may duplicate after explicit partial-delivery reconciliation |
| Ordering | Strict within a group; bounded-concurrent across groups (default 4, maximum 16) |

Nothing is exactly once. Telegram acceptance and the local delivery record cannot be committed
in one transaction, so a crash between them can produce an identical duplicate.

## State transitions

```text
RECEIVED -> TURN_STARTED -> TURN_COMPLETED -> DELIVERY_STARTED -> DELIVERED -> ACKED
    |             |                |
    |             |                +--> DELIVERY_RETRY      (answer retained; first chunk rejected)
    |             |                +--> DELIVERY_RECONCILE  (answer retained; delivery uncertain)
    |             +--> RECOVERY_BLOCKED   (a turn may have run and cannot be recovered)
    |             +--> FAILED_TERMINAL    (a stable pre-model failure)
    +--> IGNORED                          (not addressed to us, or not a registered group)
```

The mode-`0600` SQLite store at `BRIDGE_STATE` holds routing and delivery identifiers, response
IDs, thread pointers, stable error codes, attempts, cursors, timestamps, and non-secret
compatibility identity: service, version, full instructions SHA-256, model, and `FACT_FIELDS`
policy SHA-256, plus the authenticated Account Service organization id and normalized base URL.
It does not store message text, response text, account records, persona or guidance content, or
credentials. The Account Service exposes no stable instance id, so its normalized configured base
is the conservative endpoint/trust-domain boundary; a same-org token rotation does not affect it.
Because token mappings hot-reload, every Account Service read must still report the startup-bound
organization; a changed org fails closed before a model call rather than entering degraded mode.
Registry `ORG_ID` is human/operator metadata and does not protect continuity: the persisted
compatibility identity uses only the organization returned by the authenticated Account Service.

Known limitation: IronWorks cannot distinguish a different backend or data set appearing behind
an unchanged Account Service URL because the service exposes no stable instance identifier.

The response ID and thread pointers become durable in one transaction at `TURN_COMPLETED`.
`ACKED` means Telegram has accepted an offset beyond the update; compaction must not treat local
delivery alone as acknowledgment.

Every fetched batch is journaled before its tenant workers start. A completed later tenant can
therefore run and deliver while an earlier tenant is slow, but the global Telegram cursor stops at
the earliest unfinished journal row. Set `BRIDGE_MAX_WORKERS` to bound cross-tenant concurrency;
updates for one group always remain on one serial queue.

## Recovery

- A `TURN_STARTED` update without a durable response ID splits on ONE question: did the request
  reach the instance? A turn exception that **proves the request never left this process** — a
  refused connection, an unresolvable host, an unreachable network — is an ordinary
  `FAILED_TERMINAL`. The tenant is told once and can simply ask again; no model ran, so a retry
  cannot bill a second one, and it must not consume an operator's reconciliation. Every other
  outcome is `RECOVERY_BLOCKED`: any HTTP status line is proof the request arrived, and a timeout,
  a reset after the connection was established, or a 5xx that survives the retries leaves the
  execution **ambiguous**. The bridge does not risk a second model run against an unknown first
  one. Ambiguity counts as arrival, never the reverse.
- A `TURN_COMPLETED` or `DELIVERY_STARTED` update is recovered by fetching its stored response,
  not by regenerating it.
- A crash after Telegram accepts a reply but before `DELIVERED` is recorded may resend the same
  bytes.
- A deterministic rejection before Telegram accepts the first chunk becomes `DELIVERY_RETRY`.
  The response ID is retained without automatic retry; explicit redelivery fetches the stored
  response and never runs the model.
- A timeout, disconnect, server failure, or failure after any acknowledged chunk becomes
  `DELIVERY_RECONCILE`. Both delivery states advance the cursor so other tenants continue, but
  their response IDs are never compacted and `bridge status` stays unhealthy until an operator runs
  `./deploy/ironworks bridge redeliver <update-id> --confirm <update-id>` with the bridge stopped.
  That command may duplicate already-accepted chunks; it fetches the original response and cannot
  execute a model turn.
- `DELIVERED` updates are not resent merely because Telegram redelivers the update before the
  next offset is acknowledged.
- Corrupt or unknown-version state fails closed rather than being guessed or coerced.
- A persisted conversation continues only when every compatibility field matches the current
  tenant. Active v1/legacy rows have no attributable composition and fail closed. Empty legacy
  rows may bind automatically because they carry no conversation or supplied context.

`RECOVERY_BLOCKED` is terminal. Inspect the error code and decide whether the tenant needs to
repeat the request; do not edit the journal to force replay.

## Rollback

After the current bridge has processed traffic, fix forward. Do not start an older bridge against
delivery state it does not understand. Rollback is safe only when the current SQLite store
mechanically proves that it has processed no updates and has no cursor; uncertainty means roll
forward.

The current schema is **v3**. Two upgrades are recognized and both only add compatibility columns,
never rewriting thread or delivery rows: v1 gains all seven identity columns, and a *complete*
historical v2 gains the two v3 ones (`organization_id`, `account_service_base`). Before either,
the store writes a mode-`0600` SQLite backup beside the database named for the version being
**left** — `<db>.v1.bak-<UTC timestamp>` or `<db>.v2.bak-<UTC timestamp>` — and records its path
in `schema_v1_backup` or `schema_v2_backup`. Active migrated rows keep NULL compatibility identity
and refuse continuation until explicitly reset; nothing guesses what created them. Old code
refuses a newer schema, so a code rollback requires restoring the backup for the version that code
understands. There is no reverse migration.

**"v2" named two different shapes, and the difference is operational.** `organization_id` and
`account_service_base` were added without bumping the version, so a database written by that
earlier code is a legitimate `schema_version=2` carrying only five identity columns. It is
upgradeable and migrates normally. A database stamped `2` with *fewer* than v2's five columns is
not a v2 at all whatever its stamp says: it is refused as internally inconsistent rather than
repaired into a shape nothing ever wrote. That refusal used to advise restoring a v1 backup — which
a database born at v2 does not have — so read the refusal's own text, which names the version the
file claims and the backup it actually recorded.

## Operations

```sh
./deploy/ironworks bridge status
./deploy/ironworks bridge redeliver <update-id> --confirm <update-id>
./deploy/ironworks tenant reset-thread <slug>
./deploy/ironworks tenant reset-thread <slug> --confirm <slug>
./deploy/ironworks test
python3 multi/seam/test_thread_compatibility.py
python3 multi/verify/test_responses_recovery.py
```

For `bridge status`, exit `0` is healthy, `2` is unhealthy, `3` is unevaluated, and `64` is
invalid usage. A blocked or unevaluated result is not a pass. Stop the bridge gracefully before
maintenance so an in-flight turn can finish within its configured budget.
