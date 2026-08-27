# Account-coordination evaluation

A synthetic answer-quality suite for the service whose stable id is
`relationship-intelligence`. It exercises MultiAgency's dogfooded product behavior without
copying, redacting or deriving from MultiAgency's private records.

Ten invented accounts. Six plant seven coordination obligations:

- across-account selection without sales ranking;
- onboarding readiness from requested-versus-received evidence;
- a later written commitment superseding an earlier one;
- incompatible renewal statements that must remain unresolved;
- offboarding with proven identity and credential closure but unknown additional residual authority;
- an embedded instruction that must be flagged and refused.
- dependency ownership that remains with MultiAgency because no request or handoff is recorded.

The other four — Harbor Point Logistics, Kestrel Analytics, Alder Grove Trust and Bramble Fields
Cooperative — plant no obligation and are the control for the ones that do. Each closes its loops
explicitly and carries recent activity, because an account that is merely empty reads as a gap and
one that is merely old reads as stale; either would be another live issue in disguise. They exist
so an instruction to compress the accounts needing nothing has something to act on: with six
accounts, five of them live by construction, obeying that instruction would have meant dropping a
real issue.

**The graders are not in this directory.** They are `crates/graders`, and the fixture holds only what
a turn is shown: the guidance it is composed under and the records themselves. A grader living
beside its corpus was a grader nothing compiled and nothing type-checked.

Its controls — accepted answers, rejected phrasings, and a self-test in both directions for every
case — are ordinary tests, so they run with everything else and cannot be skipped by forgetting a
flag:

```sh
cargo test -p ironworks-graders
```

Run the model-backed suite after `dev/probe-up.sh` and its printed environment exports:

```sh
cargo run -p ironworks-eval
```

There is no `--suite`: this is the only suite with graders in this repository, so naming it would
be a flag with one value. The driver invokes `ironworks coordinate`, not `turn --service`, so the
product-facing entry point is part of the measured path.

Every case must report exactly ten complete records before it reaches a grader. A partial,
degraded, empty or wrong book is BLOCKED rather than scored. Crash-safe execution and delivery
remain covered by `dev/prove.sh` and `dev/prove-delivery.sh`; this suite measures judgement, not a
second implementation of those state machines.
