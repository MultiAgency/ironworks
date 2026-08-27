# ADR 0001 evidence: Reborn deletion-gate reconnaissance for S1 and S2

- **Date:** 2026-08-26
- **IronWorks revision:** `8043a391012d24d677b4add073edf445c2ce1adf`
- **Deployed IronClaw pin:** `70795c16ed0cec21eb8cba16d2dcf851d25dc83d`
  (`ironclaw-v1.3.0`)
- **Candidate upstream:** `8dc5958a1d80c84531943e494b22bd233c81033f`
  (`main`, clean and equal to `origin/main` when inspected)
- **Scope:** minimum current-upstream contract and production-composition reconnaissance for S1
  and S2. No IronClaw or bridge implementation was changed.

## Outcome

| Gate | Current result | Migration consequence |
|---|---|---|
| S1 — exact group-to-organization admission | **FAIL — REQUIRES UPSTREAM CHANGE** | Reborn has an adequate internal admission contract, but the stock binary selects presence admission and offers no supported production binding for IronWorks policy. |
| S2 — stable organizational group conversation | **BLOCKED — BRIDGE MUST REMAIN** | Current durable shared-route semantics deliberately create a separate pinger-owned thread for each event. No current authority model safely represents the required organizational conversation. |

S2 is the architectural go/no-go point. The native migration stops here. S3 must not be treated as
actionable, and no bridge deletion may begin, unless upstream first supplies a safe organizational
conversation authority and the S2 adversarial tests pass.

## S1 — exact group-to-organization admission

### Finding

The most exact current seam is not Telegram parsing and does not need an IronWorks-specific field
in the Telegram manifest. `SharedConversationAdmission` receives:

- product adapter id;
- verified installation id; and
- a route key containing external space and conversation ids.

It returns an admission decision before shared binding resolution. Product rechecks it on resolve,
lookup, and reset. This contract can express an IronWorks lookup keyed by the authenticated
Telegram installation and exact group id, while IronWorks retains all organization-registry and
duplicate/stale-binding policy.

Current production does not bind that policy. `PresenceSharedAdmission` intentionally ignores the
route key and admits every conversation delivered through its own adapter installation. Although
`ChannelExtras.shared_admission` can override that default inside a custom host assembly, the
shipping `ChannelExtensionBinding` has no admission field and composition registers its extras
with `shared_admission: None`. The generic host therefore selects presence admission. The channel
workflow also builds `DefaultProductSurface` without attaching a channel-specific
`BeforeInboundPolicy`; that other internal builder seam is not a stock configuration mechanism.

Under ADR 0001, an internal Rust trait or a custom composition does not count as support in the
official unmodified runtime. S1 is therefore **FAIL — REQUIRES UPSTREAM CHANGE**, not
“satisfiable via an existing extension point.”

### Exact upstream extension points

For S1 to be reconsidered after a future official release, the runtime would need to preserve and
production-bind the existing generic contract:

1. `ironclaw_product_contracts::shared_admission::SharedConversationAdmission` and
   `SharedConversationAdmissionRequest` are the policy boundary.
2. `ironclaw_extension_host::channel_host::ChannelExtras.shared_admission` is the existing host
   override to carry through production.
3. `ironclaw_composition::ChannelExtensionBinding` /
   `RebornHostBindings::with_channel_extension_bindings` is the current shipping input that drops
   this concern and is the first composition point to widen or complement.
4. `RebornChannelWorkflowFactory::build_channel_workflow` already forwards the resolved admission
   port into `ProductInstallationScope`; its ordering should remain unchanged.

A Rust-only field usable only by a rebuilt binary would still fail the IronWorks “official,
unmodified runtime” rule. The production binding must be supported through an official policy
provider mechanism (for example a bounded host policy provider configured for an installation),
with typed request/response, timeout, fail-closed behavior, and no raw secret exposure.

`BeforeInboundPolicy` remains useful for broader pre-staging policy, but widening it is not the
minimum S1 change because the narrower shared-admission port already carries the exact route
identity and runs at the correct binding boundary.

