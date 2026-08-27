# Reborn compatibility assessment

This plan turns the six deletion gates in
[`ADR 0001`](0001-reborn-bridge-compatibility-boundary.md) into bounded experiments. It is an
frozen assessment record, not permission to edit or fork IronClaw, prepare upstream issues, contact
maintainers, or plan a speculative migration. Production bridge deletion remains forbidden until
every gate has a recorded pass against the candidate `IRONCLAW_PIN`.

S1 and S2 are `UPSTREAM GAP — NO ACTION PLANNED`; `BRIDGE REMAINS REQUIRED`. Revisit them only
after a future official IronClaw release materially changes shared-conversation admission or
authority semantics. Until then, do not continue S3-S6 dependency work.

## Rules of evidence

Every spike records:

- IronWorks commit, deployed `IRONCLAW_PIN`, candidate upstream commit, and build provenance;
- whether the capability is shipping production wiring, an internal port, test-only scaffolding,
  or documentation only;
- source contract, production call path, durable owner, and relevant upstream tests;
- a black-box result against the shipping binary, not only a unit test around a trait;
- PASS, FAIL, or BLOCKED. BLOCKED and unevaluated are not PASS;
- the external constraint and the IronWorks policy that remains; and
- any state, rollout, or rollback implication.

Run candidate work from a separate clean IronClaw checkout. Put build products and temporary
stores outside both repositories. Do not patch the checkout to manufacture a pass: a small
throwaway harness may prove that an internal port is sufficient, but the gate remains failed until
stock production composition exposes it.

The baseline for this plan is current upstream `8dc5958a1d80c84531943e494b22bd233c81033f`
(2026-08-26). The deployed pin remains whatever `IRONCLAW_PIN` names. Re-run all source traces
after either value changes.

## Current gate status

This table is the live migration-status authority. Detailed findings remain tied to the dated
evidence that established them; changing an upstream revision does not silently inherit an older
result.

| Spike | Workflow state | Current result or dependency | Next action |
|---|---|---|---|
| S0 — baseline | Complete | **PASS** for the source and production-wiring scope used by S1/S2; the supplemental Postgres run was environment-blocked | Re-run for the next candidate revision |
| S1 — exact admission | Complete | **UPSTREAM GAP — NO ACTION PLANNED** ([evidence](evidence/0001/2026-08-26-s1-s2-reconnaissance.md)) | Preserve the bridge; await material official upstream change |
| S2 — organizational thread | Complete | **BRIDGE REMAINS REQUIRED** ([evidence](evidence/0001/2026-08-26-s1-s2-reconnaissance.md)) | Preserve the bridge; await material official upstream change |
| S3 — ordered admission | Frozen | Depends on S2 passing | No action planned |
| S4 — trusted context | Frozen | Not evaluated; cannot unblock S2 | No action planned |
| S5 — delivery reconciliation | Frozen | Not evaluated; cannot unblock S2 | No action planned |
| S6 — migration rehearsal | Frozen | Requires S1–S5 to pass against one revision | No action planned |

## Sequence and dependency graph

```text
S0 baseline
  -> S1 exact admission
  -> S4 trusted context
  -> S2 organizational thread
       -> S3 ordered admission
  -> S5 delivery reconciliation
  -> S6 migration rehearsal (requires S1-S5 PASS)
```

S1 and S4 run first because they test whether IronWorks policy can enter stock production at all.
S2 precedes S3 because ordering cannot be evaluated until successive group messages resolve to the
same organizational authority. S5 is independent but must pass before a production canary. S6 is
the final rehearsal, not a paper design exercise.

## S0 — freeze the comparison baseline

**Question:** Are the later results reproducible and tied to exact code?

**Work:**

1. Record both revisions and confirm both worktrees before testing.
2. Re-run the upstream Telegram channel conformance suite and the relevant production integration
   journey in `tests/integration/extension_delivery.rs`.
3. Capture the current manifest, shipping CLI binding, composition wiring, and persisted store
   schemas named by the later spikes.
