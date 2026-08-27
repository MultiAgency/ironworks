---
status: accepted
supersedes: the "fresh product" framing this ADR originally carried
---

# Rust IronWorks is a replacement implementation, specification-compatible, not storage-compatible

**Decision:** This project is a replacement implementation of IronWorks in Rust. It is not a
line-by-line port and does not assume compatibility with existing internal schemas or storage
formats unless compatibility is independently justified. The current IronWorks implementation
serves as the behavioral specification, operational reference, and adversarial test corpus.
Replacement is judged by preserved guarantees and successful migration of real service instances,
not by internal structural similarity.

## Why not either extreme

A **line-by-line port** would have been dominated by byte-identity work that buys nothing: the
Python store records `instructions_sha256` as thread identity, so a composition differing by one
byte refuses every live conversation on cutover.

A **fresh unrelated product** is the worse failure, and less obvious. With no replacement
obligation it becomes easy to simplify away precisely the scar tissue that makes the architecture
trustworthy — every one of those awkward branches is a defect somebody already paid for.

## The line

| Must preserve | Need not preserve |
|---|---|
| trust guarantees | Python/Shell structure |
| tenant/service semantics | SQLite schema |
| isolation invariants | env-file format |
| lifecycle semantics | internal APIs |
| delivery semantics | module names |
| acceptance behavior | current accidental abstractions |
| operator-visible guarantees | |

## Consequences

**The bar for "done" is migration, not evaluation.** A previous draft judged replacement by the
eval corpus alone. That is necessary and insufficient: the real test is that a real service
instance moves across and its guarantees still hold.

**ironworks is a corpus to run, not only prose to read.** The portable parts are its black-box
proofs (`multi/verify/*`, which speak `/v1/responses` and do not care what implements the seam)
and its eval graders. The 1,457 lines of crash-boundary unit tests are not portable, but their
twelve scenarios are, as specification.

**Storage compatibility is a decision, not a default.** Where it is chosen it must be justified
in its own right — for a migration path, say — rather than assumed.

**Invariant extraction precedes implementation.** See `docs/INVARIANTS.md`. Writing more Rust
before the invariants are written down is how the scar tissue gets simplified away.