### Ownership split

**External constraint — no action planned:** official provider lifecycle and production binding,
bounded invocation, fail-closed error mapping, and preservation of resolve/lookup/reset ordering
would be required before reassessment.

**Keep in IronWorks:** group-to-organization registry, organization selection, exact allow/deny
rules, duplicate/stale-record detection, policy backend availability, and audit policy.

### Acceptance still required after an upstream implementation

The upstream change must be tested with allowed, unknown, duplicate, stale, and cross-installation
group fixtures. Every rejection must precede actor-pairing mutation, thread/message staging, run
creation, and delivery. Retryable policy unavailability must release admission idempotency rather
than creating state. Those cases cannot pass against the current stock composition, so a local
fake-provider harness would not change this gate result.

## S2 — stable organizational group conversation

### Finding

Mentions and bot commands select the generic `Shared` route, but “shared” no longer means one
canonical group thread. The product layer passes the external event id into binding and explicitly
preserves the returned per-event source/reply pair. The durable conversation implementation stores
an `event_shared_bindings` map keyed by external event and creates a fresh thread whose sole owner
is the pinger. Redelivery of the same event reuses that event's thread; a new event in the same
group creates another thread. A second participant gets a different thread owned by that second
participant.

This behavior is intentional authority isolation, not an accidental missing cache key. Replacing
it with one human-owned thread would either make one participant the durable owner of another
participant's organizational turns or require weakening the owner/actor checks that currently
protect personal memory, credentials, approvals, filesystem scope, and reply targets. Neither is
an acceptable migration technique.

`ConversationBindingService` is a public domain trait and a test harness can substitute an
implementation, but the production channel workflow constructs the official filesystem-backed
conversation services and `ProductConversationBindingService` directly. More importantly, the
current thread authority vocabulary has no proved tenant-scoped organizational service principal
with independently authorized human posters. A custom binding implementation alone cannot prove
the required non-leakage.

S2 is therefore **BLOCKED — BRIDGE MUST REMAIN**. It also **requires upstream design and code**
before it can be re-evaluated, but the bridge consequence is the authoritative gate status.

### Exact upstream extension points

Any future official authority design would need to cover all of these together before S2 is
reassessed:

1. `binding_profile_for_trigger` may continue to classify mentions/commands as `Shared`; no
   Telegram-specific route kind is needed.
2. `ProductConversationBindingService` needs a generic, production-selectable shared-conversation
   binding strategy rather than assuming the current per-event implementation.
3. `ConversationBindingService::resolve_or_create_binding_with_trusted_scope` and the durable
   conversation store need a typed organizational authority, lifecycle, and participant-posting
   contract. Reusing a human `UserId` as an organization is not acceptable.
4. `TurnScope`, authorization, credentials, approvals, memory, filesystem, delivery reply-target
   validation, reset, and revocation must distinguish the organizational thread authority from the
   authenticated human actor for each turn.
5. Durable binding identity should remain keyed by verified installation plus external
   conversation, with explicit tenant isolation and audited reset/revocation.

The preferred upstream shape is a managed installation/service-principal conversation whose
thread is organizationally owned while each admitted turn retains its authenticated human actor.
If upstream can prove an equally strong neutral authority model without a service principal, that
is also viable. Reviving a canonical shared human-owned thread is not.

### Ownership split

**External constraint — no action planned:** typed organizational/service-principal authority,
configurable binding strategy, durable thread and participant lifecycle, authorization separation,
and cross-resource isolation guarantees would be required before reassessment.

**Keep in IronWorks:** which Telegram group maps to which organization, whether that organization
uses the organizational-conversation policy, participant eligibility derived from IronWorks
policy, and the cutover/drain/archive/rollback decision.

### Required S2 proof before reassessment

A future official implementation would have to demonstrate:

