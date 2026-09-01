# Internal compatibility constraint: production-bindable shared-conversation admission

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
admission.

## Problem

IronClaw has the correct product port for route-keyed shared-conversation admission, but stock
production composition always supplies presence-based admission. An external application cannot
configure that port without building a custom binary.

This record identifies the minimum official behavior that would justify re-evaluating S1. It does
not request work or change shared-conversation authority.

## Current behavior

At upstream `8dc5958a1d80c84531943e494b22bd233c81033f`:

- `SharedConversationAdmissionRequest` carries the adapter id, verified installation id, and
  `ProductConversationRouteKey` (`space_id`, `conversation_id`).
- `SharedConversationAdmission` answers whether that shared route is connected and is checked
  fail closed on resolve, lookup, and reset.
- `PresenceSharedAdmission` treats verified delivery for the adapter's own installation as
  admission.
- `ChannelExtras.shared_admission` is injectable by tests or a custom host assembly, but the
  shipping composition provides no externally configurable implementation.

The presence default was deliberately established by [PR
#7397](https://github.com/nearai/ironclaw/pull/7397)
after the owner/subject model was removed in [PR
#7377](https://github.com/nearai/ironclaw/pull/7377).
That default remains authoritative unless a future official release changes it.

## Limitation

Some hosts have an authoritative external registry in which a verified channel route must map to
exactly one application scope before IronClaw creates binding, actor-pairing, thread, message, or
turn state. Presence cannot prove that mapping. Unknown, ambiguous, stale, or wrong-installation
routes must be denied.

Today those hosts must ship a custom IronClaw composition, put policy in front of IronClaw and
duplicate ingress behavior, or accept the presence default.

## Why the existing contract is insufficient

The admission port and route-keyed request already exist. The missing production capability is:

1. a supported stock-runtime way to select and configure an external implementation;
2. a structured admitted result that can preserve one opaque application binding reference as
   trusted routing provenance; and
3. explicit failure semantics that cannot silently fall back to presence.

`BeforeInboundPolicy` is broader than this decision and is not the production channel binding
surface. User text or model-visible context must not carry the binding reference.

## Recognition criteria for a future official capability

Keep `SharedConversationAdmissionRequest` and the `SharedConversationAdmission` product port.
Add a production provider binding resolved by stock composition and a structured decision, either
by extending the port compatibly or by adding a companion contract:

```text
decide(SharedConversationAdmissionRequest) ->
    Admit { application_binding_ref: OpaqueBoundedRef }
  | Deny
  | Unavailable
  | Invalid
```

Any future official capability would need these semantics:

- the decision input is the verified adapter/installation/space/conversation route;
- the admitted reference is opaque, bounded, non-secret, and host-trusted;
- `Deny`, timeout/unavailability, and malformed output create no conversation state;
- timeout/unavailability may be reported as retryable, but never as admitted;
- while an external provider is configured, presence is not a fallback; and
- with no external provider configured, current `PresenceSharedAdmission` behavior is unchanged.

A missing or invalid configured provider should fail startup where configuration can be validated;
runtime provider failures still fail the affected admission closed.

## Security and authority invariants

- The installation id comes from verified host ingress, never message text or model output.
- Route identity includes adapter, installation, optional space, and conversation. Equal external
  ids under different installations are different routes.
- Admission precedes actor-pairing mutation, binding/thread creation, message staging, turn/run
  creation, model execution, and delivery.
- A configured policy has no presence fallback for denial, timeout, unavailability, or malformed
  output.
- The opaque binding reference cannot be selected by a channel participant or model and is not a
  prompt instruction.
- One accepted external event cannot silently acquire another binding reference on redelivery.
- Resolve, lookup, and reset continue to revalidate admission so revocation takes effect.
- Provider credentials remain host-mediated and model-inaccessible.

## Backward compatibility

- Unconfigured shared routes retain presence admission and current ephemeral-per-ping behavior.
- Direct routes do not call shared admission.
- Existing boolean implementations remain usable through an adapter if a structured companion is
  introduced.
- Existing durable bindings remain readable, but a configured policy must admit a route before a
  stored binding is returned.
- Disabling external policy is an explicit operator configuration change, not an automatic
  fallback after failure.

## Re-evaluation criteria

1. A stock production binary selects a configured external admission provider; test-only
   `ChannelExtras` injection is not sufficient.
2. The provider receives the exact verified adapter/installation/space/conversation route key.
3. An admitted route preserves one opaque binding reference as trusted provenance.
4. Denial, timeout, unavailability, and malformed output create no actor pairing, binding, thread,
   message, turn/run, or delivery state.
5. With external policy configured, every failure above proves that presence admission is not
   consulted as fallback.
6. The same conversation id under another installation is independently denied unless admitted.
7. Redelivery converges on the original decision and cannot create another binding or change its
   opaque reference silently.
8. Revocation makes subsequent resolve, lookup, and reset fail closed.
9. With no provider configured, current presence-based shared routing is unchanged.
10. Direct routing is unchanged and does not invoke the provider.

## Example consumer flow

```text
verified channel event
  -> SharedConversationAdmissionRequest(adapter, installation, space, conversation)
  -> external policy: Admit(application_binding_ref = "scope:7f3...")
  -> IronClaw preserves the opaque ref as trusted binding provenance
  -> normal actor, binding, idempotency, turn, and delivery processing
```

An external application owns the registry and the meaning of `scope:7f3...`. IronClaw owns
verified route construction, bounded invocation, fail-closed ordering, provenance, and lifecycle
integration. A denied or ambiguous lookup creates no conversation state.

## Non-goals

- Defining organizations, tenants, accounts, or any consumer's schema in IronClaw.
- Moving the external registry into IronClaw.
- Adding Telegram-specific allowlist fields.
- Changing actor authentication, shared-conversation authority, or thread ownership.
- Defining trusted business-context injection, ordered turn execution, or delivery settlement.

## Related work

- [#3193](https://github.com/nearai/ironclaw/issues/3193) introduced the conversation-binding
  contracts; it does not expose external admission in stock composition.
- [#7397](https://github.com/nearai/ironclaw/pull/7397) retained
  `SharedConversationAdmission` while making presence the shipping policy.
- [#6998](https://github.com/nearai/ironclaw/pull/6998) established the product-contract port
  boundary this issue should preserve.

No current issue or PR found in the duplicate search provides the requested stock production
binding.

## Local evidence

The [S1/S2 reconnaissance](../adr/evidence/0001/2026-08-26-s1-s2-reconnaissance.md) classifies S1
as `REQUIRES UPSTREAM CHANGE`.

## Verified at main (2026-08-31, `24ff93f435`)

The production composition still supplies no implementation, and upstream now states that as a
decision rather than a gap:

- `crates/app/ironclaw_composition/src/extension_host_assembly.rs:688` registers
  `ChannelExtras { …, shared_admission: None, … }` for every channel extension binding.
- `crates/extensions/ironclaw_extension_host/src/channel_host.rs:150` falls back to
  `PresenceSharedAdmission::new(adapter_id, installation_id)` when it is `None`.
- the `ironclaw_composition` crate's own CONTRACT document (upstream): "Shared-channel admission
  is presence-based and
  **needs no configuration** … the bot being in the channel is the admission".

READ THAT LAST LINE AS A CLOSED DOOR, not a pending item. This document was written as "the
minimum official behavior that would justify re-evaluating S1"; the contract now answers that an
externally configurable admission port is not planned, because presence IS the intended answer.
An operator-run host with its own authoritative registry — which tenant owns which Telegram group
— still has nowhere to say so.
