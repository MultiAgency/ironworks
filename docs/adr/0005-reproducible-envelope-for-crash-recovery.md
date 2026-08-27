---
status: accepted
---

# Make the envelope reproducible so a crashed turn can be recovered

Measured against a disposable instance at `IRONCLAW_PIN`: replaying an `Idempotency-Key` with a
byte-identical body returns **HTTP 200 and the original response id**, while a differing body is
refused **409** (`param: "idempotency_key"`). So a turn interrupted after dispatch is recoverable
— provided the request body can be rebuilt exactly. ironworks cannot rebuild it, because
`build_envelope` embeds `retrieved_at: {_now()}` at render time, and it therefore treats the
whole window as unrecoverable (`RECOVERY_BLOCKED`, operator reconciliation, no second turn). That
is a consequence of one nondeterministic field, not a property of the runtime.

We make the envelope a pure function of its inputs — the clock is passed in, never read — and
record what the body was built from when the turn starts. On restart: rebuild, compare, replay
the key, take the original response id, deliver the stored answer.

## Consequences

The journal must stay free of client content, so it records the **timestamp**, the per-account
catalog versions the envelope was built from, and a **digest** of the rendered envelope — never
the envelope itself. Recovery refetches the records, rebuilds, and compares digests. A match
replays; records that moved in the interim mean the body is genuinely irreproducible and the
update is blocked. `RecoveryBlocked` becomes the rare case rather than the default one.

This only holds if serialization is deterministic. `serde_json` over a struct emits fields in
declaration order, which is stable — but that is a property to test explicitly, not assume, and
it forbids `HashMap` anywhere in the request body (use `BTreeMap` or a `Vec` of pairs).

Confirmed against a **completed** turn, not only a failed one: the replay returned the original
id in **21 ms** against **984 ms** for the original request, so it is served from the idempotency
index rather than re-executed. That timing gap is the evidence — an identical id could in
principle come from a re-run, and the two-orders-of-magnitude difference rules that out.

Unaffected: at-most-once is still the guarantee. This does not regenerate anything — it retrieves
what a previous turn already produced, which is the same discipline, applied at a point ironworks
believed was closed.
