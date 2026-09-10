# 0060 — A second quality pass, and the one recommendation it withdrew

**2026-09-07/08.** A whole-workspace sweep for low-quality code, planned in five stages and worked
through. It is the successor to [`0048`](0048-thirteen-fail-open-defects-and-three-that-were-not.md),
which swept the same tree eight days earlier; several of its "structural" recommendations are
carried out here, and two of its findings are now stale in the way a dated record is allowed to be.

**The premise that shaped the whole pass.** The usual smells were absent, and measuring that first
is what made the rest credible: across 20 crates and ~53k lines there was **one** `#[allow]`, zero
`unsafe`, zero `todo!`/`unimplemented!`, no commented-out code, and eight `unwrap`/`expect` in
serving-path production code — every one but two carrying a documented invariant. So the defects
were not lint debt. They were **size, placement, and copies that had already drifted apart**.

That last is the shape worth naming, because it recurred: **a fix lands in one copy and not its
siblings.** `dev/lib.sh` exists because eleven copies of `verdict` drifted and a `degraded` fix
reached two of three. The same failure was found in Rust in six more places.

## What was measured

| | |
|---|---|
| Control at close | `cargo run -p ironworks-claims` — **43 of 43 claims hold** |
| Suite | `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check` and `cargo doc --workspace --no-deps` all clean |
| Free proofs | `dev/prove-tenancy.sh` 7/7, `dev/prove-lifecycle.sh` 5/5 |
| Crates | 20 → 23 (`store`, `devhttp`, `testkit`) |
| `#[allow]` in the workspace | 1 → **0** |

**No costed proof was run.** Nothing here changes a turn's request bytes, and the serving path is
covered by the free proofs plus the suite. `dev/prove.sh` and its siblings spend model turns and a
rate-limit budget; spending them to re-measure a refactor whose tests already bite would be
ceremony, not evidence.

## The eight live defects

Each was reproduced before it was fixed, and each fix has a test that was then **mutated to confirm
it fails** — the step that makes "green" mean something.

1. **A turn could be filed under the wrong tenant, silently.** `crates/journal/src/lib.rs::row_to_record`
   decoded nineteen columns **by position**, against a column list written out twice. `instance` and
   `org` are adjacent and both `TEXT`, so reordering one list would compile, run, and mislabel a
   turn. One `TURN_COLUMNS`, decode by name, and a test that selects the columns reversed.
2. **`lifecycle confine <slug> --store --dry-run` applied the confinement.** Five subcommand parsers
   were written by copy-paste; two read `--store` as `it.next().cloned()`, which set the store to
   the literal `--dry-run` and consumed it. `confine` spends a member and approval-off is one-way
   (§L-9), so this was the most expensive flag in the binary to mis-parse.
3. **A damaged `schema_version` read as "fresh store".** `journal` and `proposals` folded an
   unparseable value into the same `None` an absent row gives, and the absent arm stamps the current
   version onto a database whose shape was never established. `channel` refused, but reported
   `found: -1` — a number never on disk. All three now distinguish the three cases. The sibling
   lesson was already written down one field over, in `crates/channel/src/lib.rs::parse_acked`.
4. **`eval` still recovered the transcript's `ingredients` from terminal output by field position** —
   `split_whitespace`, `parse().unwrap_or(0)`, and a `_ => {}` — which is the exact technique
   `crates/site/src/arc/data.rs::compose` was rewritten to remove. The fix had landed in the reader
   and not in the writer, and the writer is what the published page reads.
5. **Two proof scripts exited 2 without a tally**, so a run whose measurement never happened could
   report a measured failure. The inversion [`0050`](0050-blocked-was-being-read-as-fail.md) is about.
6. **Tenant export's staging leaked a `.pending` file**, and because it used `create_new`, every
   later run then failed with "cannot create". Its sibling in the same crate had always synced
   correctly.
7. **The crash sweep covered the registry path and not the artifact path** — the one a guidance
   publication takes since [`0049`](0049-guidance-publication-became-the-serving-change-path.md).
   One `publish_durably` now serves both, and the sweep derives its boundary count from a clean run.
8. **`fetch_records` bound every failure to `_`**, so a revoked records token and a DNS outage
   produced byte-identical operator output.

