# 0048 — Thirteen fail-open defects, and three findings that did not survive verification

**2026-09-05/06.** A code-quality sweep of the whole workspace — 20 crates, ~45k lines of Rust and
~3k of `dev/*.sh` — looking for stale, kludgy, brittle, duplicated or dead code. Three read-only
audit agents swept the binary layer, the serving-path libraries and the operator-side crates; every
finding below was then re-read and confirmed against source before it was acted on.

**Each fix was reproduced before it was made.** That is the only reason this record is worth
keeping: three of the agents' findings did not survive that step, and one of my own
recommendations was wrong in a way only the reproduction attempt exposed. The withdrawals are as
much the result as the fixes.

Control at the close of the work: `cargo run -p ironworks-claims` reported **38 of 38 claims
holding**, 43 test suites, 904 assertions, 0 failures.

## What the sweep did not find

Worth stating first, because it shapes everything else. The tree is unusually clean by the usual
markers: three `#[allow(...)]` in the entire workspace, zero `TODO`/`FIXME`/`HACK`, no commented-out
code, `unsafe_code = "forbid"`, `todo`/`unimplemented` denied workspace-wide, and layering enforced
by Cargo rather than by convention.

So none of this is litter. What it is instead: **places where a rule the repo states in prose is
not the rule the code implements.** Several defects sit within a few lines of a comment describing
the correct behaviour.

## Fixed

### The seed panicked, and two proofs were measuring nothing

`crates/lifecycle/src/lib.rs::reach` refuses `member_confined` outright, and refuses to skip it —
a stage may only be reached from its immediate predecessor, because that ordering is what stops a
member with an unbounded tool surface becoming routable. The seeding example walked every stage and
panicked there. The `through` values the scripts passed, `activated` and `serving`, were never
stage tags, so the early return could not save it.

| call site | what happened |
|---|---|
| `dev/prove-lifecycle.sh` (twice) | BLOCKED before scoring anything; the second call had no guard at all |
| `dev/prove-confinement.sh` | checked `[ -f "$STORE" ]`, which passes on a file `open` created — so a run **spent a member** measuring a store at `member_minted` while believing it was at `serving` |

`DESIGN.md` tabulated `dev/prove-lifecycle.sh 5 PASS`; that tally could not be reproduced. The seed
now validates `through` and stops at `member_minted`, refusing the rest by name — past that point
the only way in is `crates/lifecycle/src/lib.rs::certify_confinement`, which takes evidence of a
measurement that happened, and scaffolding must not manufacture that (§B-8).

**Two further defects surfaced while fixing it, mutually masking:** the fixture patched
`stage_history` while reconciliation reads `authority_identities`, so check 1 never exercised the
MANAGED path at all; and its assertion was an unanchored `grep "MANAGED \[acme-sales\]"`, which
matches `UNMANAGED [acme-sales]` — so the positive control the other three checks depend on was
passing on the exact result it exists to rule out. Both fixed; the proof now scores 5 PASS with
check 1 genuinely reporting MANAGED.

### Three panics on paths that take text somebody else chose

- `crates/domain/src/address.rs::find_mention` searched a lowercased copy and handed the offset to
  a caller slicing the original. `to_lowercase` is not length-preserving, so a preceding `ẞ` or `İ`
  moved the cut. Worse than the predicted panic: `"ẞ @Multiferroic_bot hi"` returned `"ẞ t hi"`,
  leaking the mention's tail into the model's input. It now searches the original and returns a
  range, which also closes a second half of the same defect — the caller resumed at
  `found + at.len()`, the un-lowercased handle's length, against an offset found with the
  lowercased one.
- `crates/ironclaw/src/model.rs::malformed_answer` bounded its 32-byte window with an arbitrary
  byte offset; a multi-byte character inside it panicked. This runs on **every delivered answer**,
  after the turn is billed.
- `crates/devstore/src/main.rs::percent_decode` re-sliced a `&str` at `i+1..i+3`, so `%€` panicked
  and took the fixture down mid-proof from a query string.

