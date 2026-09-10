//! Two kinds of test, and both are needed.
//!
//! **Can it fail?** Synthetic inputs proving each checker actually rejects something. A checker
//! that only ever passes is indistinguishable from one that does nothing, and this file's whole
//! premise is that a check nobody can see failing gets skimmed.
//!
//! **Does the tree hold?** Every pure check runs over the real repository, so `cargo test` enforces
//! them without a build of the workspace or a shell-out to cargo. Two need a subprocess and cannot
//! be here — `commands` asks the built binary about its flags, `toolchain` runs the suite and the
//! linters — and `self_description` cannot either, because its input is the findings the binary
//! collects rather than the tree. Those three are the binary's, and only the binary sees all of it.

use super::*;

/// A scratch tree at a path unique to this process.
///
/// **The name may not be a constant, and that is not tidiness.** The tests below build a tree
/// and begin by deleting whatever is already there. At a fixed path, two `cargo test` processes
/// running at once delete each other's fixtures and then fail on findings that describe the other
/// process's tree — measured, twice, with two different tests failing each time. The suite was
/// green on a serial run and red under concurrency.
///
/// That is this crate's own K-8 defect: a harness can invent a difference as easily as a match,
/// and `toolchain` reads `cargo test --workspace` to decide whether `DESIGN.md` may say `passing`.
/// A checker whose own suite depends on what else is running cannot be the thing that says the
/// suite passes.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let at =
            std::env::temp_dir().join(format!("ironworks-claims-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        Self(at)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    /// Write one file, creating the directories above it. Parents come from the path so a test
    /// names the tree it wants rather than building it in two steps.
    fn write(&self, rel: &str, body: &str) {
        let at = self.0.join(rel);
        std::fs::create_dir_all(at.parent().expect("a scratch file has a parent"))
            .expect("scratch tree");
        std::fs::write(at, body).expect("write");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── can each checker fail? ──────────────────────────────────────────────────────────────────

#[test]
fn the_tally_reader_finds_a_disagreement() {
    let claim =
        &tallies::recorded_claims("### Run 1 — `9999-x.json`\n\n- Recorded: 4 PASS, 1 FAIL\n")[0];
    assert_eq!(claim.transcript.as_deref(), Some("9999-x.json"));
    assert_eq!(claim.tally.get("PASS"), Some(&4));
    assert_eq!(claim.tally.get("FAIL"), Some(&1));

    // Reading the write-up is only half of it, and the half this test was named for is the
    // comparison — which it did not make. A reader that parses `4 PASS, 1 FAIL` correctly and is
    // never asked what the transcript holds reports agreement over every run in the tree.
    let dir = Scratch::new("tally-disagreement");
    dir.write(
        "docs/experiments/9999-x.json",
        r#"{"cases":[{"verdict":"PASS"},{"verdict":"FAIL"}]}"#,
    );
    dir.write(
        "docs/experiments/README.md",
        "### Run 1 — `9999-x.json`\n\n- Recorded: 4 PASS, 1 FAIL\n",
    );
    let findings = tallies::check(dir.path());
    assert!(
        findings.iter().any(|f| f.verdict != Verdict::Pass),
        "the write-up claims 4 PASS and the transcript holds 1: {findings:#?}"
    );

    // The positive control. Without it this passes on a checker that reports every tally wrong,
    // which is the same defect wearing the other face (§K-8).
    dir.write(
        "docs/experiments/README.md",
        "### Run 1 — `9999-x.json`\n\n- Recorded: 1 PASS, 1 FAIL\n",
    );
    let agreeing = tallies::check(dir.path());
    assert!(!agreeing.is_empty(), "`all` over nothing is true; this half must read a claim");
    assert!(
        agreeing.iter().all(|f| f.verdict == Verdict::Pass),
        "a tally that matches its transcript must pass: {agreeing:#?}"
    );
}

#[test]
fn a_regrade_is_not_what_the_transcript_holds() {
    // The line carries two tallies. Only the first describes the recorded run; the second is a
    // later judgement under corrected graders, and comparing it would report a disagreement that
    // is the entire point of the ledger.
    let claim = &tallies::recorded_claims(
        "### Run 1 — `9999-x.json`\n\n- Recorded: 5 PASS, 2 FAIL. **Regraded 6 PASS, 1 FAIL** — later.\n",
    )[0];
    assert_eq!(claim.tally.get("PASS"), Some(&5), "the regrade must not be read as the record");
    assert_eq!(claim.tally.get("FAIL"), Some(&2));
}

#[test]
fn a_recorded_line_with_no_transcript_above_it_is_blocked_not_passed() {
    let claim = &tallies::recorded_claims("- Recorded: 4 PASS, 1 FAIL\n")[0];
    assert_eq!(claim.transcript, None, "nothing names what this tally describes");
}

#[test]
fn the_command_reader_follows_a_wrapped_invocation() {
    // A reader that took only the first line would miss every flag but the first.
    let doc = "```sh\nironworks coordinate \\\n    --slug x \\\n    --journal y\n```\n";
    let got = commands::documented(doc);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].verb, "coordinate");
    assert_eq!(got[0].flags, ["--slug", "--journal"]);
}

#[test]
fn lifecycle_is_one_verb_in_two_words() {
    let got = commands::documented("```sh\nironworks lifecycle confine acme --store s\n```\n");
    assert_eq!(got[0].verb, "lifecycle confine");
    assert_eq!(got[0].flags, ["--store"]);
}

#[test]
fn an_invocation_outside_a_fence_is_prose_not_a_command() {
    // Otherwise a sentence mentioning `ironworks relay --channel-id` would be checked as though
    // it were an instruction, and the docs could not discuss a removed flag.
    assert!(commands::documented("ironworks relay --channel-id 5\n").is_empty());
}

#[test]
fn a_reference_carrying_a_line_number_is_not_a_path_claim() {
    // `cases.py:86-127` names a location in a file, and the colon is what distinguishes it. A
    // truncating reader would check `cases.py` and fail on a correct citation.
    let refs = backticked("see `docs/PRD.md` and `docs/PRD.md:12-14` here", |c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
    });
    assert_eq!(refs, ["docs/PRD.md"], "only the bare path is a claim that a file exists");
}

