---
status: accepted
date: 2026-08-31
---

# Treat flushed stdout as direct-turn delivery

## Context

`turn` and `coordinate` commit a completed model response, print it to stdout and have no channel.
They previously left delivery `pending` forever because only relay and redelivery recorded a
delivery attempt. Since `Journal::unresolved` correctly includes every turn not both completed and
delivered, a persistent terminal journal always recovered its oldest answer and could never serve
a second question.

A fresh journal avoids the symptom by minting a new idempotency key. It also abandons the durable
recovery evidence, so it cannot be the normal operating rule.

## Decision

Stdout is the delivery surface for `turn` and `coordinate`. The command writes the complete answer
and flushes stdout before recording `AttemptOutcome::Complete`. A failed write records an ambiguous
delivery because a prefix may already be visible. A crash after the flush but before the journal
update leaves `completed + pending`; recovery fetches the known response and may print an identical
duplicate, but never creates a new logical turn.

For this surface, `delivered` means the complete write and flush succeeded: the bytes left the
process through the operator-selected stdout destination. It does not claim that a person read
them; redirecting stdout to `/dev/null`, for example, still constitutes successful terminal
delivery under this model.

The `after-output` proof boundary makes that residual window deterministic. After a successful
restart, delivery is durable and the row leaves `Journal::unresolved`, allowing the same persistent
journal to carry the next question.

## Consequences

- Successful direct turns no longer make their journal permanently single-question.
- `status` rows now mean work or delivery evidence genuinely remains outstanding.
- Existing `completed + pending` rows are not silently changed. The next invocation recovers and
  prints that known response before settling it; a later invocation can then serve a new question.
- A failed stdout write, including an ordinary broken pipe, records `uncertain` because some prefix
  may have escaped. That is the first direct-terminal path requiring a human: automatic recovery
  cannot safely decide whether to resend, and no reconciliation command exists yet.
- Reconciliation remains separate. This decision prevents completed terminal turns from remaining
  pending after a successful write; it does not add a command that clears uncertain old state by
  assertion.
- The `after-output` boundary and its recovery behavior are established by the ninth B5 check
  against the disposable live deployment: one create, recovery by the same response id, successful
  terminal delivery and zero unresolved turns.
