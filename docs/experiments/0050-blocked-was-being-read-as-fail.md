# 0050 — BLOCKED was being read as FAIL, and a test stub was wedging the repository

**2026-09-06.** A code-quality pass over the working tree, then over the whole workspace. No
runtime, model key, member, channel, record service or live host was contacted: every number below
comes from running the repository's own binaries and scripts against local input, or from reading
source. Control at the close: `cargo run -p ironworks-claims` reported 38 of 38 claims holding on
the tree as it stood, on a tree carrying uncommitted work from another session.

Two measurements are worth keeping. Both are cases where a mechanism reported the wrong one of
three values, and neither was visible as a failing test.

---

## 1. Seven of eleven blocking causes were scored FAIL

`MEASUREMENT` · counterpart to `0044`

`degraded()` in `dev/lib.sh` decides `BLOCKED` versus `FAIL` for every `dev/prove*.sh`: the
scripts read
`if no_model … elif degraded … else FAIL`, so a cause neither predicate matches is scored as a
product failure. It was written by hand against `ironworks_ironclaw::Cause`'s prose and had drifted
from it.

**Method.** Each `Cause` variant's own `Display` string, passed through the two predicates exactly
as the scripts pass a run's output.

| `Cause` renders | before | after |
|---|---|---|
| `Resolve` — "the runtime host did not resolve" | FAIL | BLOCKED |
| `Connect` — "the connection was refused or unreachable" | FAIL | BLOCKED |
| `Proxy` — "the egress proxy refused to tunnel" | FAIL | BLOCKED |
| `Tls` — "the TLS handshake failed" | FAIL | BLOCKED |
| `SendTimeout` — "timed out writing the request" | BLOCKED | BLOCKED |
| `ReceiveTimeout` — "timed out awaiting the response" | FAIL | BLOCKED |
| `BudgetExhausted` — "the turn budget ran out while it was running" | FAIL | BLOCKED |
| `Transport` — "transport failure: …" | FAIL | BLOCKED |
| `Unauthorized`, `Malformed`, `Undecodable`, `Rejected`, `IdempotencyConflict` | measured | measured |

`SendTimeout` is the positive control: it matched `timed out (writing|reading)` and was the only
one that did, so "the predicate matches nothing" and "the predicate matches everything" are both
excluded. The last row is the negative control — a pattern that blocked those five would hide real
failures, and the check below asserts they stay measurements.

**`ReceiveTimeout` is the one that mattered.** It is the runtime's ~30 s synchronous limit, the most
common blocked condition there is, and the predicate looked for `timed out reading`, which nothing
in the tree has ever emitted. Producer counts at the time: `not a valid model name` 0,
`model provider rejected` 0, `rate limited` 0, `timed out reading` 0, `timed out writing` 2.

**This is `0044` in the other direction.** That record narrowed `degraded()` because a bare `429`
matched inside a hex digest and reported BLOCKED on two of eight sittings that had delivered
cleanly, and it closes by saying a false BLOCKED is the expensive kind of wrong. It is — and the
same function was simultaneously failing to match seven causes that genuinely blocked. Narrowing a
predicate and widening it are the same edit to the same line, which is why one hand-written
pattern held both defects at once.

**Fixed by moving the rule into the type.**
`crates/ironclaw/src/classify.rs::Cause::blocks_measurement` states it once, beside the structured
value; `degraded()` in `dev/lib.sh` carries its shell spelling for the provider prose the seam
does not
author and cannot enumerate. `crates/eval/src/outcome.rs::turn_timeout` matched the same dead
`timed out reading` and now matches what `Cause` renders.

**The agreement is machine-checked, not asserted.**
`crates/ironclaw/src/classify.rs::the_shell_agrees_about_which_causes_block_a_measurement` reads
`dev/lib.sh`, extracts the pattern, and runs the same `grep -E` bash will run against every
variant's own words. Verified by mutation: restoring the shipped pattern fails the test, naming the
first variant it disagrees about; `dev/lib.sh` was restored byte-identical afterwards.

**What this does not establish.** No proof script was re-run against a real runtime here, so this
says the classification is now consistent with `Cause`, not that any particular sitting changes
verdict. `no_model` was left alone: its phrases describe what a *provider* says through the runtime,
which `0013` measured this pin does not surface — a separate question from this one.

---

## 2. A test stub with no timeouts wedged the whole repository

`MEASUREMENT` · infrastructure, not product

`crates/cli/tests/registry_publish.rs` and `crates/cli/tests/lifecycle_adopt.rs` each carried a
byte-identical single-shot HTTP stub: one unbounded `accept`, one unretried `read`, and a
`JoinHandle` the test unconditionally joined.

**Method.** The built integration binary, run directly, single-threaded and at default parallelism.

| | result |
|---|---|
| `--test-threads=1` | 12 passed, 0 failed, 4.6 s |
| default parallelism | **5 of 12 failed**, 0.15 s |
| after the fix, both modes | 12 passed; 20 consecutive parallel runs of both binaries clean |