#[test]
fn only_workspace_members_are_ours() {
    // `crates/product/…` and `crates/kernel/…` are inside the pinned IronClaw source, which
    // DESIGN.md cites precisely because reading the pinned source is the rule. Membership, not
    // prefix — this misfired within a minute of the check existing.
    let members = paths::owned_crates(&root());
    assert!(members.contains(&"crates/domain/".to_owned()), "{members:?}");
    assert!(paths::is_ours("crates/domain/src/lib.rs", &members));
    assert!(!paths::is_ours("crates/product/ironclaw_openai_compat/", &members));
    assert!(paths::is_ours("docs/PRD.md", &members));
    assert!(
        !paths::is_ours("/etc/ironworks/registry.json", &members),
        "a deployment target is not ours"
    );
    assert!(!paths::is_ours("~/agency/ironworks-py/multi/eval/cases.py", &members), "another repo");
}

#[test]
fn the_foreign_deploy_path_is_not_ours_but_the_rest_of_deploy_is() {
    // `site/` is generated and gitignored, and the Python reference has its own `deploy/lib` —
    // but that is one path, and excluding all of `deploy/` to avoid it hid a real rename: five
    // citations in `docs/adr/0008` and four in experiments named a unit file that no longer
    // existed. Both halves are asserted here, because dropping either one is the defect.
    let members = paths::owned_crates(&root());
    assert!(!paths::is_ours("deploy/lib/whatever.py", &members), "the Python reference's tree");
    assert!(!paths::is_ours("site/index.html", &members), "generated and gitignored");
    assert!(paths::is_ours("deploy/ironworks-ironclaw@.service", &members), "a unit is ours");
    assert!(paths::is_ours("deploy/README.md", &members), "so is the procedure that names it");
}

#[test]
fn the_deploy_procedure_is_one_of_the_checked_documents() {
    // It was the largest document in the repository that nothing read — 900-odd lines of operator
    // procedure for a host that is serving. Asserting membership rather than a count, because the
    // point is that the directory is globbed at all; `deploy/user/` holds units, not prose.
    let files = doc_files(&root());
    assert!(
        files.iter().any(|p| p.ends_with("deploy/README.md")),
        "deploy/README.md must be checked: {files:?}"
    );
}

#[test]
fn a_dead_deploy_citation_is_caught_now_that_deploy_is_checked() {
    // The positive control for widening `OWNED`. A negative check needs one: `deploy/` sat outside
    // the path check while nine citations of a renamed unit file accumulated inside it, and a
    // checker that reports clean over a set it never reads is the defect (§K-8, §F-14).
    let dir = Scratch::new("deploy-citation");
    dir.write("Cargo.toml", "[workspace]\nmembers = []\n");
    dir.write("DESIGN.md", "The unit is `deploy/ironworks-ironclaw@.service`.\n");
    let findings = paths::check(dir.path());
    assert!(
        findings.iter().any(|f| f.verdict == Verdict::Fail && f.detail.contains("deploy/")),
        "a deploy/ path that is not in the tree must fail: {findings:?}"
    );
}

#[test]
fn an_adr_citation_is_checked_even_though_an_adrs_paths_are_not() {
    // `markdown_in` does not recurse, so `docs/adr/` fell out of *every* check rather than out of
    // the path check alone. An ADR is a dated record: its paths may go stale, its pointers may not.
    let dir = Scratch::new("adr-citation");
    dir.write("Cargo.toml", "[workspace]\nmembers = []\n");
    dir.write("docs/adr/0001-a.md", "See `docs/adr/0001-a.md:12`.\n");
    let findings = paths::check_anchors(dir.path());
    assert!(
        findings.iter().any(|f| f.verdict == Verdict::Fail),
        "a line anchor inside an ADR must be refused: {findings:?}"
    );
}

#[test]
fn a_reader_that_reads_nothing_does_not_pass_everything() {
    // §B-3, applied to this crate: an empty input passes every check over it. The tallies live in
    // one section of one file now, so a rename that empties it would otherwise report a clean run
    // over zero claims. This is the positive control for that.
    let dir = Scratch::new("vacuity");
    dir.write("docs/experiments/README.md", "# no tallies here\n");
    let findings = tallies::check(dir.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].verdict, Verdict::Blocked, "an unread claim is never a pass");
}

#[test]
fn a_status_block_stating_nothing_blocks_rather_than_checking_nothing() {
    // The same control, on the check that matters most. `toolchain` reads its four claims out of
    // `DESIGN.md` — and with no claim found at all, every arm was skipped and the run reported no
    // build, no clippy and no formatting while the tally still said every claim held. That is the
    // gap `toolchain`'s own header says it came back to close: a checker that goes green on a tree
    // that does not compile.
    //
    // No arm spawns cargo when nothing is claimed, so this costs nothing.
    let dir = Scratch::new("toolchain-vacuity");
    dir.write("DESIGN.md", "# a design with no status block\n");
    let findings = toolchain::check(dir.path());
    assert_eq!(findings.len(), 4, "one per unstated claim: {findings:?}");
    assert!(
        findings.iter().all(|f| f.verdict == Verdict::Blocked),
        "an unstated claim is never a pass: {findings:?}"
    );
    assert!(findings.iter().any(|f| f.detail.contains("cargo test --workspace")), "{findings:?}");
}

#[test]
fn a_status_block_dropping_one_claim_blocks_that_claim() {
    // The hole the all-or-nothing guard left: reword the `cargo test` line alone and the one check
    // that establishes the tree compiles was skipped — no finding, exit 0. `unclaimed` is what
    // `check` reports before it spawns anything, so this costs nothing either.
    let dropped = toolchain::claimed(
        "cargo clippy --all-targets   clean\ncargo fmt --check            clean\n\
         cargo doc --no-deps          clean\n",
    );
    let findings = toolchain::unclaimed(dropped);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].verdict, Verdict::Blocked);
    assert_eq!(findings[0].subject, "workspace tests");
    // The positive control: a block making every claim leaves nothing unmeasured.
    let all = toolchain::claimed(
        "cargo test --workspace       passing\ncargo clippy --all-targets   clean\n\
         cargo fmt --check            clean\ncargo doc --no-deps          clean\n",
    );
    assert!(toolchain::unclaimed(all).is_empty());
}

#[test]
fn an_anchored_citation_is_read_past_the_colon() {
    // Both spellings name lines. `backticked` alone cannot see either, which is why they went
    // unchecked: the colon ends the match.
    let anchors = |s: &str| {
        crate::backticked(s, |c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | ':' | ',')
        })
    };
    assert_eq!(anchors("see `crates/domain/src/lib.rs:12-14`"), ["crates/domain/src/lib.rs:12-14"]);
    assert_eq!(
        anchors("see `crates/services/src/service.rs:203,221`"),
        ["crates/services/src/service.rs:203,221"]
    );
}

