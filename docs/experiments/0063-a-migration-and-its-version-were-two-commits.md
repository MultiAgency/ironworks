# 0063 — A migration and its version were two commits, and the control had seven ways to stay green

**2026-09-10.** A read-only review of the staged change set that carries `0046`–`0062` — five
parallel readers over disjoint parts of the tree, every finding re-read against source before it was
reported — and then the fixes. No model turn was spent and no member minted. The host was reached
once, read-only, by the operator's own `dev/prove-host.sh` run at the close; everything else here
establishes what the code does, never what a deployment does.

It is the third sweep after [`0048`](0048-thirteen-fail-open-defects-and-three-that-were-not.md) and
[`0060`](0060-a-second-quality-pass-and-the-three-recommendations-it-withdrew.md), and the shape they
named recurred: **a rule stated in prose that nothing enforced, and a fix that reached one copy.**

## What was measured

| | |
|---|---|
| Control at close | `cargo run -p ironworks-claims` — **43 of 43 claims hold**, exit `0` |
| Suite, through the control | `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check` and `cargo doc --workspace --no-deps` all pass, the last now with `-D warnings` |
| Feature build | `cargo clippy -p ironworks-cli --all-targets --features synthetic-qualification` clean |
| Free proofs | `dev/prove-tenancy.sh` 7/7, `dev/prove-lifecycle.sh` 5/5 |
| Offline script tests | `dev/test-prove-host.sh` 28/28, `dev/test-state-backup.sh` 25/25 (new), `dev/test-ironclaw-artifacts.sh` passes |
| Host proof | `dev/prove-host.sh` against `multi-serve`, run by the operator: **13 PASS, 0 FAIL, 0 BLOCKED** — the widened check 11 (four root-run paths, and every object under the deployment tree), 12 and 13 on the host for the first time |

**No costed proof was run.** The host proof is read-only and was the operator's decision in the
moment; it is the only line above measured where the change serves. The first `dev/prove-tenancy.sh`
run was BLOCKED — its store did not come
up while the control's workspace build ran beside it. Started by hand with the same arguments it
came up, and the script re-run alone scored 7/7. The BLOCKED is recorded because it was one.

## Defects that change what the product does

1. **A crash between two commits locked a store out for good.** `crates/store/src/lib.rs::open_durable`
   ran the caller's migration and then wrote `schema_version` as a separate statement, and both
   callers — `crates/journal/src/lib.rs::Journal::open` and
   `crates/proposals/src/lib.rs::ProposalStore::open` — committed their own `ALTER TABLE` first. A
   crash between the two left the column added and the old version recorded, so every later open
   re-ran the `ALTER` into `duplicate column name`. Both closures' comments said an interrupted
   migration rolls back; the store's own test for the property migrated with no DDL, so it could
   not see it. `open_durable` now owns one transaction for the version read, the migration and the
   version write, and lends the closure a `&Transaction` it cannot commit. Each store has a
   regression that fails on the old code.
2. **Re-running `publish-one` after a rollback silently undid it.** On an already-staged instance,
   `crates/cli/src/lib.rs::run_registry_publish_one` called the durable publish — which replaces a
   differing live file — ignored what it returned, and printed `live registry matches`. HEAD
   compared and refused; the move of the binary into a library lost the comparison. It reads the
   live bytes under the held lock again and refuses anything but identical, and
   `rerunning_publish_one_after_a_rollback_refuses_and_writes_nothing` exits `0` against the old
   binary where `2` is expected.
3. **The restore rehearsal's verifier was at a path nothing installs.** `deploy/rehearse-state-restore.sh`
   called `/opt/ironworks/bin/ironworks`; everything else names `target/release`. A restore that
   succeeded then exited `127`, which is no verdict at all.
4. **Both backup scripts scored a check that never ran as a corrupt repository.** A host without
   `restic`, or a configuration it could not read, exited `2` — the leftover branch was FAIL. Every
   could-not-measure case is now named and exits `3`; `dev/test-state-backup.sh` holds both scripts to
   it offline, with a PASS control beside each FAIL and BLOCKED case.
5. **A guidance rollback that deleted the live file reported restoring an empty one** — `bytes=0`
   and the digest of nothing — while the registry still named that path, so an operator report that
   read as success preceded a boot that would fail. `crates/cli/src/lib.rs::run_guidance_rollback_one`
   now says `removed:`, and a file missing after an ordinary restore is an error; its test failed on
   the old output.

## The control, and the ways it stayed green

`cargo run -p ironworks-claims` passed 43 of 43 on the reviewed tree. Every one of these was a hole
that tree did not happen to fall through — which is the condition in which a control is trusted
least and needed most.

- **A BLOCKED became a FAIL.** `toolchain` filed its could-not-run finding under `toolchain claims`,
  a name no prose had reason to contain, so `crates/claims/src/self_description.rs::check` reported
  it unnamed and the binary exited `2` — *measured, and wrong* — over a measurement that did not
  happen. Self-description now requires every name in `CHECKS` as well as every subject reported,
  which also closes the other hole: a check whose findings were all filed under documents had no
  subject of its own that run.
- **Rewording one line retired the build check.** The vacuity guard in
  `crates/claims/src/toolchain.rs::check` fired only when all four status claims were gone, so
  dropping the `cargo test --workspace … passing` line alone skipped the one check that establishes
  the tree compiles, with no finding. `crates/claims/src/toolchain.rs::unclaimed` reports each
  missing claim BLOCKED.
- **`assert!(ready)` declared `ready`.** `crates/claims/src/paths.rs::declared_by_macro` accepted any
  macro whose first argument was the name. It now requires a macro the same file defines with
  `macro_rules!`, which every declaring macro in the tree is.