The parallel failures panicked in the stub thread at its one `read`, with
`Os { code: 35, kind: WouldBlock }` — the accepted socket inherits the listener's non-blocking flag
on macOS — and the test then failed on `join().unwrap()` with `Any { .. }`.

**The expensive mode was the other one.** When the spawned `ironworks` child never connected at all,
`accept` blocked forever and the binary never exited — so it held cargo's build lock and nothing in
the repository could build. Seven such processes were found alive at once, the oldest at 1 day
4 hours elapsed, each holding ~14 idle `LISTEN` sockets on 0.28 s of CPU. It presents as a slow
build, not as a red test, in both sessions working the tree.

**Fixed by one bounded stub**, `crates/testkit/src/lib.rs::serve`, used by both targets and by
the policy fixture that had become a third copy: an accept deadline, a read timeout, a loop until
the request head is complete, and `Option<String>` so "no client ever connected" cannot pass an
assertion that only checks a prefix.

**Diagnostic worth keeping.** A wedge presents as a slow build. `ps -o pid,etime,time` showing a
test binary with large elapsed time and near-zero CPU is a wedge, not a build; re-run that one
binary single-threaded rather than the workspace.

---

## Withdrawn — reported, then found not to be defects

> **Superseded in part, 2026-09-08.** The adapter-bearer read below is now `Var::AdapterToken` in
> `crates/cli/src/lib.rs`, and the fail-closed reasoning it rests on is stated once on that variant
> rather than at one of its two call sites — the withdrawal stands, the duplication it noticed does
> not. The bounded stub this record cites also moved to `crates/testkit`. See
> [`0060`](0060-a-second-quality-pass-and-the-three-recommendations-it-withdrew.md). Nothing below
> is edited.


**The relay's empty adapter bearer.** `crates/cli/src/main.rs` reads `CHANNEL_ADAPTER_TOKEN` with
`unwrap_or_default()` where every sibling credential is required, which reads as the one credential
that can be silently absent. It is deliberate and documented at the call site: an empty bearer is
refused by `crates/telegram/src/main.rs::AdapterToken::presented`, which answers *no* to an absent,
unreadable or wrong value, so it fails closed at the boundary that owns the credential.
`ironworks-devchannel` ignores the header, which is why no `dev/` script sets the variable —
requiring it in the relay would block every proof script for no gain.

**`eval` scoring BLOCKED as FAIL.** Claimed while investigating finding 1, and wrong.
`crates/eval/src/outcome.rs::classify` falls through to its `exit_code != 0` arm, so a provider
failure still returns `Blocked`; the dead predicates cost the *reason*, not the verdict. The FAIL
misclassification was real only in the shell, where the `else` branch is a verdict rather than a
fallthrough. Recorded because the difference between the two is the whole finding.

---

## Also changed in this pass

Not measurements — each is a defect reproduced, fixed, and covered by a test that fails against the
old behaviour.

- **A promotion could be made for the wrong organization.** `boot_tenant` compares the record
  store's answer to the registry's `org` and refuses on mismatch; the command path reached
  `share_scope`, which resolved the same org and compared nothing, and `/confirm` writes a durable
  activity into whatever the store answers. `crates/cli/src/lib.rs::agreed_org` is now the one
  control both paths call.
- **A command update could hold the channel open forever.** A durable `share_scope` failure left the
  update `Unclaimed`, and `Disposition::Unclaimed` is `Openness::Open`, so it held the process-wide
  safe cursor — the shape `0033` measured on the turn path. Durable refusals now settle through
  `finish_command`; `ScopeFailure::Unreachable` still stays open and retries.
- **Records were fetched serially, one round trip per account, per turn.**
  `crates/cli/src/lib.rs::in_order` fans out over contiguous chunks and concatenates, so the order
  cannot depend on timing — the envelope renders records in the order it is given them and ADR 0005
  requires that body to rebuild byte-identically. Pinned by a test whose closure completes in
  reverse order, with a collect-as-completed implementation confirmed to fail it.
- **The arc page's published figures were scraped from terminal output.**
  `crates/site/src/arc/data.rs::compose` ran `target/debug/ironworks compose` and parsed it
  positionally with `unwrap_or(0)` and a `_ => {}`. It now melts in process. Two silent modes gone:
  a reworded line publishing `0 bytes`, and a stale binary publishing a composition the tree no
  longer had — which happened while capturing the baseline for this change, because the binary
  would not rebuild. Output verified byte-identical: same alloy digest, byte counts, and parts in
  the same order.

---

## Limits

Source reads and local runs only. Nothing here was measured against a runtime, a model, a member, a
channel, a record service or a live host, so this establishes what the code and the scripts now
classify, never what a deployment reports. The tree carried another session's uncommitted work
throughout; every failure was attributed before it was acted on, and the ones belonging to that
work were left alone.