4. Create a result directory outside the repositories containing commands, outputs, and a compact
   source-path index. Store no tokens or Telegram payload content.

**Exit:** the baseline is reproducible, or all later spikes are BLOCKED.

## S1 — exact group-to-organization admission

**Current upstream points to test:**

- `crates/extensions/packages/telegram/manifest.toml`: presence-based shared admission.
- `crates/extensions/packages/telegram/src/channel.rs`: `ChannelIngress::receive` normalization.
- `crates/product/ironclaw_assistant/src/policy.rs`: `BeforeInboundPolicy` sees installation,
  actor, conversation, binding key, and user payload and can allow/rewrite/reject.
- `crates/product/ironclaw_assistant/src/workflow.rs`:
  `DefaultProductSurface::with_before_inbound_policy`.
- `crates/contracts/ironclaw_product_contracts/src/binding.rs`: `ProductBindingResolver` and route
  selection.

**Experiment:**

1. Send signed Telegram webhook fixtures for an allowed group, an unknown group, duplicate group
   bindings, a stale binding, and the same textual group ID asserted from a different verified
   installation.
2. First run the stock binary. Confirm that pairing/presence is insufficient to implement the
   IronWorks allowlist.
3. In an isolated upstream harness, bind a fake `BeforeInboundPolicy` backed by a two-organization
   registry. Prove rejection occurs before canonical message staging and thread creation and that
   replays converge.
4. Trace whether the shipping composition offers any supported way to install that policy. An
   internal builder method alone is a FAIL for stock compatibility.

**Acceptance:** exactly one registry binding admits; every invalid case leaves no thread, staged
message, run, or delivery attempt; policy unavailability fails retryably without admitting; and
the same behavior is configurable in the unmodified shipping runtime.

**Disposition:** `UPSTREAM GAP — NO ACTION PLANNED`. Keep the registry, organization lookup,
duplicate detection, and allow/deny decision in IronWorks and preserve the bridge. Re-run this
assessment only after an official release materially changes production shared admission.

## S4 — authoritative per-turn context

**Current upstream points to test:**

- `crates/loop/ironclaw_loop_host/src/identity_context.rs`:
  `HostIdentityContextSource` and trusted identity candidates.
- `crates/app/ironclaw_composition/src/runtime.rs`: production chooses only
  `DefaultSystemPromptIdentitySource` or `EmptyIdentityContextSource`.
- `crates/product/ironclaw_assistant/src/policy.rs`: rewriting changes the user-message payload;
  it is not a trusted context lane.
- `crates/kernel/ironclaw_host_runtime/src/memory_context.rs`: memory snippets are wrapped as
  untrusted content.
- `crates/product/ironclaw_assistant/src/unbound_turn.rs`: prepared context is initial-thread
  material, not a per-turn refresh contract.

**Experiment:**

1. Use two organization-scoped fixture services returning distinguishable records and versions.
2. In an upstream harness, implement a dynamic `HostIdentityContextSource` and determine whether
   it can safely load current organization context on every prompt rebuild, including after
   compaction and restart.
3. Attempt the same through the stock binary's supported configuration. Separately test and reject
   three false substitutes: `RewriteUserMessage`, prepared initial context, and memory snippets.
4. Exercise service timeout, wrong organization, over-budget content, malicious record text,
   version change between turns, and replay of the same inbound event.

**Acceptance:** the authenticated binding—not model input—selects the organization; current data is
loaded for every turn; it occupies a bounded host-authoritative lane with provenance and version;
wrong-scope or unavailable context fails before model execution; raw credentials and service
addresses are never model-visible; and the provider is bindable in stock production.

**Disposition:** no action planned while S2 remains blocked. IronWorks continues to own the Account
Service client, organization authorization, record selection, envelope schema, and composition
order through the maintenance-only bridge. Do not plan a hypothetical upstream context source or
route authoritative records through memory.

## S2 — stable organizational group conversation

**Current upstream points to test:**

