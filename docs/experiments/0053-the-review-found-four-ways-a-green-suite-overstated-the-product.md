# 0053 — The review found four ways a green suite overstated the product

**2026-09-07.** Remediation record for a source review after `0049`. No model, runtime, member,
channel, record service, backup repository, or live host was contacted.

The review started from the product claims rather than the passing suite and found four gaps that
the existing controls did not exercise:

1. Hazardous guidance was still a valid `Guidance`, so each activation path had to remember a
   second publication gate. The legacy leaked-comment shape and unwritten stubs are now rejected by
   `Guidance::parse`; compose, publication, rollback and serving share that single control.
2. A preserved tenant export was checked by hashing the self-digest-bearing file even though the
   Account Service digest covers the object without that field. Retry therefore rejected a valid
   export, and it happened before current scope verification. Retry now runs the same complete
   export validator after resolving the current registry and credential scope.
3. `dev/prove-host.sh` still read the former fleet registry, environment file and service account.
   Its fake SSH control now refuses a remote script that does not name the per-cell paths and
   identity used by the templated units.
4. The first snapshot manifest proved archived bytes but discarded the canonical restore
   destination for credentials. Manifest version 2 carries unique canonical restore destinations
   and explicit root-custody requirements. The local upload receipt was deleted entirely: nothing
   consumed it, it could not verify the remote snapshot, and it added a post-upload failure path.

The same pass bounded the Telegram adapter's request line, headers and body, added socket I/O
timeouts, and moved authentication ahead of body allocation. The positive control still reaches
Telegram with the exact credential; unauthenticated and oversized requests reach it zero times.
An Account Service deletion receipt now refuses any nonzero remaining-record count, but no
deprovision command was added: the residual-authority ledger and external controls remain absent
as `docs/PRD.md` says.

Free controls passed after the changes: `cargo fmt --check`, clippy over the workspace and all
targets with warnings denied, `cargo test --workspace`, the offline host-proof controls, and
`cargo run -p ironworks-claims` at 42/42. The localhost-based tests required their normal socket
permission; the first sandboxed claims run reported 41/42 solely because that permission was
withheld, and the permitted rerun passed. No off-box upload, restore rehearsal, or live-host proof
was run, so those operational gates remain unmeasured rather than passing.