- two actors and multiple events in one admitted group resolve one durable thread across restart;
- redelivery converges without creating a second thread or second turn;
- another group or installation cannot resolve, reply to, reset, or inspect that thread;
- removing a participant revokes future posting without rewriting history;
- neither participant can inherit the other's personal credentials, memory, approvals, files,
  runs, or delivery authority;
- the organizational service principal cannot be used as a general interactive human identity;
  and
- reset and revocation are explicit, durable, and audited.

Until all of those pass through production wiring, the IronWorks bridge remains the conversation
authority.

## Evidence index

Current upstream source inspected:

- `crates/contracts/ironclaw_product_contracts/src/shared_admission.rs:22-90`
- `crates/extensions/ironclaw_extension_host/src/channel_shared_admission.rs:1-66`
- `crates/extensions/ironclaw_extension_host/src/channel_host.rs:135-155,200-213,906-920`
- `crates/app/ironclaw_composition/src/input.rs:239-274,837-843`
- `crates/app/ironclaw_composition/src/extension_host_assembly.rs:682-692`
- `crates/product/ironclaw_assistant/src/channel_workflow.rs:366-410`
- `crates/contracts/ironclaw_product_contracts/src/binding.rs:127-147`
- `crates/product/ironclaw_assistant/src/inbound_turn.rs:632-672`
- `crates/domains/ironclaw_conversations/src/traits.rs:15-79`
- `crates/domains/ironclaw_conversations/src/memory.rs:1393-1445`
- `crates/domains/ironclaw_conversations/tests/conversation_state_store_contract.rs:697-812`
- `tests/integration/extension_delivery.rs:1932-1971`

Targeted upstream tests, all using
`CARGO_TARGET_DIR=/private/tmp/ironclaw-ownership-verify` and `--locked`:

| Evidence | Result |
|---|---|
| `cargo test -p ironclaw_extension_host channel_shared_admission` | **PASS** — 4 tests. Confirms the shipping default admits every conversation for its own adapter/installation and rejects only foreign adapter/installations. |
| `cargo test -p ironclaw_assistant --test product_surface_contract shared_admission` | **PASS** — 2 tests. Confirms per-request admission checks, immediate disconnect behavior, route-key integrity, and per-event thread divergence. |
| `cargo test -p ironclaw_assistant --test product_surface_contract unadmitted_shared_route_fails_before_actor_binding_side_effects` | **PASS** — 1 test. Confirms denial precedes actor-binding mutation. |
| `cargo test -p ironclaw_conversations --test conversation_state_store_contract shared_channel_pings_get_ephemeral_pinger_owned_threads_idempotent_per_event` | **PASS** — 1 test. Confirms distinct-event and distinct-actor thread separation plus same-event replay across reopen. |
| `SKIP_FRONTEND_BUILD=1 RUST_MIN_STACK=16777216 cargo test -p ironclaw_integration_tests --test reborn_integration_extension_delivery telegram_update_becomes_a_turn_and_a_coordinated_reply::case_1_libsql -- --exact --test-threads=1` | **PASS** — 1 full production-wiring LibSQL journey. It includes a second paired Telegram participant in the same supergroup/topic and asserts a different pinger-owned thread. |
| The same full journey, `case_2_postgres` | **ENVIRONMENT BLOCKED** before behavior assertions because no Docker daemon was reachable. This is not counted as a pass and does not alter either gate classification. |

The first combined two-backend invocation also exhausted the default test-thread stack on the
LibSQL leg while the Postgres leg failed for missing Docker. Running the hermetic LibSQL case alone
with a 16 MiB test-thread stack completed successfully. The WebUI build was skipped because this
channel journey does not exercise WebUI and the sandbox does not permit Vite to create its
temporary config file in the read-only upstream checkout.

## Sequence decision

Do not start S3-S6 and do not implement bridge deletion. No upstream dependency, issue,
maintainer-coordination, fork, or speculative migration work is planned. Revisit S1/S2 only after
a future official IronClaw release materially changes shared-conversation admission or authority
semantics.
