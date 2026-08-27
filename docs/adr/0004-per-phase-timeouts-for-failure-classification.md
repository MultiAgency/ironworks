---
status: accepted
---

# Configure per-phase HTTP timeouts, never a global one

At-most-once model execution rests on one distinction: whether a failed request *provably never
left this process* or *may have executed*. We use `ureq` 3.4 (41 packages, blocking, no async
runtime) and it can express that — but only if timeouts are configured per phase. `ureq::Error`
carries a sound provably-unsent set (`HostNotFound`, `ConnectionFailed`, `ConnectProxyFailed`,
the TLS variants, and `Io` with a connect-phase `ErrorKind`), and `Error::Timeout` wraps a
`Timeout` enum naming the phase — `Resolve` and `Connect` are unsent, `SendRequest`, `SendBody`
and `RecvResponse` are ambiguous. But `Timeout::Global` and `Timeout::PerCall` are phase-agnostic,
so a single global timeout collapses "DNS never resolved" and "the model is mid-turn" into one
indistinguishable error.

## Consequences

Every connect failure would otherwise have to be classified ambiguous, which blocks that tenant's
update for operator reconciliation rather than failing it cleanly and letting the sender retry.
Correct, but needlessly pessimistic, and it turns a common transient into manual work.

The HTTP client is therefore chosen on whether its error type can express "provably unsent" — not
on ergonomics or dependency count. Any replacement must clear the same bar, and the classifier
has tests asserting each variant lands in the right bucket.

The classifier matches known-unsent variants explicitly with `_ => Ambiguous` as the catch-all.
This is the rare case where a wildcard is correct: the safe default is "assume it ran", so an
unrecognised variant costs pessimism rather than a second billed turn. It is the exact inverse of
the state machine, where a catch-all would be a defect.
