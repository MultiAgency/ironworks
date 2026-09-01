# Internal compatibility constraint: managed shared-conversation authority

- **Status:** `UPSTREAM GAP — NO ACTION PLANNED`
- **Bridge status:** `BRIDGE REMAINS REQUIRED`
- **Assessed runtime:** [`nearai/ironclaw`](https://github.com/nearai/ironclaw) official,
  unmodified Reborn
- **Re-checked:** 2026-08-31 against `nearai/ironclaw` main `24ff93f435` — **UNCHANGED, still
  required.**
  33 commits since the assessed rev (`8dc5958a`, an ancestor of main): notifications, tool
  results, CI, memory, Slack payload handling, threads compaction, sandbox and docker. None
  touches shared-conversation admission or conversation authority; the `contracts/` crates did
  not change at all. Evidence in the "Verified at main" section below.

This is an internal compatibility record, not an upstream issue draft or implementation plan. Do
not submit it, contact maintainers about it, fork IronClaw, or implement against this hypothetical
shape. Revisit only after an official IronClaw release materially changes shared-conversation
authority semantics.

## Problem

S2 would require an opt-in authority mode in which an admitted shared channel route resolves to
one durable
canonical thread under a non-human managed authority. Each accepted message still records and
acts as its authenticated human `TurnActor`; participants never inherit one another's personal
credentials, capabilities, approvals, memory, resources, or authority.

This is a generic authority primitive. It does not introduce application tenancy, organizations,
or channel-specific group policy.

## Current behavior

At upstream `8dc5958a1d80c84531943e494b22bd233c81033f`, a `Shared` route creates one fresh,
event-keyed ephemeral thread per ping, owned by its pinger. Redelivery of that external event
reuses its event binding, but another event—even from the same actor in the same group—creates
another thread. There is no canonical shared thread or shared participant set.

This deliberate `owner == actor` model was established by [PR
#7377](https://github.com/nearai/ironclaw/pull/7377)
and [PR #7397](https://github.com/nearai/ironclaw/pull/7397) to prevent one human's resource scope
from becoming another human's authority. It remains the default unless a future official release
adds a separate opt-in model that preserves the same isolation by construction.

## Limitation

Some hosts require one continuous conversation for an admitted external group or service. The
current safe choices cannot represent it:

- ephemeral pinger-owned threads discard shared continuity; and
- a stable human-owned thread would conflate the owner's authority with other actors.

A stable `ThreadId` alone is not enough. Thread ownership, actor attribution, resource scope,
capability checks, approvals, reply targets, revocation, ordering, and persistence must agree on
the authority split.

## Why existing contracts are insufficient

IronClaw already has the right terms but not the required combination:

- `TurnActor` names the authenticated human invoker.
- `TurnThreadOwner` and `TurnOwner` describe thread/product ownership; `TurnOwner::SharedAgent`
  supplies agent/project product ownership but does not by itself define a non-login conversation
  principal or participant-specific resource isolation.
- `SharedEventBinding` is event-keyed, so it cannot provide one route-keyed canonical thread.
- `SourceBindingRef`, `ReplyTargetBindingRef`, and `AcceptedMessageRef` preserve provenance and
  idempotency, but do not establish managed authority, serial admission, or revocation semantics.
- Using a synthetic `UserId` for a group would recreate the human-authority confusion the current
  model intentionally removed.

## Recognition criteria for a future official capability

Add one opt-in managed shared-binding mode and extend the existing owner vocabulary to represent
its authority. Exact type names are intentionally not prescribed; conceptually:

```text
shared binding mode:
  EphemeralPerEvent                         # current default
  ManagedCanonical {
    managed_authority: NonLoginPrincipalId,
    stable_binding_ref: OpaqueBoundedRef
  }

accepted turn:
  thread_owner / turn_owner = managed authority
  actor                     = TurnActor(authenticated UserId)
```

An official implementation could extend `TurnOwner`/`TurnThreadOwner`, or reuse `SharedAgent` if
it can satisfy every
invariant below without manufacturing a human identity. The essential contract is one typed,
non-login conversation authority distinct from `TurnActor`.

For an admitted `Shared` route, trusted host policy selects `ManagedCanonical`. IronClaw then:

1. atomically resolves or creates one durable route-to-canonical-thread binding;
2. records the authenticated actor separately for every accepted message and turn;
3. assigns each distinct `AcceptedMessageRef` one durable monotonically ordered position on that
   conversation's serialization lane;
4. routes ordinary replies through the stored shared-route reply binding while preserving exact
   actor routing for authority-bearing prompts; and
5. revalidates route and actor access so revocation blocks future admission without rewriting
   attributed history.

This issue establishes ordered admission, not the later busy-turn execution policy. Whether an
accepted item queues, steers, or is rejected under load remains separate.

The mode can be tested through current presence admission and is independently actionable from
the external-policy binding requested by S1. The two compose when a host needs both.

## Security and authority invariants

- Conversation authority, stable thread identity, and human `TurnActor` are distinct typed facts.
- Managed authority is not a `UserId`, cannot log in or pair as a human, and has no personal
  credentials, capabilities, approvals, memory, files, or resources by default.
- Every message, turn, run, capability request, approval, audit event, and delivery attempt retains
  its initiating actor.
- Actor-scoped credentials, capabilities, approvals, memory, mounts, and resources resolve only
  from that actor; one actor never inherits another's state by sharing the thread.
- Managed resources, if supported, require explicit managed-authority grants and never fall back
  to a participant's grants.
- Admitting an actor permits only the shared operations granted by policy; it does not grant
  access to any participant's direct threads or personal state.
- Reply targets remain sealed to installation, route, thread, and the required actor interaction.
  Cross-route, cross-thread, stale, and cross-actor authority-bearing replies fail closed.
- Revocation blocks future message admission and replies for that actor while preserving prior
  attribution and audit history.
- Ordering is assigned durably once per distinct accepted message. Retry/redelivery cannot obtain
  a second position, thread, or turn.
- Model output and channel text cannot select authority, binding keys, actor access, grants, or
  revocation.

## Backward compatibility

- `EphemeralPerEvent` remains the default for existing and unconfigured shared routes.
- Direct/personal thread semantics are unchanged.
- Existing per-event bindings remain readable and are never silently coalesced or re-owned.
- Enabling managed mode is explicit and prospective. Reset/cutover is explicit and cannot
  reinterpret existing thread authority in place.
- Existing consumers that cannot interpret managed authority fail closed.
- All new durable binding, authority, ordering, access, and revocation records have identical
  libSQL and PostgreSQL semantics.

## Re-evaluation criteria

1. Stock production wiring can explicitly select managed mode; a test-only fake binding service
   is not sufficient.
2. Multiple authenticated actors and events on one admitted shared route resolve one canonical
   managed thread.
3. Process restart preserves the same route-to-thread binding and managed authority.
4. No synthetic human `UserId` represents the managed authority anywhere in thread, turn,
   resource, approval, credential, or audit state.
5. Each turn retains the correct `TurnActor`; each actor can contribute permitted shared content
   but cannot inspect or use another actor's direct threads, personal memory, credentials,
   capabilities, approvals, files, mounts, resources, or run state.
6. Capability and approval gates use the initiating actor unless an explicit managed-authority
   grant/policy is selected; no participant grant is inherited.
7. Distinct accepted messages receive deterministic positions on one conversation lane.
8. Redelivery converges on the same accepted-message position, canonical thread, and turn and does
   not duplicate transcript or delivery effects.
9. Ordinary shared replies route to the stored external conversation; authority-bearing prompts
   retain exact initiating-actor routing and reject stale/cross-actor refs.
10. Revocation blocks the actor's future messages and replies while preserving attributed history.
11. Equal external conversation ids under another installation or admitted binding cannot resolve
    or inspect the thread.
12. Explicit reset is idempotent, creates a new canonical thread prospectively, and invalidates old
    reply targets without rewriting history.
13. Unconfigured shared routes retain the current ephemeral-per-event behavior exactly.
14. libSQL and PostgreSQL conformance suites produce identical results for binding, restart,
    ordering, redelivery, actor isolation, reply routing, reset, and revocation.

These tests must assert authority and resource boundaries, not only `ThreadId` equality.

## Example consumer flow

```text
admitted shared route + trusted opaque application binding
  -> host policy selects ManagedCanonical(managed authority, stable binding ref)
  -> IronClaw resolves one canonical thread and one admission position
  -> turn owner/thread owner: managed authority
  -> TurnActor: alice
  -> alice's personal gates/resources remain alice-only

later event on the same route
  -> same canonical thread, next admission position
  -> TurnActor: bob
  -> bob's personal gates/resources remain bob-only
```

An external application chooses the managed authority and route association. IronClaw owns typed
authority, canonical binding, actor attribution, resource isolation, admission ordering, reply
validation, persistence, and revocation enforcement.

## Non-goals

- Defining organizations, tenants, customers, or any consumer's business schema.
- Restoring a subject human, `owner != actor` human authority, or a synthetic organization user.
- Sharing participant credentials, capabilities, approvals, memory, files, or resources.
- Replacing actor authentication or shared-route admission.
- Defining trusted business-context injection.
- Fully specifying durable queued-turn execution, busy/steering policy, or overload behavior.
- Migrating external/legacy bridge state into IronClaw threads.
- Changing the default shared-route behavior.

## Related work

- [#3193](https://github.com/nearai/ironclaw/issues/3193) established conversation-binding
  contracts; its earlier owner/participant framing was subsequently narrowed.
- [#7377](https://github.com/nearai/ironclaw/pull/7377) removed shared-route subject binding and
  made runs act as their invoker.
- [#7397](https://github.com/nearai/ironclaw/pull/7397) established the current presence-admitted,
  ephemeral-per-ping behavior. A review comment discussed canonical shared continuity, but the
  merged contract is the authoritative current behavior.
- [#3204](https://github.com/nearai/ironclaw/issues/3204) covers canonical thread/transcript
  persistence and message ordering primitives, not managed authority.
- [#3266](https://github.com/nearai/ironclaw/issues/3266) covers outbound/reply binding and
  visibility concerns, not conversation authority.
- [#7194](https://github.com/nearai/ironclaw/issues/7194) is an adjacent outbound-target request;
  it does not provide inbound managed authority.

No current issue or PR found in the duplicate search provides this opt-in authority model.

## Local evidence

The [S1/S2 reconnaissance](../adr/evidence/0001/2026-08-26-s1-s2-reconnaissance.md) classifies S2
as `BLOCKED / BRIDGE MUST REMAIN`.

## Verified at main (2026-08-31, `24ff93f435`)

The ephemeral-per-ping model is unchanged and is named as such in the contracts:

- `crates/contracts/ironclaw_product_contracts/src/binding.rs:53-58`: "Shared (channel) routes
  resolve each inbound event onto its OWN ephemeral thread with its own refs … instead of a
  per-conversation ref pinned to the first event's thread. Direct (DM) routes carry their
  persistent per-user thread's refs."
- `crates/contracts/ironclaw_loop_contracts/src/host/run_context.rs:321` and
  `crates/contracts/ironclaw_product_contracts/src/approval_prompt.rs:68` both call it "the
  ephemeral-per-ping remodel", and state `owner == actor` as its consequence.

ONE THING TO RESOLVE, and it is upstream's prose against upstream's code rather than ours. The
`ironclaw_composition` crate's CONTRACT document describes "a shared Slack conversation is one
canonical thread its paired participants share, each message running as its sender" — which reads
as the very thing this document says is absent. `binding.rs` is the contract the resolution actually
implements and it says per-event ephemeral threads; it is unchanged since before this
assessment, so it is not a new capability, and the two have simply drifted. Code outranks prose,
so the gap stands — but anyone re-reading that contract alone would conclude otherwise, which is
worth knowing before the next re-check.