- `crates/contracts/ironclaw_product_contracts/src/binding.rs`:
  `binding_profile_for_trigger` maps mentions/commands to `Shared`.
- `crates/product/ironclaw_assistant/src/inbound_turn.rs`: external events carry per-event
  source/reply bindings.
- `crates/domains/ironclaw_conversations/src/traits.rs`: `ConversationBindingService`.
- `crates/domains/ironclaw_conversations/src/memory.rs`: `SharedEventBinding` deliberately mints
  a pinger-owned thread per event.
- `crates/product/ironclaw_assistant/src/conversation_binding.rs`:
  `ProductConversationBindingService`.

**Experiment:**

1. Post two mentions and one reply from two paired humans in one Telegram group; prove stock
   behavior produces distinct shared-event threads.
2. Build an isolated binding-service harness that resolves all three events to one stable thread.
3. Test both possible authorities explicitly:
   - a shared human-owned thread; and
   - a tenant-scoped organizational service principal to which authenticated participants have
     posting authority but from which they gain no personal authority.
4. Attack cross-group, cross-installation, stale reply-target, removed participant, and binding
   reset cases. Verify owner/actor checks, delivery routing, and memory scope.

**Acceptance:** one group maps to one durable thread across restart and redelivery; a different
group cannot resolve it; participants cannot inherit each other's credentials, personal memory,
approvals, or filesystem authority; and reset/revocation is explicit and audited.

**Disposition:** `BRIDGE REMAINS REQUIRED`. Do not design or implement a replacement authority
model locally or upstream. Preserve the current bridge and re-run this assessment only after an
official release materially changes shared-conversation authority semantics.

If upstream rejects organizational service conversations as outside its model, record S2 FAIL and
retain the bridge. Do not emulate the feature with shared user credentials inside native Reborn.

## S3 — ordered admission as separate turns

**Dependency:** S2 PASS.

**Current upstream points to test:**

- `crates/kernel/ironclaw_turns/src/coordinator.rs`: `TurnCoordinator` and one-active-run
  enforcement.
- `crates/product/ironclaw_assistant/src/steering.rs`: `admit_busy_steering`.
- `crates/product/ironclaw_assistant/src/inbound_turn.rs`: busy results become steering or
  `RejectedBusy`/`DeferredBusy` outcomes.
- `crates/loop/ironclaw_loop_host/src/durable_input_queue.rs`: durable inputs consumed by an
  active run.
- `tests/integration/steering.rs`: current queued input intentionally modifies the active run.

**Experiment:**

1. Hold turn A inside a model call, then admit B and C for the same organizational thread.
2. Prove stock behavior and distinguish steering, rejection, and a genuinely queued next turn.
3. Prototype a generic busy-admission policy `queue_as_next_turn` in a harness without changing
   the turn coordinator's single-active-run invariant.
4. Crash after B is durable but before A completes; restart and prove A, B, C each execute once in
   order. Repeat with duplicate vendor delivery and with A failing terminally.

**Acceptance:** every accepted event has a durable queue position; A, B, and C produce distinct
turn/run identities in order; duplicates converge; no accepted message is converted to steering,
busy-noticed away, or skipped after failure/restart; and bounded backpressure is explicit.

**Disposition:** no action planned while S2 remains blocked. Do not add another generic worker
pool to IronWorks or plan a hypothetical native queue.

## S5 — model-free ambiguous-delivery reconciliation

**Current upstream points to test:**

- `crates/product/ironclaw_assistant/src/delivery_coordinator.rs`: persisted
  `Prepared -> Sending`, bounded retry, partial-send refusal, and interrupted-send recovery.
- `crates/domains/ironclaw_outbound/src/store.rs`: `OutboundStateStorePort` point reads, lists,
  claims, recovery, and status updates.
- `crates/domains/ironclaw_outbound/src/types.rs`: `OutboundDeliveryAttempt`, projection reference,
  and `Unknown`/`DeadLettered` states.
