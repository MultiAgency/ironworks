//! Two kinds of test, and both are needed.
//!
//! **Can it fail?** Synthetic inputs proving each checker actually rejects something. A checker
//! that only ever passes is indistinguishable from one that does nothing, and this file's whole
//! premise is that a check nobody can see failing gets skimmed.
//!
//! **Does the tree hold?** The two pure checks run over the real repository, so `cargo test`
//! enforces them. The other two need a subprocess and live in the binary — see `Cargo.toml`.

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
        !paths::is_ours("/etc/rustworks/registry.json", &members),
        "a deployment target is not ours"
    );
    assert!(!paths::is_ours("~/agency/ironworks/multi/eval/cases.py", &members), "another repo");
}

#[test]
fn deploy_and_site_are_deliberately_not_ours() {
    // The Python reference has its own `deploy/lib`, and `site/` is generated and gitignored.
    let members = paths::owned_crates(&root());
    assert!(!paths::is_ours("deploy/lib/whatever.py", &members));
    assert!(!paths::is_ours("site/index.html", &members));
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
        "pub enum Stage {\n    MemberConfined,\n}\n\
         pub struct Wire {\n    egress: WireAccess,\n}\n\
         pub fn apply(plan: &Plan) -> Applied { renamed_away(plan) }\n",
    );
    dir.write(
        "docs/citing.md",
        "live: `crates/claims/src/thing.rs::Stage::MemberConfined`, \
         `crates/claims/src/thing.rs::Wire::egress`, `crates/claims/src/thing.rs::apply`\n\
         gone: `crates/claims/src/thing.rs::renamed_away`\n",
    );
    let findings = paths::check_symbols(dir.path());
    let fails: Vec<_> = findings.iter().filter(|f| f.verdict == Verdict::Fail).collect();
    assert_eq!(fails.len(), 1, "only the renamed one may fire: {findings:#?}");
    assert!(fails[0].detail.contains("renamed_away"), "{:?}", fails[0].detail);
    // The positive control is the other three passing — and `renamed_away` *appears* in the file,
    // at its call site, so this also asserts that presence alone is not enough.
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

// ── does the tree hold? ─────────────────────────────────────────────────────────────────────

#[test]
fn every_cited_experiment_exists() {
    let findings = paths::check_experiments(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn no_line_anchor_points_into_our_own_tree() {
    let findings = paths::check_anchors(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_bare_named_document_exists() {
    let findings = paths::check_bare_docs(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_cited_symbol_is_declared() {
    let findings = paths::check_symbols(&root());
    let bad: Vec<_> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_referenced_path_exists() {
    let findings = paths::check(&root());
    let bad: Vec<&Finding> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(
        bad.is_empty(),
        "documents name files that are not in the tree:\n{}",
        bad.iter()
            .map(|f| format!("  {} — {}", f.subject, f.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn every_experiment_tally_matches_its_transcript() {
    let findings = tallies::check(&root());
    assert!(!findings.is_empty(), "found no recorded tallies to check — the reader is not reading");
    let bad: Vec<&Finding> = findings.iter().filter(|f| f.verdict != Verdict::Pass).collect();
    assert!(
        bad.is_empty(),
        "a writeup disagrees with the run it describes:\n{}",
        bad.iter()
            .map(|f| format!("  {} — {}", f.subject, f.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
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