- **A check's identity moved with its verdict.** Only `tallies` set a claim distinct from its
  subject, so a check read as `symbol citations` when it held and as `README.md` when it failed, and
  two checks failing in one file merged. `crates/claims/src/lib.rs::Check::findings` claims every
  document-filed finding by its check's name, in the one place that knows which check produced it.
- **Free was prose.** Both documents said a script missing from the free list is a defect, and
  `crates/claims/src/dev_costs.rs::check` only held costly scripts to a list. A free script now has
  to be in each document's free list, and a costly one out of it.
- **A dependency without backticks was dropped before the comparison** in
  `crates/claims/src/layering.rs::rows`. It is now reported.
- **`clean` had come to mean "builds".** The rustdoc arm read only `cargo doc`'s exit status, and
  only `rustdoc::broken_intra_doc_links` is denied, so `DESIGN.md`'s `clean` held over twelve
  warnings — ten public docs linking private items, an unclosed `<slug>` tag and a redundant link
  target. `crates/claims/src/toolchain.rs::check` now runs that arm with `-D warnings`, and the
  twelve are fixed. Only that arm: `cargo test` hands `RUSTDOCFLAGS` to every doctest as well.

One finding from this group did not survive being read as a defect in the check: the experiment
index is matched by number, and its doc comment says so on purpose. `CLAUDE.md` claimed it named
"every numbered write-up and every transcript", which was the overstatement. The prose was corrected.

## The evidence state

- **An id admitted a sentence.** `crates/evidence/src/state.rs` said its ids refuse anything that
  could hold prose and refused only line breaks and control characters, so any one-line sentence
  under 128 bytes was a `ClaimId`. Whitespace is now refused; `record-findings` joins the control's
  check names with `-` at the one boundary that reads them.
- **One column held two clocks.** `observed_at` was a bare string: the recorder wrote Unix seconds
  and every test wrote RFC 3339, and a clock before 1970 was recorded as `0`. It is now a `Utc`,
  minted through `crates/domain/src/attestation.rs::Utc::from_unix_seconds`, and a row in the other
  spelling reads as an error.
- **A task could write the document.** `crates/evidence/src/plan.rs::Task::parse` bounded length and
  not lines, so a task could render `## Preserve — derived from evidence, not chosen` itself. It
  refuses line breaks and control characters.

## Tests that could not fail

- Both credential-purpose tests in `crates/reconcile` resolved `env:` handles nothing sets, so they
  passed with the purpose guard deleted. They use `env:PATH` now, beside a positive control, and
  deleting the guard fails them.
- `crates/ironclaw/src/classify.rs`'s shell-agreement test built every cause with empty text, so the
  shell's provider clauses were compared against nothing — and they disagreed: a refusal relaying
  `provider nearai unavailable` was BLOCKED to `dev/lib.sh::degraded` and a measurement in
  `Cause::blocks_measurement`. The shell reflects measured behaviour (`0044`), so the Rust side moved;
  the test now gives every kind every sample text, `0044`'s digest among the negatives. **It then
  found the thirty-second cliff meaning two things**: a create refused `service_unavailable` after
  the runtime's own wait ran out was BLOCKED to the shell's `unsettled` and a measurement to Rust,
  whose doc already counted "a wait that ran out" as unmeasured. Rust now mirrors all three shell
  predicates — `Cause::dependency_unavailable` for `degraded`, `Cause::outran_wait` for
  `outran_wait`, and `Cause::blocks_measurement` as their disjunction for `unsettled` — and the test
  reads each pattern out of `dev/lib.sh` and compares each pair. It failed on the old rule at
  `Undecodable("service_unavailable")`.
- `publishing_written_guidance_warns_about_nothing` asserted that `publish-one` printed no `WARNING`,
  on a path that stopped printing one when `Guidance::parse` began refusing stubs. Its success half
  repeated another test. Deleted.

## Found while fixing

- **Check 11 did not see a unit that runs as root without `+`.** `ironworks-attest@.service` is
  `User=root` and runs a script from `/opt/ironworks/deploy`, the exposure
  [`0058`](0058-the-serving-host-ran-its-own-preflight-from-a-directory-the-login-account-could-write.md)
  closed for `+` lines. `dev/prove-host.sh` now walks every command systemd runs as root, reading the
  effective `User=` from the host, and blocks on a unit it cannot read. That script runs the
  `ironworks` binary in turn — a path no unit line names — so whenever a root-run path lies under
  `/opt/ironworks` the whole tree is scanned too, failing on any object not owned by root or
  writable by group or other and blocking on a tree it cannot read. Removing the scan makes the
  binary-offender fixture pass, so the fixture tells the two apart.
- **A `case` pattern's `)` inside the remote heredoc ended the command substitution on bash 3.2**,
  the shell macOS ships, so part of the remote walker would have run locally. `bash -n` and
  shellcheck passed it; the offline fixtures did not.
- `dev/prove-host.sh` gained checks 12 and 13 — the attestation timer, and the age of the evidence
  it keeps — because a stopped timer refuses every new turn while every unit stays active.
- **The binary kept its own calendar, with the same fallback.** `crates/cli/src/lib.rs::now_iso`
  mapped a clock before 1970 to `0`, as the recorder did, through a second copy of the civil-date
  conversion. It now mints through `Utc::from_unix_seconds` and stops on a clock it cannot read as
  an instant — a panic in place of a false timestamp every store would have accepted.
- **Ten doc comments had come loose in the move into a library**, each left on whatever item had
  followed its function — five found by the review, five more by comparing every item against HEAD.
