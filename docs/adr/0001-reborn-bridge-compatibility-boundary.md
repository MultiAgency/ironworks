# ADR 0001: Keep the bridge as a temporary Reborn compatibility adapter

- **Status:** Accepted
- **Date:** 2026-08-26
- **Scope:** Multi-tenant Telegram product path
- **Decision owners:** IronWorks maintainers
- **Spike plan:** [`docs/adr/0001-spike-plan.md`](0001-spike-plan.md)

## Context

IronWorks is the operator-run application and lifecycle layer above official, unmodified
IronClaw. The current Telegram bridge also implements generic channel and runtime mechanisms that
current IronClaw Reborn is designed to own. That overlap is intentional only where stock Reborn
cannot yet express IronWorks' organizational semantics.

This decision was re-established against:

- the deployed pin in `IRONCLAW_PIN` (`70795c16e`, IronClaw v1.3.0); and
- current upstream `main` at `8dc5958a1d80c84531943e494b22bd233c81033f` on 2026-08-26.

Current upstream has a production-wired Telegram extension, verified webhook ingress, generic
idempotent admission, typed thread/turn/run state, and a host-owned delivery coordinator. It does
not, however, reproduce the product path's organizational conversation model:

- Telegram shared-conversation admission is presence-based: any group containing the bot is
  eligible once the participant is paired.
- Each shared-channel ping is bound to a fresh, pinger-owned ephemeral thread. Successive pings in
  one Telegram group do not form one durable organizational conversation.
- Busy-thread steering is not an ordered queue of separate organizational turns.
- The internal `BeforeInboundPolicy` port may allow, rewrite the user message, or reject, but
  cannot add a distinct host-authoritative context lane. Stock production composition does not
  expose a configurable dynamic `HostIdentityContextSource` either.
- The delivery coordinator safely records ambiguous and partial outcomes, but stock operator
  surfaces do not provide IronWorks' explicit, model-free reconciliation and redelivery workflow.

IronWorks currently supplies the missing semantics in `multi/seam/telegram_bridge.py`,
`multi/seam/bridge_core.py`, and `multi/seam/bridge_state.py`: exact group admission, one persistent
thread per group, per-group serialization, per-turn Account Service composition, and explicit
delivery reconciliation.

## Decision

The IronWorks bridge is a **temporary compatibility adapter, but it is not currently replaceable
by stock Reborn without changing IronWorks' organizational semantics**.

Maintain the bridge for correctness only. Do not add new generic runtime, channel, turn,
delivery, memory, network, credential, or automation mechanisms to it. When a generic mechanism
is absent, preserve the compatibility boundary rather than forking IronClaw, proposing speculative
upstream work, or adding another generic IronWorks implementation. Re-evaluate only after an
official IronClaw release materially changes the relevant behavior.

IronWorks retains ownership of the organizational policy above those mechanisms:

| Responsibility | Long-term owner |
|---|---|
| Group-to-organization admission rules and registry | IronWorks policy |
| Authoritative organization and Account Service scope | IronWorks |
| Service definition, guidance, and business-context composition | IronWorks |
| Tenant provisioning, deprovisioning, readiness, and operator proofs | IronWorks |
| Telegram protocol, webhook verification, normalization, and generic deduplication | IronClaw |
| Rendering, chunking, delivery attempt persistence, and bounded retry | IronClaw |
| Typed thread/turn/run state, model results, execution recovery, and memory | IronClaw |
| Runtime HTTP and runtime/extension credential mediation | IronClaw |
| Infrastructure containment around the runtime | Both, protecting different boundaries |

An internal Rust trait or test seam does not satisfy this decision. A gate is met only when the
official, unmodified shipping runtime exposes a supported production extension point that
IronWorks can configure or implement without patching or vendoring IronClaw.

Current-upstream reconnaissance has now exercised the first architectural boundary:

- S1 is **`FAIL — REQUIRES UPSTREAM CHANGE`**; and
- S2 is **`BLOCKED — BRIDGE MUST REMAIN`**.