## The copies that had drifted

`store` (the SQLite open protocol, three copies — where defect 3 lived), `devhttp` (the fixtures'
request reader and response writer, two copies whose status lines disagreed three ways and only one
of which had the case-folding fix), one `boot` (two functions where only one applied `agreed_org` —
whose own doc says *"a control applied to the first and not the second is a control with a way
around it"*), one SHA-256, one operator agent, one `scan` in `graders/src/text.rs` where there were
five, two transcript decodes where there were five, one check registry in `claims` where there were
three, and one experiment-number rule where there were four **with three different length bounds**.

## One recommendation withdrawn, and two that were wrong to withdraw

The reason this record is worth keeping, as in `0048` — including that two of the three
withdrawals below did not survive being questioned a second time.

- **~~A `rustdoc::broken_intra_doc_links` lint was added and then removed.~~ Reinstated, and
  enforced.** The withdrawal was correct about the diagnosis and wrong about the remedy: a denied
  lint nothing invokes is worse than none, but the answer was to invoke it rather than to drop it.
  `crates/claims/src/toolchain.rs` now runs `cargo doc --workspace --no-deps` as a fourth cargo
  invocation — 107s on a control that already takes about eight minutes — and `DESIGN.md`'s status
  block states the claim it checks.

  **It found four broken links nobody knew about**, on top of the one it was written for. One of
  them is `crates/lifecycle/src/confinement.rs` citing ``[`Applied`]``, which
  [`0048`](0048-thirteen-fail-open-defects-and-three-that-were-not.md) recorded as a finding it
  could not act on — *"the edge runs the other way, and the claims checker only reads `.md`"*. That
  is the argument for the cost: `.rs` doc comments now get the same guarantee
  `crates/claims/src/paths.rs::check_symbols` already gives citations in `.md`, and in this
  repository the doc comments **are** the design record.
- **An `Html` newtype was planned for `crates/site` and not built** — that part stands. The claim
  was that it would make a missed `esc()` impossible, and it would not: `format!` accepts anything
  implementing `Display`, so a raw `String` interpolates just as happily.

  **What was wrong was stopping at one renderer.** Three generators interpolate untrusted text, and
  the first attempt covered only the smallest. All three are covered now, each by rendering with a
  payload in **every** field the input carries: `read/render.rs` (13 escape sites), `arc/render.rs`
  (**37**, and the values are transcript filenames, case names, part paths and digests), and
  `operator/mod.rs`, whose five sites needed `body` split out of `build` before they were reachable
  at all — `build` touches the filesystem twice before it formats anything.

  The operator test asserts **both directions**, because that page carries the markup/text
  distinction by memory: `arc.html`, `reading.binding` and `reading.cases` are already HTML and must
  *not* be escaped again, while five sibling fields must be, and all eight are `String`. Escaping
  the binding sentence fails the test as loudly as forgetting to escape the transcript name. Each
  test was verified by dropping a real `esc` and watching it fail.

  The old test in `crates/page` asserted only that the escaper escapes, while its comment claimed
  the generators never interpolate raw text; that comment now points at the tests that check it.
- **The seven graders were not made a gate table.** The audit read them as one shape retyped seven
  times. On inspection one of the seven is a pure gate chain; the other six compute something
  *between* their gates — `attention` counts connected accounts before testing that count — and mix
  "must contain" with "must not contain". A `fn(&str) -> bool` table cannot see a local, so every
  gate would re-derive it: more code, repeated work, and a rewrite of the instrument that pins every
  recorded verdict. No drift was found between the seven. The reasoning is recorded in
  `crates/graders/src/cases.rs` beside `CASES`.

## The stringly-typed timestamp, finished — and what it found

`0048` counted **31 `now: &str` sites where the ordering guarantees live**. The `lifecycle` half was
done first; the rest — `journal`, `proposals`, `channel`, `records` — is done here, and doing it
turned up more than a type change.

- **There were two parsers for one rule, and the ledger's was the weaker.**
  `ironworks_lifecycle::ObservedAt` checked length, punctuation, digits and the `+00:00` offset and
  **never the calendar**, so `2026-13-45T99:99:99+00:00` parsed and sorted between two real
  instants. `ironworks_domain::AttestationTime` checked the same shape *and* the calendar, leap
  years included, and `crates/journal` already stored a confinement observation as one. They are now
  one type.
- **Which is `domain::Utc`,** renamed from `AttestationTime`. The name came from the first thing
  that needed it; the rule is not about attestation. The turn journal's `first_seen`, the channel
  journal's `received_at` and the ledger's `at` are the same rule, and `ORDER BY first_seen, key`
  rests on it.
- **The crash-boundary tests were passing `"t0"`, `"t1"`, `"t2"`, `"t3"`, `"t9"`** into the column
  that `ORDER BY` reads. Not timestamps at all. Nothing caught it, because the parameter was `&str`.
- **`now_iso` returns the type**, so the value is checked once where it is minted rather than at
  each of the thirty-one places it is written. The one `expect` in it is the one place the invariant
  is provable: the format string two lines above **is** the shape the parser accepts.
- **`records::Append::occurred_at` takes a `Date`.** Its grammar was stated in a comment, and the
  shape check was `.len() != 10` on a byte-slice of a stored timestamp — which accepts
  `abcdefghij` and hands it to the store to refuse as `invalid_occurred_at`, a round trip to learn
  what the shape already says.

**Where it deliberately stops.** The *parameters* are typed; the *fields read back from a row* stay
`String`. `TurnRecord::retrieved_at` is the clearest case: it goes into the envelope, and a
recovered turn reuses the journalled value so the reconstruction is byte-identical (ADR 0005). Typing
it would mean re-parsing rows written before this change, on the recovery path, to gain a guarantee
the write side now provides anyway — a new failure mode for a journal whose own error message tells
an operator not to delete it. The guarantee belongs at the write.

## Two tests that were passing for the wrong reason

- `crates/cli` asserted `!matches!(f, BookFailure::ScopeDrift { .. })` over an array that
  enumerated four non-`ScopeDrift` variants, then built an unrelated value. **Deleting
  `fetch_records` would not have failed it.**
- `crates/lifecycle` asserted `store.adopt(…, "bad").is_err()`, and its own comment admitted why:
  *"the v3 table rejects this timestamp **after** the instance and stage rows have been attempted."*
  A rollback test wearing a validation test's name. `adopt` now takes `ObservedAt`, so `"bad"` does
  not compile, and the two claims are asserted separately.

## What this supersedes

- [`0048`](0048-thirteen-fail-open-defects-and-three-that-were-not.md) recommends *"splitting
  `crates/cli/src/main.rs`, which is six concerns and the workspace's longest function"*, and names
  the seven-copy `Dir` helper and the 31 `now: &str` sites. `crates/cli` is now a library with two
  12-line binaries; `Dir` is `ironworks_testkit::Scratch`; the `lifecycle` half of `now: &str` takes
  `ObservedAt`. The rest of that surface — `journal`, `proposals`, `channel`, `records` — is
  unchanged and still worth doing.
- [`0050`](0050-blocked-was-being-read-as-fail.md) records the relay reading
  `CHANNEL_ADAPTER_TOKEN` with `unwrap_or_default()`. That read is now `Var::AdapterToken`, and the
  fail-closed reasoning it relied on — an empty bearer a real adapter refuses — is stated once
  rather than at one of its two call sites. It also cites the bounded stub, which now lives in
  `crates/testkit`.

Neither record is edited. This is the pointer.

## What was left

`crates/cli/src/lib.rs::TurnDriver::drive` (409 lines), `::run_relay` (354) and
`crates/site/src/arc/render.rs::sections` (343) are still long. The discipline that kept the rest of
this pass honest applies to them too and is why they are not done here: **only extract what you can
then test.** `operator::body` is what that looks like when it does pay — it was split out precisely
because five escape sites were otherwise unreachable, and it came with the test that reaches them. `run_registry_publish_one`'s four identity rules were worth extracting because they are
policy that previously needed an Account Service, an IronClaw, a SQLite store and a filesystem to
reach; they are now two pure functions with a test asserting both directions of each. An extraction
that produces no new test is a rearrangement, and this tree has enough of those already.
