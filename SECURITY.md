# Security contract

Report vulnerabilities through GitHub's private vulnerability-reporting tab. Do not open a public
issue or include credentials, client records, group IDs, or live host details. If the private tab
is unavailable, contact the repository owner through a known private channel first.

## Trust boundaries

This contract covers the multi-tenant product path. The single-tenant fleet and the Secretary
([`README.md`](README.md) § What else is in this repository) run the pinned runtime but are not
described by the boundaries below.

- Each tenant uses a sealed IronClaw account and organization-scoped Account Service identity;
  cross-organization resources return not found.
- The seam holds both of a tenant's credentials and uses each on one leg only. The Account
  Service receives that tenant's organization credential; IronClaw's HTTP authentication layer
  receives that tenant's sealed member bearer, which is how the sealed-account boundary is
  enforced. Neither credential nor the Account Service address belongs in model-visible content.
- The IronClaw request body carries the selected model, composed instructions and tenant guidance,
  user input, scoped business context, and an optional previous-response identifier. Neither
  credential nor the Account Service address appears in that model-visible body. Instructions,
  input, prior conversation, and model output may persist in IronClaw-managed response/thread
  state; the bridge persists identifiers and delivery metadata only. This repository does not
  prove that IronClaw, a reverse proxy, or surrounding infrastructure never records transport
  authentication headers in operational logs.
- Clients never receive member bearers. Whoever holds one can reverse per-bearer tool settings.
- Provisioning disables non-allowlisted tools and refuses activation if confinement cannot be
  proved.
- The runtime sits on internal-only Docker networks. Provider traffic crosses a CONNECT-only
  gateway with one exact `host:port` allowlist entry. The Account Service uses a separate private
  path.
- Guidance is mandatory, slug-bound, service-bound, and supplied every turn. A registry mismatch
  fails closed.
- A tenant's organization is the one the Account Service authenticates from its credential, not
  the registry's `ORG_ID`, which is operator metadata only. The bridge binds that authenticated
  org at startup and re-checks it on every records read, so a hot-reloaded credential that begins
  resolving elsewhere fails the turn closed rather than degrading to a no-records answer.
- A persisted conversation continues only while the composition it was built under still matches.
  Service, version, composed instructions, model, `FACT_FIELDS` policy, organization scope, and
  Account Service endpoint are bound per conversation; any change refuses continuation until an
  operator explicitly resets it. Reset clears conversation and context continuity and preserves
  the delivery journal.
- The bridge stores routing and delivery identifiers in mode-`0600` SQLite, plus a non-secret
  compatibility identity per conversation: service and version, a SHA-256 of the composed
  model-visible instructions, the effective model, a `FACT_FIELDS` policy hash, and the
  authenticated Account Service organization id and normalized base URL. It stores no message
  text, response text, account records, persona or guidance content, and no credentials — the
  instruction and policy fingerprints are hashes, and the base URL is refused if it carries
  credentials.

## Egress guarantee

Only when `./deploy/ironworks egress status` reports `VERIFIED` may an operator claim that the
runtime has no direct public route and network egress is restricted to the configured model
provider independently of model-visible tools.

```sh
./deploy/ironworks egress status
./deploy/egress/egress-control.sh verify
./deploy/egress/egress-control.sh activate --confirm
./deploy/egress/egress-control.sh rollback --i-accept-unrestricted-egress
```

Activation recreates the runtime. Stop the bridge gracefully, activate, verify runtime health and
containment, restart, and run the isolation proofs. A gateway failure is fail-closed: inference
stops while containment remains. Rollback deliberately restores unrestricted egress and records a
degraded state.

The allowed model provider remains a data sink by design. A prompt-injected turn could encode data
to that provider, though it cannot reach attacker infrastructure through this boundary. Reassess
the policy if the provider host, port, model path, or URL-fetch behavior changes.

## Delivery guarantee

| Property | Guarantee |
|---|---|
| Model execution | At most once, except before a response ID becomes durable |
| Telegram delivery | Complete only when every chunk is acknowledged; otherwise retained for retry or explicit reconciliation |
| Re-delivery | Uses the original stored response and never a second model execution |
| Ordering | Strict within a group; serial across groups |

Nothing is exactly once. Updates move transactionally through `RECEIVED`, `TURN_STARTED`,
`TURN_COMPLETED`, `DELIVERY_STARTED`, `DELIVERED`, and `ACKED`. Response ID and thread pointers
commit together. Recovery fetches a completed response instead of regenerating it.

If a model request may have run but no response ID is durable, the update becomes
`RECOVERY_BLOCKED` and is never replayed. **"May have run" is a claim about evidence, not about
intent**: a returned status line proves the request arrived, and a timeout or a post-connection
failure leaves it ambiguous — both block recovery. Constructing a request object opens no socket
and is not evidence of anything; a failure that proves the request never left the process is an
ordinary failure the tenant can retry. Use `./deploy/ironworks bridge status`; exit `0` is
healthy, `2` unhealthy, and `3` unevaluated. Fix forward after delivery state has processed
traffic unless the store mechanically proves no update or cursor was recorded.

## Pinned-runtime limitations

- **No individual member-session revocation.** Deleting or suspending a member does not
  invalidate its bearer. Custody, residual-authority reporting, and fleet-wide signing-key
  rotation are the current controls. Probe with `multi/verify/test_session_revocation.py`.
- **Built-in HTTP lacks default-deny configuration.** Per-bearer confinement and the network
  boundary compensate. Probe with `test_egress_closed.py`.
- **The settings catalog is not the complete model surface.** Run `test_surface_drift.py` and
  `test_member_admin_negative.py` after every runtime change.
- **No per-account runtime persona.** The seam composes persona and guidance every turn; this is
  the intended boundary.
- **No production-profile extension lifecycle.** Telegram transport and product composition stay
  outside the runtime.

Do not patch or vendor IronClaw to close these limitations. Re-measure them after every pin bump.

## Accepted operational limits

- One bot and bridge process form a shared availability boundary; cross-group work is serial.
- Response retention is externally controlled, so recovery blocks rather than regenerates when a
  stored response cannot be fetched.
- Tool confinement depends on token custody and must be repeated after tool-surface changes.
- Persona governance is empirical: composition and behavior tests must pass after prompt or model
  changes.
- Operator scripts are supported only on single-operator hosts because some request credentials
  may be visible to local process inspection.

Credential rotation, cross-tenant exposure response, and destructive recovery procedures are in
[`deploy/README.md`](deploy/README.md).