Also in the fixture: the auth token was read off the lowercased header line, so an identity
registered with any uppercase character could never authenticate at all. The header *name* is now
matched case-insensitively and the *value* taken from the original.

### Guards that were switched off by the error they discarded

- `crates/channel/src/lib.rs::parse_acked` — both readers of `cursor_acked` used
  `.and_then(|v| v.parse().ok())`, and `None` from that is indistinguishable from an absent row.
  A damaged value therefore read as "never acknowledged", which disarms
  `AcknowledgementWouldRegress`, and — once compaction has removed the rows the barrier derives
  from — sends the channel offset `0`. That is verbatim the failure `poll_offset`'s own doc cites
  ADR 0006 to rule out: *"a derivation that fell back to zero here would redeliver the entire
  history"*. Both readers now refuse.
- `crates/journal/src/lib.rs::record_operator_required` committed the outcome and its reason
  separately. A crash between them left a row `row_to_record` refuses, and because `unresolved`
  collects into one `Result`, **that single row made the entire operator alarm list return `Err`** —
  a state whose whole purpose is raising an alarm could silence every alarm at once. The two writes
  now share a transaction (`crates/journal/src/lib.rs::set_outcome_in` was split out to allow it).
  Refusing to *read* such a row is left as-is and is deliberate: the schema says a row that has
  lost its reason is *"refused rather than read as a blank"*, and with the write atomic only
  external corruption can produce one.

### A race that fired on the first attempt

`crates/proposals/src/lib.rs::begin_echo` and five sibling transitions read the row, judged it in
Rust, then issued an `UPDATE` with no state predicate and no transaction. The `Mutex<Connection>`
orders that pair within one process and says nothing about two, and the relay is documented as one
cycle per process.

The first test written for this **passed** — two sequential handles re-read at call time, so that
is not the window. A barrier probe then fired on **round 0 of 200**: both handles acquired the echo
intent immediately. This is not a narrow window.

All six transitions now compare-and-swap on the observed state, passing the observed tag as a
parameter rather than rebuilding `may_attempt`/`may_begin` in SQL — the state machine stays in one
place. The probe fires 0/200 after.

### Compaction was dead, and had already drifted

`crates/channel/src/lib.rs::compact` had **no production caller**, so `channel_updates` grew for the
life of a deployment — while `safe_cursor` reads every row and queries the turn journal per `turn`
among them, twice per relay cycle plus once inside `acknowledge`. The cost of a poll was
proportional to every update ever received.

Its `DELETE` had also lost `'command'` from its list, while `cost_a_turn` — described in its own doc
as *"the compaction rule in one predicate"* — says a command was never billed. That is the
enumeration defect `compact`'s own doc warns about, one paragraph below the warning.

The set is now derived from `Disposition::tag` and pinned by a test against both predicates. It
stays an **enumeration** deliberately: its failure direction is retaining a row that could have gone
(table growth), whereas a negation would forget a future billed disposition (lost evidence).
`compact` is now called after a successful acknowledge in the relay, non-fatal, bounded by the
acknowledged offset.

### Records dropped at the catalogue stage were never counted

`crates/records/src/lib.rs::Catalog` dropped unusable rows with no count, and
`crates/cli/src/lib.rs::fetch_records` seeded `unreadable` at zero — so rows lost while *listing*
were invisible and the model was told `Complete` over a book that had quietly shrunk. Measured
through the model-free `--crash-at before-dispatch` path against a devstore whose book held one
blank-named record:

| | reported to the model |
|---|---|
| before | `39 record(s) [complete]` — a record silently vanished |
| after | `39 record(s) [DEGRADED]` |

The healthy path is unchanged: the full fixture book still reports `40 record(s) [complete]`, which
is what `dev/prove.sh` check 1 asserts.

A second case turned up while fixing it: when **every** row is unparseable the catalogue is empty
and `fetch_records` returned `Empty` — a claim that the org *has* no records, when the truth is that
none could be read. That branch now reports `Partial` too.

