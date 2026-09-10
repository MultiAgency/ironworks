# 0046 — One failing update took its whole batch with it

Date: 2026-09-05. **Measured with no runtime and no model key, which is the point: the failure is
deterministic and costs nothing to reproduce.** Two tenants, two updates, one `relay` cycle, a
record store that is not listening. Before the change the cycle stopped at the first update and the
second was never attempted; after it, both are attempted and the cycle reports a count.

No transcript file — this measures **which updates a cycle touches**, not answers, and no grader is
in the path. The write-up is the evidence.

## Why it was measured

`crates/cli/src/lib.rs::run_relay` propagated with `?` inside its per-update loop. The concern was
raised against the `Refusing WITHOUT settling` branch of `crates/cli/src/lib.rs::drive` — the one
`0033` added so a degraded read could be retried on the next cycle rather than settling — but the
shape is general: any per-update error ended the cycle.

`§E-3` journals the whole batch before any worker starts, so nothing is *lost* when that happens:
the untouched updates stay open, hold the barrier, and are redelivered. B8 is satisfied either way.
The question was narrower and had not been measured: **does one tenant's outage delay every other
tenant's traffic by a whole cycle?**

## The run

Fixture: two tenants in one registry on two channels, `IRONWORKS_RECORDS` pointed at a closed port
so `Book::resolve` fails closed for both, one update each in a single batch. Deterministic — a
refused TCP connect, not a timeout, and no model is reached.

```
── HEAD (d98eedd)
relay: 2 tenant(s) in registry.json
relay: poll offset=0
relay: batch of 2 journalled
ironworks: instance coordination-eval: refusing to serve — the record store could not be reached: io: Connection refused
exit 2
```

**Update 702 is not mentioned.** It was journalled, it was open, and it was never attempted. Its
tenant's store was fine; its only defect was being second.

```
── with per-update containment
relay: 2 tenant(s) in registry.json
relay: poll offset=0
relay: batch of 2 journalled
relay: update 701 did not settle this cycle — instance coordination-eval: refusing to serve — the record store could not be reached: io: Connection refused
relay: update 702 did not settle this cycle — instance tenant-b: refusing to serve — the record store could not be reached: io: Connection refused
ironworks: 2 of 2 update(s) did not settle this cycle; the rest were served. Each is reported above.
exit 2
```

Both attempted, both reported, one count. In this fixture both tenants fail because both share the
one unreachable store — which is what makes it a clean *control on the loop* rather than on the
tenants: the assertion is that the second update is **reached**, not that it succeeds.

## What the exit code does, and why it did not change

Both runs exit non-zero, deliberately. Forward progress is reported by the **cursor**, not by the
exit code — `0033` asserted exactly that (`poll offset=43` → `poll offset=44`), it is durable where
an exit status is not, and `dev/prove-continuation.sh`'s `cursor.released` already checks it. A
cycle that settled a turn for an operator should still make `dev/relay-loop.sh` back off, because
whatever produced the failure may produce it again on the next update. Adding a second success axis
would be two mechanisms for one fact.

## What this establishes, and what it does not

**Establishes.** At HEAD, one update that cannot be served ends the cycle before the updates behind
it are attempted, across tenants, with the batch already journalled. With per-update containment
the rest of the batch is attempted in the same cycle and each failure is reported separately. The
cycle's exit code is unchanged in both directions.

**Does not establish.** Nothing about a settled turn releasing the cursor — that is
`crates/journal/src/tests.rs` and `crates/channel/tests/crash_boundaries.rs`, and end to end it is
`dev/prove-continuation.sh`, **which was not run for this write-up**: no probe and no model key were
available in the session that made the change. Its four new arms — `settles.cancelled`,
`cancelled.released`, `accepted.recovers`, `batch.contained` — are therefore **written and
unmeasured**, and this file must not be read as their result. Nothing about two tenants where only
one fails: both failed here, because that is what a closed port does to a shared store setting.
Nothing about a deployment.

## The related change this run does not cover

The same pass routed four more outcomes in `crates/cli/src/lib.rs::finish` into `OperatorRequired`
— `Failed`, `Cancelled`, `Incomplete`, and a completed answer the delivery gate refuses. Each left
its row `accepted`/`completed` with delivery `pending`, which `TurnRecord::openness` reports as
`Open`, which pins the safe cursor across every tenant: `0033`'s second defect reached by four more
routes. That is covered by unit tests and by the two proof arms above, not by this run.

Its evidence is a source read rather than a measurement, and is recorded in `DESIGN.md`'s upstream
contract: at the pin a failed response's `error` is runtime-authored and constant — `code` is a
closed enum whose only reachable value there is `internal_error`, and `message` is derived from
`code` rather than passed through. `0013` measured that same constant standing in for a provider
**rate limit**, which is why no durable operator reason quotes it as a cause.