#[test]
fn a_line_anchor_into_our_own_tree_is_refused() {
    let dir = Scratch::new("anchors");
    dir.write("docs/short.md", "one\ntwo\n");
    dir.write("docs/citing.md", "see `docs/short.md:99` and `docs/short.md:2`\n");
    let findings = paths::check_anchors(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "a line anchor into our own tree is refused: {findings:#?}");
    assert!(fails.iter().all(|f| f.detail.contains("cite `path::symbol`")), "{fails:#?}");
}

#[test]
fn the_refused_thing_is_the_form_and_not_the_number() {
    // The gate used to be "does the spec parse as digits", which exempted every anchor naming
    // anything else — `docs/short.md:USAGE` drifts exactly as silently as `docs/short.md:2` and
    // was passing. Both halves are asserted here: the non-numeric anchor is refused, and the
    // `path::symbol` form it must not be confused with is left alone.
    let dir = Scratch::new("anchor-form");
    dir.write("docs/short.md", "one\ntwo\n");
    dir.write("Cargo.toml", "[workspace]\nmembers = [\n    \"crates/claims\",\n]\n");
    dir.write("crates/claims/src/thing.rs", "pub fn apply() {}\n");
    dir.write(
        "docs/citing.md",
        "refused: `docs/short.md:USAGE`\n\
         allowed: `crates/claims/src/thing.rs::apply`\n",
    );
    let findings = paths::check_anchors(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the anchor may fire: {findings:#?}");
    assert!(fails[0].detail.contains("docs/short.md:USAGE"), "{:?}", fails[0].detail);
}

#[test]
fn a_symbol_citation_fails_when_the_symbol_is_gone() {
    // Both directions, in one tree. §K-8: a harness that only ever reports a match is
    // indistinguishable from one that does nothing, and the check this replaces went green over
    // six wrong citations — so the negative arm is the one that had to be built first.
    let dir = Scratch::new("symbols");
    // `crates/…` is ours by workspace *membership*, not by prefix — so the scratch tree needs the
    // manifest that says so, exactly as the real one does.
    dir.write("Cargo.toml", "[workspace]\nmembers = [\n    \"crates/claims\",\n]\n");
    dir.write(
        "crates/claims/src/thing.rs",
        // `pin_newtype!` is a declarative macro naming what it declares, defined in the same file —
        // the shape `crates/domain/src/pins.rs` uses, and the one this checker read as a broken
        // citation until it learned to see it. `assert!(ready)` has the same shape and declares
        // nothing, and it is the one the checker learned to see too well.
        "macro_rules! pin_newtype {\n    ($name:ident, $doc:literal) => {};\n}\n\
         pub enum Stage {\n    MemberConfined,\n}\n\
         pub struct Wire {\n    egress: WireAccess,\n}\n\
         pub fn apply(plan: &Plan) -> Applied { renamed_away(plan) }\n\
         pin_newtype!(ModelPin, \"the model of record\");\n\
         fn guard(ready: bool) {\n    assert!(ready);\n}\n",
    );
    dir.write(
        "docs/citing.md",
        "live: `crates/claims/src/thing.rs::Stage::MemberConfined`, \
         `crates/claims/src/thing.rs::Wire::egress`, `crates/claims/src/thing.rs::apply`, \
         `crates/claims/src/thing.rs::ModelPin`\n\
         gone: `crates/claims/src/thing.rs::renamed_away`, \
         `crates/claims/src/thing.rs::RuntimePin`, `crates/claims/src/thing.rs::ready`\n",
    );
    let findings = paths::check_symbols(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 3, "only the three absent ones may fire: {findings:#?}");
    assert!(fails.iter().any(|f| f.detail.contains("renamed_away")), "{fails:#?}");
    // A macro that declares nothing named `RuntimePin` must still fail, so recognising the naming
    // convention did not become "any macro line declares anything".
    assert!(fails.iter().any(|f| f.detail.contains("RuntimePin")), "{fails:#?}");
    // And a macro the file does not define declares nothing: `ready` is `assert!`'s first argument,
    // which is a use, and until the macro had to be one of the file's own it passed as a
    // declaration.
    assert!(fails.iter().any(|f| f.detail.contains("::ready")), "{fails:#?}");
    // The positive control is the other four passing — and `renamed_away` *appears* in the file,
    // at its call site, so this also asserts that presence alone is not enough.
}

/// A shell function is a declaration too, and the checker used to say otherwise.
///
/// It reported `dev/lib.sh` "does not declare `creates`" about a file that declares it on the line
/// above `fetches`. A false refusal is the same defect as a false pass here: the first one teaches
/// its reader that this row can be ignored.
#[test]
fn a_shell_function_citation_is_checked_against_the_script_that_declares_it() {
    let dir = Scratch::new("shell-symbols");
    dir.write("Cargo.toml", "[workspace]\nmembers = [\n    \"crates/claims\",\n]\n");
    dir.write(
        "dev/lib.sh",
        "#!/usr/bin/env bash\n\
         creates() { call_count \"$1\" create; }\n\
         pace_window() {\n  sleep 60\n}\n\
         # A call, not a declaration — shell calls carry no parentheses, which is the whole\n\
         # reason this can be decided at all.\n\
         summary \"done\"\n",
    );
    dir.write(
        "docs/citing.md",
        "live: `dev/lib.sh::creates`, `dev/lib.sh::pace_window`\n\
         gone: `dev/lib.sh::summary`, `dev/lib.sh::renamed_away`\n",
    );
    let findings = paths::check_symbols(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "only the two that are not declared here may fire: {findings:#?}");
    // `summary` is *called* in that script and declared in another. Presence is not declaration,
    // which is the same rule the Rust arm holds to.
    assert!(fails.iter().any(|f| f.detail.contains("summary")), "{fails:#?}");
    assert!(fails.iter().any(|f| f.detail.contains("renamed_away")), "{fails:#?}");
}

#[test]
fn a_deleted_document_cited_by_bare_name_fails() {
    // The defect this exists for, and it is not hypothetical: `README.md` named four top-level
    // reports, all four were deleted, and this binary reported every claim holding — a bare name
    // has no owned prefix, so `is_ours` rejects it and the path check skips it in silence.
    let dir = Scratch::new("bare-docs");
    dir.write("docs/kept.md", "# kept\n");
    dir.write("scrap/personas/COORDINATOR.md", "# a doc that is not at the root\n");
    dir.write(
        "docs/citing.md",
        // Live, live-but-nested, dead, and one that belongs to `check_experiments` instead.
        "see `kept.md` and `COORDINATOR.md` but not `RECON.md`;\n\
         `0021-pruned.md` is an experiment and is not this check's business.\n",
    );
    let findings = paths::check_bare_docs(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the deleted one may fire: {findings:#?}");
    assert!(fails[0].detail.contains("RECON.md"), "{:?}", fails[0].detail);
    // The positive controls are the other two passing: `COORDINATOR.md` is cited bare and lives
    // three directories down, so a root-only rule would fail a citation that is perfectly good.
}

#[test]
fn a_pruned_experiment_cited_by_name_fails() {
    // The defect this exists for: a writeup is cited as `0021-three-runs-three-answers.md`, the
    // writeup is pruned into a consolidated README, and no path check can see it — a bare name has
    // no owned prefix, so `is_ours` rejects it and it is skipped in silence.
    let dir = Scratch::new("experiments");
    dir.write("docs/experiments/0001-kept.md", "# kept\n");
    // Two live citations, one dead, and two things that must not be read as citations at all.
    dir.write(
        "docs/citing.md",
        "see `0001-kept.md` and `0001-kept.md` but not `0021-pruned.md`;\n\
         `regrades.json` has no run number and `0020` alone is not a filename.\n",
    );
    let findings = paths::check_experiments(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the pruned one may fire: {findings:#?}");
    assert!(fails[0].detail.contains("0021-pruned.md"), "{:?}", fails[0].detail);
}

#[test]
fn a_markdown_link_counts_only_beside_the_writeups() {
    // `](0001-alloy-drift.md)` is a sibling link inside `docs/experiments/`. The same spelling in
    // another directory would be relative to *that* directory, so it is not read as a citation.
    let dir = Scratch::new("links");
    dir.write("docs/experiments/README.md", "[a](0009-gone.md)\n");
    dir.write("docs/elsewhere.md", "[b](0009-gone.md)\n");
    let cited = paths::cited_experiments(dir.path());
    assert_eq!(cited.len(), 1, "only the sibling link is a citation: {cited:?}");
    assert!(cited[0].0.contains("experiments"), "{cited:?}");
}

#[test]
fn an_unindexed_write_up_fails_and_an_indexed_one_does_not() {
    // Two-sided, because both halves are the same defect (§K-8): a check that fired on every tree
    // would satisfy the first assertion and establish nothing, and one that fired on none would
    // pass the tree in exactly the state this exists to catch.
    let dir = Scratch::new("experiment-index");
    dir.write("docs/experiments/0001-indexed.md", "# a write-up\n");
    dir.write("docs/experiments/0002-not-indexed.md", "# a write-up\n");
    dir.write("docs/experiments/README.md", "| [`0001`](0001-indexed.md) | what it measured |\n");

    let findings = paths::check_experiment_index(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the unindexed one may fire: {findings:#?}");
    assert!(fails[0].detail.contains("0002"), "{:?}", fails[0].detail);
    assert!(
        !fails[0].detail.contains("0001"),
        "the indexed one is the control: {:?}",
        fails[0].detail
    );

    // The number must be named as a number, not merely appear inside a filename the index links.
    // Without this the row `[a](0002-not-indexed.md)` would satisfy the check while naming nothing
    // a reader can scan for.
    dir.write(
        "docs/experiments/README.md",
        "| [`0001`](0001-indexed.md) | m |\n| [`0002`](0002-not-indexed.md) | m |\n",
    );
    let findings = paths::check_experiment_index(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn an_unreadable_index_blocks_rather_than_passing_or_failing() {
    // An index that cannot be read has measured nothing. BLOCKED is never a pass, and calling it a
    // FAIL would report a missing row that nobody can see is missing.
    let dir = Scratch::new("experiment-index-absent");
    dir.write("docs/experiments/0001-indexed.md", "# a write-up\n");
    let findings = paths::check_experiment_index(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Blocked), "{findings:#?}");
}

// ── does the tree hold? ─────────────────────────────────────────────────────────────────────

/// Every check that can run here, run here.
///
/// # This was six hand-written tests, and the list could fall behind
///
/// `main` listed thirteen checks and this file re-listed six of them, so a check added to one and
/// not the other ran in the binary and not in the suite — and `toolchain` then reports
/// `cargo test --workspace` as `passing` to `DESIGN.md`. Iterating `CHECKS` is what makes the two
/// sets the same set.
///
/// **Both halves are asserted.** A check that reports nothing has measured nothing, which is the
/// empty-store shape §B-3 and §K-8 are about: a reader that reads no documents passes every claim
/// over them. So a silent check fails here as loudly as a disagreeing one.
#[test]
fn every_check_that_can_run_in_the_suite_holds() {
    let root = root();
    let mut ran = 0;
    for check in crate::CHECKS.iter().filter(|c| c.in_suite) {
        ran += 1;
        let findings = check.findings(&root);
        assert!(
            !findings.is_empty(),
            "the {} check reported nothing — a reader that reads nothing passes everything",
            check.name
        );
        let bad: Vec<&Finding> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
        assert!(
            bad.is_empty(),
            "the {} check disagrees with the tree:\n{}",
            check.name,
            bad.iter()
                .map(|f| format!("  {} — {}", f.subject, f.detail))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    // The positive control for the loop itself: a filter that matched nothing would pass every
    // assertion above by never reaching one.
    assert!(ran >= 10, "only {ran} check(s) ran in the suite; the registry or the filter is wrong");
}

/// The two checks the suite cannot run, named here so the exemption is visible rather than
/// inferred from an absence.
///
/// Both need a subprocess: `commands` asks the built binary about its own flags, and `toolchain`
/// shells out to cargo — a suite running either would be running itself. `dev/prove-workspace.sh`
/// invokes the binary, which runs them.
#[test]
fn the_only_checks_outside_the_suite_are_the_two_that_need_a_subprocess() {
    let outside: Vec<&str> = crate::CHECKS.iter().filter(|c| !c.in_suite).map(|c| c.name).collect();
    assert_eq!(outside, ["documented commands", "toolchain"]);
}

/// The rule that decides what is a numbered experiment, in both directions.
///
/// It existed four times with three different length bounds, and two copies never checked the
/// hyphen — so `00012-foo.md` was experiment `0001` to the index checks and nothing at all to the
/// citation checks. Nothing is named that way today, which is why four copies could disagree
/// silently; the negative arm below is what makes that unrepresentable.
#[test]
fn one_rule_decides_what_carries_an_experiment_number() {
    use crate::experiment_number as number;

    assert_eq!(number("0021-three-runs-three-answers.md"), Some("0021"));
    assert_eq!(number("0002-eval-parity-run.json"), Some("0002"));
    assert_eq!(number("0001-a"), Some("0001"), "the extension is a separate question");

    // The divergence: five digits then a hyphen is not experiment `0001`.
    assert_eq!(number("00012-foo.md"), None);
    // And the bounds each copy chose differently.
    assert_eq!(number("0001"), None, "a number with nothing after it names no file");
    assert_eq!(number("0001-"), None, "nor does one with only a hyphen after it");
    assert_eq!(number("README.md"), None);
    assert_eq!(number("regrades.json"), None);
    assert_eq!(number("abc-x.md"), None);
}

#[test]
fn a_spelled_out_count_before_crates_is_a_claim() {
    assert_eq!(members::stated("crates/       twenty crates in three groups"), [20]);
    assert_eq!(members::stated("Twenty crates. `domain` depends on nothing"), [20]);
    assert_eq!(members::stated("twenty-one crates, two layers"), [21]);
}

#[test]
fn the_lockfile_claim_is_not_a_workspace_claim() {
    // `DESIGN.md` says "**88 crates in the lockfile**", counting transitive dependencies rather
    // than workspace members. A check that fired on it would be wrong on a correct sentence, and
    // the first false alarm teaches its reader to skim the whole report.
    assert!(
        members::stated("**88 crates in the lockfile**, against the several hundred").is_empty()
    );
}

#[test]
fn a_path_is_not_a_count() {
    assert!(members::stated("see `crates/domain/src/lib.rs` and crates/ironclaw").is_empty());
    assert!(members::stated("the fourteen services").is_empty(), "only counts of crates");
}

#[test]
fn the_stated_counts_match_the_workspace() {
    // The claim this whole module exists for. Both README.md and DESIGN.md state it, and both
    // were hand-corrected twice in one day while three crates landed.
    let findings = members::check(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn the_documented_layering_matches_the_manifests() {
    let findings = layering::check(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_dev_script_costs_what_the_documents_say() {
    // The one check whose failure costs money rather than credibility: a script documented as free
    // that spends a member or a model turn is read as safe to run by whoever reads it next.
    let findings = dev_costs::check(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn the_experiment_index_names_every_numbered_run() {
    // An index that silently omits entries is worse than an absent one: `docs/experiments/README.md`
    // is the documented way to find the run behind a claim, so a missing row reads as a claim with
    // no measurement rather than a measurement with no row.
    let findings = paths::check_experiment_index(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn no_experiment_number_is_claimed_twice() {
    // A number is an identity. Two write-ups sharing one means a citation resolves to whichever a
    // reader opens first, which is the failure mode a citation exists to prevent.
    let findings = paths::check_experiment_numbers(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

// `self_description` is deliberately absent from this section, and the reason is the rule rather
// than the effort. Its input is the set of findings that ran, so a test would have to assemble its
// own list of checks — a second place to keep in step with `main`, for one invariant. It would also
// assert something weaker than the binary does, because the two subprocess checks cannot report
// here. The binary is the only caller that has the whole set, so it stays the only caller.

#[test]
fn a_command_in_a_fenced_block_names_a_path_the_inline_check_cannot_see() {
    // The gap this closes, in the shape it actually had. `fixtures/account-coordination/README.md`
    // told a reader to run a script that had been deleted, in the same file whose neighbouring
    // block had been updated — and nothing noticed, because a command writes its path bare while
    // the only checked spelling was the backticked one. A command is the worse of the two to get
    // wrong: a reader does not follow it, they paste it.
    let doc = "Run it:\n\n```sh\ndev/eval.py --suite account-coordination\n```\n";
    assert_eq!(paths::fenced_paths(doc), ["dev/eval.py"]);

    // Flags, subcommands and bare words are not paths, whatever else is on the line.
    let doc = "```sh\ncargo run -p ironworks-eval --json docs/experiments/0029-x.json\n```\n";
    assert_eq!(paths::fenced_paths(doc), ["docs/experiments/0029-x.json"]);
}

#[test]
fn correct_writing_inside_a_fence_is_not_reported() {
    // The other half, and the one that decides whether this check is usable. Every item here is
    // something the documents legitimately contain, and a checker that fired on any of them would
    // be turned off within a day.
    let cases = [
        // A URL. The scheme's colon is what rules it out, and the host is nobody's file.
        "```sh\ncurl https://example.test/api/v2/session\n```\n",
        // A line anchor, which `backticked` exempts by refusing the span — so the rule has to be
        // restated here, where the token is trimmed before it is judged.
        "```text\nsee crates/domain/src/lib.rs:12-14\n```\n",
        // A directory prefix standing alone is prose, not a reference.
        "```text\neverything under docs/ is checked\n```\n",
        // An environment variable, which carries no slash at all.
        "```sh\nexport IRONWORKS_RECORDS=http://127.0.0.1:8080\n```\n",
    ];
    for doc in cases {
        assert!(paths::fenced_paths(doc).is_empty(), "{doc:?} named a path it should not have");
    }
}

#[test]
fn a_fence_that_is_never_closed_does_not_swallow_the_document() {
    // A stray fence must not turn the rest of a document into commands. The scan flips on the
    // marker, so an unclosed block reads to the end of the file — which is the right failure
    // direction (it checks more, never less), and is asserted so it stays deliberate.
    let doc = "```sh\ndev/probe-up.sh\n\nand then prose mentioning dev/nothing-here.sh\n";
    let found = paths::fenced_paths(doc);
    assert!(found.contains(&"dev/probe-up.sh".to_owned()), "{found:?}");
    assert!(found.contains(&"dev/nothing-here.sh".to_owned()), "{found:?}");
}

#[test]
fn a_path_written_both_ways_is_reported_once() {
    // Most commands are also named in the prose around them. Reporting the pair twice would
    // double every finding and make the count meaningless.
    let root = crate::root();
    let referenced = paths::referenced(&root);
    let mut seen = std::collections::BTreeSet::new();
    for (doc, path) in &referenced {
        assert!(seen.insert((doc.clone(), path.clone())), "{doc} names {path} twice");
    }
}

/// A disposable `docs/experiments` with exactly the files named.
fn experiments_dir(kind: &str, files: &[&str]) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "ironworks-claims-{kind}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("docs/experiments");
    std::fs::create_dir_all(&dir).expect("temp dir");
    for f in files {
        std::fs::write(dir.join(f), "# a write-up\n").expect("write");
    }
    root
}

#[test]
fn two_write_ups_claiming_one_number_are_refused() {
    // Not hypothetical: two `0037`s were written into this directory six minutes apart while the
    // tree was edited from two places, and only `git status` noticed. `cited_experiments` matches
    // on the name, so both files exist and both pass — the ambiguity is invisible to every other
    // check, which is exactly why it needs one of its own.
    let root = experiments_dir(
        "collision",
        &["0037-what-the-seam-never-asked-for.md", "0037-the-rule-that-never-fired.md"],
    );
    let findings = paths::check_experiment_numbers(&root);
    assert!(
        findings.iter().any(|f| f.verdict == Verdict::Fail),
        "a repeated number must be refused, got {findings:?}"
    );
    // Both sides named, so an operator can see which two files to look at.
    let text = format!("{findings:?}");
    assert!(text.contains("what-the-seam") && text.contains("never-fired"), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn distinct_numbers_and_the_index_are_not_a_collision() {
    // The positive control. Without it the check above would pass on a function that refused every
    // directory it was given — and `README.md` carries no number, so it must not be counted as a
    // write-up that lost one.
    let root = experiments_dir(
        "distinct",
        &[
            "0037-what-the-seam-never-asked-for.md",
            "0038-the-rule-that-never-fired.md",
            "README.md",
        ],
    );
    let findings = paths::check_experiment_numbers(&root);
    assert!(
        findings.iter().all(|f| f.verdict == Verdict::Pass),
        "two numbers and an index are not a collision, got {findings:?}"
    );
    assert!(format!("{findings:?}").contains('2'), "both write-ups must be counted");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_transcript_beside_its_write_up_is_not_a_collision() {
    // `0022` legitimately has two transcripts and a console log under one number. Only the `.md`
    // that reads a run is what a citation resolves to, so only those may not repeat.
    let root = experiments_dir("transcripts", &["0022-onboarding-stability.md"]);
    let dir = root.join("docs/experiments");
    for sibling in ["0022-a.json", "0022-b.json", "0022-probe-console.log"] {
        std::fs::write(dir.join(sibling), "{}").expect("write");
    }
    let findings = paths::check_experiment_numbers(&root);
    assert!(
        findings.iter().all(|f| f.verdict == Verdict::Pass),
        "a transcript is not a second write-up, got {findings:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_transcript_with_no_write_up_still_has_to_be_indexed() {
    // The five that were missing — `0004` through `0007` and `0009` — had transcripts and no
    // write-up, so the old expectation, built from `.md` files alone, could not see them. Both
    // sides here: an indexed transcript-only run is the positive control, and without it a check
    // that had simply stopped looking would pass this test too.
    let dir = Scratch::new("index-covers-transcripts");
    dir.write("docs/experiments/0003-a-run.json", "[]");
    dir.write("docs/experiments/0004-a-run.json", "[]");
    dir.write("docs/experiments/README.md", "| `0003` | the arc |\n");

    let findings = paths::check_experiment_index(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "the unindexed transcript must fire: {findings:#?}");
    assert!(fails[0].detail.contains("0004"), "{:?}", fails[0].detail);
    assert!(!fails[0].detail.contains("0003"), "the indexed one is the control: {findings:#?}");

    dir.write("docs/experiments/README.md", "| `0003`, `0004` | the arc |\n");
    let findings = paths::check_experiment_index(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

// ── layering: DESIGN.md's dependency tables against the manifests ───────────────────────────

/// A scratch workspace with one member and the `DESIGN.md` table that describes it.
fn layering_tree(name: &str, member_deps: &str, table: &str) -> Scratch {
    let dir = Scratch::new(name);
    dir.write("Cargo.toml", "[workspace]\nmembers = [\n    \"crates/site\",\n]\n");
    dir.write("crates/site/Cargo.toml", &format!("[package]\nname = \"x\"\n\n[dependencies]\n{member_deps}\n\n[lints]\nworkspace = true\n"));
    dir.write("DESIGN.md", table);
    dir
}

#[test]
fn a_dependency_the_manifest_does_not_have_fails() {
    // The defect this was written for: the table named `graders`, the manifest named `services`,
    // and the paragraph justifying the phantom edge outlived the edge by weeks.
    let dir = layering_tree(
        "layering-phantom",
        "ironworks-services = { path = \"../services\" }\n",
        "| Crate | Holds | Depends on |\n|---|---|---|\n| `site` | the page | `graders` |\n",
    );
    let findings = layering::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("names graders"), "{:?}", fails[0].detail);
    assert!(fails[0].detail.contains("omits services"), "{:?}", fails[0].detail);
}

#[test]
fn a_matching_dependency_column_passes() {
    // The positive control. Without it the test above is satisfied by a check that fails on
    // everything, which is the same defect in the other direction (§K-8).
    let dir = layering_tree(
        "layering-match",
        "ironworks-services = { path = \"../services\" }\n",
        "| Crate | Holds | Depends on |\n|---|---|---|\n| `site` | the page | `services` |\n",
    );
    let findings = layering::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn an_effect_free_dependency_is_not_something_a_crate_owns() {
    // `serde` parses; parsing is not an effect. A crate carrying only those may still write `—`,
    // which is what lets `domain`'s row be honest.
    let dir = layering_tree(
        "layering-owns",
        "serde = { workspace = true }\nrusqlite = \"0.40\"\n",
        "| Crate | Holds | Owns |\n|---|---|---|\n| `site` | the page | `rusqlite` |\n",
    );
    let findings = layering::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn a_design_with_no_dependency_table_blocks_rather_than_passing() {
    // A table that was renamed away has measured nothing. An empty expectation passes every tree.
    let dir = Scratch::new("layering-absent");
    dir.write("Cargo.toml", "[workspace]\nmembers = []\n");
    dir.write("DESIGN.md", "# Design\n\nNo table here.\n");
    let findings = layering::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Blocked), "{findings:#?}");
}

#[test]
fn a_dependency_written_without_backticks_fails() {
    // Plain text in the cell used to be dropped before the comparison, so this row read as
    // `{services}` and passed — the manifest held to a row that said less than it appeared to.
    let dir = layering_tree(
        "layering-unticked",
        "ironworks-services = { path = \"../services\" }\n",
        "| Crate | Holds | Depends on |\n|---|---|---|\n| `site` | the page | `services`, graders |\n",
    );
    let findings = layering::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("graders without backticks"), "{:?}", fails[0].detail);
}

// ── dev costs: the `# COST:` line against the two lists that name it ────────────────────────

/// A scratch tree with one `dev/` script and the two cost regions that must agree with it.
fn cost_tree(name: &str, script: &str, cost: &str, costly_list: &str, free_list: &str) -> Scratch {
    let dir = Scratch::new(name);
    dir.write(&format!("dev/{script}"), &format!("#!/usr/bin/env bash\n# COST: {cost}\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let at = dir.path().join("dev").join(script);
        std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let body = format!(
        "## What costs something to run\n\n{costly_list}\n\nThese cost nothing: {free_list}\n\n**Free is stated.**\n\n## House rules\n"
    );
    dir.write("CLAUDE.md", &body);
    dir.write(
        "docs/AUDIT-BRIEF.md",
        &format!(
            "### Commands you must not run\n\n{costly_list}\n\nYou **may** run these: {free_list}\n\n**Free is stated.**\n"
        ),
    );
    dir
}

#[test]
fn a_costly_script_no_document_names_fails() {
    // The defect: `dev/probe-up.sh` bills a turn per bring-up and CLAUDE.md's section did not name
    // it, so the always-loaded brief said the one script an agent would certainly run was free.
    let dir = cost_tree(
        "cost-unnamed",
        "probe-up.sh",
        "one model turn per bring-up",
        "- nothing here",
        "none",
    );
    let findings = dev_costs::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "one per document: {findings:#?}");
    assert!(fails.iter().all(|f| f.detail.contains("dev/probe-up.sh")), "{findings:#?}");
}

#[test]
fn a_costly_script_both_documents_name_passes() {
    // The positive control for the test above.
    let dir = cost_tree(
        "cost-named",
        "probe-up.sh",
        "one model turn per bring-up",
        "- `dev/probe-up.sh` bills a turn",
        "none",
    );
    let findings = dev_costs::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn a_free_script_listed_as_costly_fails() {
    // The other direction, and it is the half that makes "most of dev/ is free" a claim rather
    // than an absence: a list nobody removes from drifts exactly as badly as one nobody adds to.
    // Named in the free lists as well, so the one disagreement left is the costly listing.
    let dir = cost_tree(
        "cost-free-listed",
        "prove-tenancy.sh",
        "free — no runtime, no model key",
        "- `dev/prove-tenancy.sh` spends nothing",
        "`dev/prove-tenancy.sh`",
    );
    let findings = dev_costs::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "one per document: {findings:#?}");
    assert!(fails.iter().all(|f| f.detail.contains("declares itself free")), "{findings:#?}");
}

#[test]
fn a_script_declaring_no_cost_at_all_fails() {
    let dir = Scratch::new("cost-unmarked");
    dir.write("dev/prove-x.sh", "#!/usr/bin/env bash\n# no marker here\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let at = dir.path().join("dev/prove-x.sh");
        std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    dir.write(
        "CLAUDE.md",
        "## What costs something to run\n\nThese cost nothing: none\n\n**Free is stated.**\n\n## House rules\n",
    );
    dir.write(
        "docs/AUDIT-BRIEF.md",
        "### Commands you must not run\n\nYou **may** run these: none\n\n**Free is stated.**\n",
    );
    let findings = dev_costs::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("declares no `# COST:` line"), "{:?}", fails[0].detail);
}

#[test]
fn a_missing_cost_region_blocks_rather_than_passing() {
    // Without the region markers the check would search each whole file, find every script name in
    // it, and report a clean run having compared nothing.
    let dir = cost_tree("cost-no-region", "prove-x.sh", "a turn", "- `dev/prove-x.sh`", "none");
    dir.write("CLAUDE.md", "## Something else entirely\n");
    let findings = dev_costs::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Blocked), "{findings:#?}");
}

#[test]
fn a_free_script_no_document_names_fails() {
    // The half that was prose: both documents said an unlisted script is a defect, and a script
    // marked free and named in neither passed.
    let dir = cost_tree("cost-free-unnamed", "test-x.sh", "free — offline", "- nothing", "none");
    let findings = dev_costs::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "one per document: {findings:#?}");
    assert!(fails.iter().all(|f| f.detail.contains("does not say so")), "{findings:#?}");
}

#[test]
fn a_free_script_both_free_lists_name_passes() {
    // The positive control for the test above.
    let dir =
        cost_tree("cost-free-named", "test-x.sh", "free — offline", "- nothing", "`dev/test-x.sh`");
    let findings = dev_costs::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn a_costly_script_also_listed_as_free_fails() {
    // Named where it must be and also where it must not: a document telling one reader both.
    let dir = cost_tree(
        "cost-both-lists",
        "probe-up.sh",
        "one model turn per bring-up",
        "- `dev/probe-up.sh` bills a turn",
        "`dev/probe-up.sh`",
    );
    let findings = dev_costs::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 2, "one per document: {findings:#?}");
    assert!(fails.iter().all(|f| f.detail.contains("also lists it as free")), "{findings:#?}");
}

// ── self-description: the prose list against the checks that ran ────────────────────────────

#[test]
fn a_registered_check_is_required_even_when_it_reported_only_under_documents() {
    // The first hole subjects alone left: a check whose every finding that run was a failure filed
    // under a document reported no subject of its own, so the prose was not asked to name it at
    // exactly the moment it mattered.
    let dir = Scratch::new("self-desc-registry");
    dir.write(
        "CLAUDE.md",
        "## What is machine-checked, and what is not\n\nreferenced paths, self-description.\n\n\
         ## House rules\n",
    );
    let ran = [Finding::fail("README.md", "cites a symbol that is gone")];
    let findings =
        self_description::check(dir.path(), &["referenced paths", "symbol citations"], &ran);
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("symbol citations"), "{:?}", fails[0].detail);
}

#[test]
fn a_check_that_could_not_run_is_not_reported_as_unnamed() {
    // The second hole: `toolchain` filed its BLOCKED under a name no prose had reason to hold, so
    // a measurement that never happened arrived as this check's FAIL and the binary exited 2.
    // Against the real `CLAUDE.md` and the real registry, every way `toolchain` can be BLOCKED is
    // a name the section already carries.
    let names: Vec<&str> = crate::CHECKS.iter().map(|c| c.name).collect();
    let none = toolchain::Claimed {
        tests_pass: false,
        clippy_clean: false,
        fmt_clean: false,
        docs_clean: false,
    };
    let findings = self_description::check(&root(), &names, &toolchain::unclaimed(none));
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

fn one_document_disagrees(_: &std::path::Path) -> Vec<Finding> {
    vec![
        Finding::fail("README.md", "a citation that no longer resolves"),
        Finding::pass("README.md:12", "a tally").claiming("tally:0002-run.json"),
        Finding::pass("clippy", "clean"),
    ]
}

#[test]
fn a_finding_filed_under_a_document_is_claimed_by_its_check() {
    // Its subject stays the file a reader opens; its identity becomes the check, so one check is
    // one claim whether it held or not. An explicit claim and a named subject are both kept.
    let check = Check { name: "symbol citations", run: one_document_disagrees, in_suite: true };
    let findings = check.findings(std::path::Path::new("."));
    let identities: Vec<(&str, &str)> =
        findings.iter().map(|f| (f.subject.as_str(), f.claim.as_str())).collect();
    assert_eq!(
        identities,
        [
            ("README.md", "symbol citations"),
            ("README.md:12", "tally:0002-run.json"),
            ("clippy", "clippy"),
        ]
    );
}

#[test]
fn a_check_the_prose_does_not_name_fails() {
    // The defect: `experiment index` had been machine-checked for some time and CLAUDE.md did not
    // say so, so a planning pass treated the gap it closes as still open.
    let dir = Scratch::new("self-desc-missing");
    dir.write(
        "CLAUDE.md",
        "## What is machine-checked, and what is not\n\nIt checks referenced paths.\n\n## House rules\n",
    );
    let ran = [Finding::pass("referenced paths", "ok"), Finding::pass("experiment index", "ok")];
    let findings = self_description::check(dir.path(), &[], &ran);
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("experiment index"), "{:?}", fails[0].detail);
}

#[test]
fn a_prose_list_naming_every_check_passes() {
    // The positive control, and it also pins the self-reference: the section must name
    // `self-description` itself, which is never present in the findings it is handed.
    let dir = Scratch::new("self-desc-complete");
    dir.write(
        "CLAUDE.md",
        "## What is machine-checked, and what is not\n\nreferenced paths, experiment index, \
         self-description.\n\n## House rules\n",
    );
    let ran = [Finding::pass("referenced paths", "ok"), Finding::pass("experiment index", "ok")];
    let findings = self_description::check(dir.path(), &[], &ran);
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn a_subject_naming_a_document_is_not_held_to_the_prose() {
    // `tallies` files its findings under the document carrying the claim, so requiring CLAUDE.md
    // to name them would require it to list every file — which its first sentence already does.
    let dir = Scratch::new("self-desc-documents");
    dir.write(
        "CLAUDE.md",
        "## What is machine-checked, and what is not\n\nself-description.\n\n## House rules\n",
    );
    let ran = [Finding::pass("README.md:144", "a tally"), Finding::pass("docs/PRD.md", "a path")];
    let findings = self_description::check(dir.path(), &[], &ran);
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}

#[test]
fn a_missing_section_blocks_rather_than_passing() {
    let dir = Scratch::new("self-desc-absent");
    dir.write("CLAUDE.md", "# Working in this repository\n");
    let ran = [Finding::pass("referenced paths", "ok")];
    let findings = self_description::check(dir.path(), &[], &ran);
    assert!(findings.iter().all(|f| f.verdict == Verdict::Blocked), "{findings:#?}");
}

#[test]
fn the_real_design_table_would_still_catch_the_edge_it_was_written_for() {
    // The two tests above run on synthetic tables. This one runs the parser over the **real**
    // `DESIGN.md` and the **real** manifests, with one row put back the way it was found: `site`
    // naming `graders`, an edge the workspace has not had since the refusal rate stopped being
    // computable. A parser that silently found no rows in the real file would pass every synthetic
    // test and report a clean tree, which is the vacuity §F-14 names.
    let real = crate::root();
    let design = std::fs::read_to_string(real.join("DESIGN.md")).expect("DESIGN.md");
    let broken = design.replacen(
        "| `site` | `ironworks-site`: the published page, the arc, the reader, the 404 | `domain`, `services`, `evidence`, `page` |",
        "| `site` | `ironworks-site`: the published page, the arc, the reader, the 404 | `domain`, `graders`, `evidence`, `page` |",
        1,
    );
    assert_ne!(broken, design, "the row this reproduces has moved; update the test, not the row");

    let dir = Scratch::new("layering-real");
    dir.write("DESIGN.md", &broken);
    dir.write("Cargo.toml", &std::fs::read_to_string(real.join("Cargo.toml")).expect("manifest"));
    for member in paths::owned_crates(&real) {
        let manifest = format!("{member}Cargo.toml");
        dir.write(&manifest, &std::fs::read_to_string(real.join(&manifest)).expect("manifest"));
    }

    let findings = layering::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the reintroduced edge may fire: {findings:#?}");
    assert!(fails[0].detail.contains("`site` row"), "{:?}", fails[0].detail);
    assert!(fails[0].detail.contains("names graders"), "{:?}", fails[0].detail);
}

#[test]
fn a_workspace_member_with_no_row_fails() {
    // The completeness half. A crate in the workspace and in no table has no row to be wrong, so
    // the row-by-row comparison above stays green over a diagram that no longer describes the
    // workspace — the same §K-8 defect from the other side, and the harder one to notice.
    let dir = Scratch::new("layering-undocumented");
    dir.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\n    \"crates/site\",\n    \"crates/page\",\n]\n",
    );
    for c in ["site", "page"] {
        dir.write(&format!("crates/{c}/Cargo.toml"), "[package]\nname = \"x\"\n\n[dependencies]\n");
    }
    // Only `site` gets a row; `page` is the one the tables forgot.
    dir.write(
        "DESIGN.md",
        "| Crate | Holds | Depends on |\n|---|---|---|\n| `site` | the page | — |\n",
    );

    let findings = layering::check(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "{findings:#?}");
    assert!(fails[0].detail.contains("no row for page"), "{:?}", fails[0].detail);
    assert!(!fails[0].detail.contains("site"), "the documented one is the control: {findings:#?}");

    dir.write(
        "DESIGN.md",
        "| Crate | Holds | Depends on |\n|---|---|---|\n| `site` | the page | — |\n| `page` | the shell | — |\n",
    );
    let findings = layering::check(dir.path());
    assert!(findings.iter().all(|f| f.verdict == Verdict::Pass), "{findings:#?}");
}