### Tests that could not fail

Four, all now asserting something. `crates/eval/src/main.rs::exit_code` and
`crates/eval/src/regrade.rs::provenance_of` were extracted so the rules they encode are testable at
all — the former's test asserted `3 != 2` on two integer literals; the latter's hashed `b"different"`
and asserted it differed from the manifest, true for any two unequal strings. A new test reads
`crates/graders/src` from the tree, because a constant cannot notice its own omission.

Each rewritten test has a verified negative control: inverting the BLOCKED/FAIL ranking, and
dropping one `SOURCES` entry, both fail with the right message.

### Smaller

- The published operator page rendered its footer link twice; nothing asserted the footer, and a
  `contains` check could not have caught it. Now count-asserted.
- `crates/channel/src/lib.rs::is_acknowledged` discarded the `Err` the `parse_acked` fix introduced
  while itself returning `Result` — an inconsistency created by this work, corrected in it.

## Withdrawn — findings that did not survive verification

Recorded so they are not raised again.

**`digest16` is not duplicated four times.** The claim was that
`crates/domain/src/composition.rs::digest16`'s body appears inline in `crates/domain/src/supply.rs`,
`crates/domain/src/share.rs`, `crates/services/src/lib.rs` and
`crates/lifecycle/src/confinement.rs`, against its own doc's rule that a second
implementation is the defect a fingerprint exists to rule out. They are in fact *incremental*
hashers with deliberately different framing — NUL separators, `\n`, length-prefixed parts — and two
different output lengths (`take(8)` and `take(16)`). None can call `digest16(bytes)` without
materialising a concatenated buffer; for `services::digest` that is the whole scrap corpus. The only
shared code is a one-line hex tail, and `PolicyFingerprint` is persisted. Not worth the churn.

**`Book::record`'s `Err(_) => Ok(None)` does not lose a count.**
`crates/cli/src/lib.rs::fetch_records` does `Ok(None) => unreadable += 1`, so
`Degradation::Partial` was produced correctly on that path and the comment was accurate. Only the
*catalogue*-stage drop was real, and it is fixed above.

**`reconcile/examples/confine.rs` should not have been deleted.** The analysis stands — its `apply`
half bypasses the policy ceiling, the locked-row skip, the confirmation re-read and the `/session`
binding, then prints success on writes it never checked, and running it would over-confine a member
irreversibly (§L-9). The recommendation to delete it rested on the file being unreferenced *and*
unmodified. `git rm` refused: it carried uncommitted work, made after the check that recommendation
rested on, correcting the very `CredentialPurpose` mislabel this sweep reported. Deleting it would
have destroyed that. **The staleness half of a "dead code" finding expires; re-check it at the
moment of acting, not at the moment of finding.**

## Open

Verified still open at the close of this record.

| | |
|---|---|
| `reconcile/examples/confine.rs` | as above — belongs to whoever holds that diff |
| 7× hand-rolled temp-directory `Dir` helper | the dependency budget rules out `tempfile`, but the copies have drifted; one in `crates/cli/src/main.rs`'s tests uses `process::id()` alone, the shape a sibling copy carries a written lesson about |
| three hand-rolled HTTP servers | `devstore`, `telegram`, `devchannel` share a verbatim header loop and disagree on the status line for no reason — one emits a literal `X` as its reason phrase. All three size a body buffer from an uncapped `Content-Length` |
| `python3` unguarded | `prove-workspace.sh`, `prove-confinement.sh`, `prove-cutover.sh`, `prove-lifecycle.sh` embed it with no `command -v` check, while `prove-host.sh` guards it. Without it they report a BLOCKED that names the wrong cause |
| persisted tag spellings | the disposition↔string mapping is written four times in `channel`; `journal` and `proposals` re-derive predicates in SQL beside the methods that define them; `lifecycle`'s `Drift::KIND_TAGS` claims to be the source for a `CHECK` that is an independently typed literal |
| unresolvable intra-doc link | `crates/lifecycle/src/confinement.rs` cites ``[`Applied`]``, which lives in `reconcile`; the edge runs the other way, and the claims checker only reads `.md` |
| stale generated pages | `local/arc/` and `local/read/` predate the merge documented in `crates/site/src/operator/mod.rs`; `local/operator.html`, the current output, does not exist. Untracked, so cosmetic |
| structural | a relay context struct (four functions thread six identical arguments, 14 call sites, suppressing the lint that names the shape); splitting `crates/cli/src/main.rs`, which is six concerns and the workspace's longest function; and `record::Timestamp` at the 31 stringly-typed `now: &str` sites where the ordering guarantees live |