- `crates/extensions/packages/telegram/src/channel.rs`: per-part provider evidence.

**Experiment:**

1. Inject known-unsent first-part failure, accepted first part plus retryable second-part failure,
   adapter-level ambiguity, crash in `Sending`, and failure of the final durable `Delivered` write.
2. Confirm stock state and prove automatic paths never blindly resend.
3. Determine whether the stored projection reference can rematerialize the exact original semantic
   output after restart without model execution. Record any lost rendering, attachment, reply
   target, or authorization input.
4. Exercise an operator-only prototype with four explicit outcomes: inspect, mark observed
   delivered, redeliver accepting duplicate risk, and abandon/dead-letter. Require a confirmation
   token and immutable audit event for every mutation.

**Acceptance:** an operator can identify the exact scoped attempt, inspect non-secret evidence,
settle it without changing the model result, or redeliver the same persisted semantic output
without a model call; stale/cross-tenant attempts fail indistinguishably; and every decision is
durable and auditable.

**Disposition:** no action planned while S2 remains blocked. Maintain the current model-free bridge
reconciliation path for correctness; do not prepare upstream API or operator-surface work.

## S6 — drain, archive, and cutover rehearsal

**Dependencies:** S1-S5 PASS against one candidate upstream rev.

**IronWorks points to test:**

- `multi/seam/bridge_state.py`: update, thread, cursor, and delivery rows.
- `multi/seam/bridge_core.py`: terminal states and explicit redelivery.
- `docs/BRIDGE_DELIVERY.md`: current recovery and rollback contract.
- `multi/provision/deprovision.sh`: explicit thread removal.

**Experiment:**

1. Clone a sanitized production-shaped bridge database and populate every terminal and
   non-terminal state.
2. Disable new Telegram admission, drain normal work, and require explicit settlement for
   `TURN_STARTED`, `DELIVERY_RETRY`, and `DELIVERY_RECONCILE` rows.
3. Produce an archive manifest containing schema version, safe cursor, row counts by state,
   unresolved count, cutover timestamp, IronWorks commit, old/new IronClaw revisions, and database
   digest. Store the database read-only outside the live state path.
4. Register native webhook ingress, create fresh organizational Reborn threads, and optionally
   inject one operator-approved handoff summary through the gate-4 context source.
5. Run a no-send shadow comparison, then a separate-bot/private-group canary, then one production
   tenant. Prove the old poller and new webhook are never active for the same production bot.
6. Rehearse rollback before first native acceptance and prove rollback is refused afterward.

**Acceptance:** zero unresolved bridge rows; archive integrity verifies; no identifier is
translated; exactly one ingress authority is active; first native turns start at the recorded
cutover; isolation and delivery proofs pass; and rollback obeys the one-authority rule.

**Disposition:** no migration or cutover work is planned. If all gates are independently satisfied
by a future official release, IronWorks still owns retention, canary, rollback, and the
organization-specific handoff; IronClaw does not need to understand bridge SQLite or
`previous_response_id` migration.

## External constraints — no action planned

| Gate | External constraint record | Current IronWorks posture |
|---|---|---|---|
| S1 | [Production-bindable shared admission](../upstream-proposals/shared-conversation-admission.md) | Keep exact registry-backed admission in the bridge |
| S2 | [Managed shared-conversation authority](../upstream-proposals/organizational-conversation-authority.md) | Keep the organizational conversation authority in the bridge |

These records are not issue drafts, implementation plans, or invitations to coordinate upstream.
No submission or dependency work is planned.

## Stop conditions

Stop a spike and record BLOCKED rather than widening scope when it would require:

- patching or vendoring the production runtime;
- exposing Account Service credentials or organization selection to the model;
- sharing one human's personal runtime authority with another participant;
- treating untrusted memory as authoritative business context;
- running the model again to recover delivery; or
- operating polling and webhook ingress concurrently for one production bot.

The correct result of a spike may be that the bridge must remain. The plan measures compatibility;
it does not assume it.