These results are authoritative until re-established against a later upstream revision. Do not
proceed to S3 or begin bridge deletion. The bridge remains frozen to maintenance/correctness work
only, and its deletion is a conditional opportunity rather than planned work. Internal,
application-neutral compatibility records are retained for
[shared-conversation admission](../upstream-proposals/shared-conversation-admission.md) and
[managed shared-conversation authority](../upstream-proposals/organizational-conversation-authority.md).
Both are `UPSTREAM GAP — NO ACTION PLANNED`; `BRIDGE REMAINS REQUIRED`. They do not authorize issue
submission, maintainer coordination, a fork, or implementation planning.

## Bridge-deletion gates

The bridge may be deleted only after all six gates are proved against the candidate upstream rev:

1. **Exact admission:** an authenticated Telegram group resolves to exactly one allowed
   IronWorks organization, and unknown, duplicate, stale, or mismatched bindings fail before turn
   creation.
2. **Stable organizational conversation:** every accepted ping for the group resolves to one
   durable organizational thread, without granting one human participant another participant's
   personal authority.
3. **Ordered admission:** accepted group messages become separate turns in arrival order; none is
   merged into an active run as steering, rejected merely because the thread is busy, or silently
   dropped.
4. **Trusted per-turn context:** current Account Service records are loaded on every turn under
   the authenticated organization and enter a bounded, host-authoritative prompt lane—not the
   user message and not untrusted conversational memory.
5. **Operator reconciliation:** ambiguous or partial channel delivery can be inspected and
   explicitly settled or redelivered from the already-persisted model result without running the
   model again, with an immutable audit trail.
6. **Deliberate migration:** the old bridge is quiesced and drained, unresolved rows are settled,
   existing SQLite state is archived, and native Reborn begins from an explicit cutover boundary.

Passing five gates is not sufficient. No gate may be waived by describing current behavior as
"close enough."

## State migration decision

Do not mechanically translate bridge SQLite rows, `previous_response_id` values, context-version
maps, or delivery states into Reborn thread state. Those identifiers have different authorities
and lifecycle contracts.

At cutover:

1. stop new bridge admission;
2. drain or explicitly settle every non-terminal update and delivery row;
3. record an archive manifest and integrity digest for the SQLite database;
4. retain the archive read-only for the incident-retention period;
5. start a new Reborn organizational thread at a named cutover boundary; and
6. inject an operator-approved handoff summary only if gate 4 provides the trusted lane for it.

Rollback may reactivate the archived bridge state only before native Reborn accepts production
traffic. Once both sides have accepted turns, automatic rollback would create two conversation
authorities and is forbidden.

## Consequences

### Positive

- IronWorks stops treating temporary runtime duplication as differentiated product architecture.
- Current delivery and isolation guarantees remain intact while the upstream gaps are real.
- Internal gap records stay generic; Account Service schemas and MultiAgency policy do not leak
  into IronClaw.
- Deletion becomes evidence-driven rather than date- or preference-driven.

### Cost

- Bridge implementation, tests, service rendering, and watchdog code remain until all gates pass.
  Their deletion is conditional and must not be scheduled while S1 or S2 remains unsatisfied.
- The current polling and SQLite operational burden continues during the compatibility period.
- Stable organizational threads may require a new Reborn identity/binding model rather than a
  small configuration option; the spike must be willing to conclude that stock Reborn is not yet
  an architectural fit.

## Rejected alternatives

- **Delete the bridge now and accept ephemeral per-ping groups.** Rejected because it changes the
  product from an organizational conversation to isolated participant pings.
- **Fork IronClaw.** Rejected because IronWorks' governing constraint is official, unmodified
  IronClaw, and the missing mechanisms are generic upstream responsibilities.
- **Keep the bridge permanently because its guarantees are stronger.** Rejected because generic
  transport and runtime guarantees do not belong in the application layer once upstream can
  express the policy.
- **Put Account records into IronClaw memory.** Rejected because authoritative business records
  and untrusted conversational memory have different trust and freshness semantics.
- **Translate existing response IDs into Reborn thread IDs.** Rejected because the identifiers are
  neither structurally nor semantically equivalent.