> **Superseded in part, 2026-09-08.** The three rows above marked `structural`, the `7× Dir` row and
> the `three hand-rolled HTTP servers` row were acted on by
> [`0060`](0060-a-second-quality-pass-and-the-three-recommendations-it-withdrew.md): `crates/cli` is
> a library with two 12-line binaries, `Dir` is `ironworks_testkit::Scratch`, the two fixture
> servers share `crates/devhttp` — `telegram` deliberately does not, and that record says why — and
> the `now: &str` sites in `lifecycle` take `ObservedAt`. The `journal`, `proposals`, `channel`
> and `records` half of that last row is unchanged.
>
> The `unresolvable intra-doc link` row is also closed, and by a mechanism rather than by
> hand: `rustdoc::broken_intra_doc_links` is denied and `cargo doc` is now one of the
> commands `crates/claims/src/toolchain.rs` runs, so *"the claims checker only reads `.md`"*
> no longer holds. It found four more broken links besides the one that row names. Nothing
> above is edited; this is the pointer.

## Limits

**The relay change is not measured end to end.** `compact` being called from the relay is covered by
unit tests and by the model-free control above; what exercises `run_relay` in anger is
`dev/prove-delivery.sh` and `dev/prove.sh`, and both spend model turns and rate-limit budget. That
run is the operator's decision and has not been made.

**Nothing here was measured against a live host or a real tenant.** Every reproduction used unit
tests, an in-process stub, or a disposable `devstore` — no runtime, no model key, no probe.

## Addendum, 2026-09-06 — the first limit was lifted

The limit above stands as written, because it was true when the record was made. It has since been
retired by measurement rather than by argument, and the run is recorded here rather than edited
into the paragraph it supersedes.

`dev/prove-delivery.sh` against a disposable probe at `4cb47cfaf3`, profile `production`, with a
real model key: **4 passed, 0 failed, 0 blocked** — the baseline `DESIGN.md` records.

```
PASS  deliver.known_unsent                1 attempt, 0 chunks accepted, no automatic retry
PASS  deliver.b5_residual_window          2 sends, identical (8adc8bf46396), create=1 fetch=1
PASS  deliver.partial_evidence_holds      1 chunk confirmed; a later 'nothing sent' did not lower it
PASS  deliver.b7_redelivery_after_ack_gap redelivered; create=1 sends=1, offset advanced past the update
```

Check 4 is the one that bears on the change. Compaction now runs after every successful
acknowledge, so a row removed that should have been kept would surface there as a rerun or a
resend across the process death; `create=1 sends=1` with the offset advancing says neither
happened. Check 2 separately establishes that the redelivered answer stayed byte-identical, which
is the property compaction must not disturb.

**The first attempt scored 4 BLOCKED, and that was the harness, not the code.** It ran inside the
30-requests-per-60-seconds window immediately after `dev/probe-up.sh`'s warming turn. Worth
recording because the BLOCKED verdicts *read* like findings — they quoted `delivery=delivered` and
`NEEDS AN OPERATOR — a resend is safe`, which are the checks' own success sentences — since
`degraded` matches the whole output while `why` reports only its last line. A BLOCKED whose reason
line is a success message is a rate limit until proven otherwise.

Still true: nothing here has been measured against a live host or a real tenant.
