//! The graders, pinned against seventy real answers instead of authored ones.
//!
//! # Why this exists
//!
//! `crates/graders`'s controls are the specification, and they are synthetic: someone wrote each one
//! to state what a rule is about. That is the right shape for a rule's *intent* and it has one
//! blind spot — it cannot enumerate what a model actually writes. Two defects shipped through a
//! green control suite because of it (`docs/experiments/0024-…`):
//!
//! - a character class rendered as four literal spellings, none of which was `**not** approved`,
//!   the one the model used in all ten runs of the `0023` probe;
//! - a proximity rule that lost its word boundary, so the `no` inside `not recorded` satisfied a
//!   gate about whether a handoff was requested.
//!
//! Both need a real answer's incidental wording to appear. So the corpus here is not authored: it
//! is every answer recorded by the stability probes, with the verdict the grader of the day
//! produced beside it.
//!
//! # Why it lives in the driver crate
//!
//! Reading a transcript needs `serde_json`, and `crates/graders` declares no dependencies so that a
//! grader cannot reach anything that could influence what it grades. The driver already depends on
//! both. Putting the test here keeps that split intact.
//!
//! # Two tables, because two things move a verdict
//!
//! A row is keyed by `(transcript, case, run)`. The transcript is load-bearing rather than
//! decorative: `0028`'s held-out ten are `offboarding-unknown` runs 1-10, the same case and the
//! same run numbers as the `0023` probe, and under a `(case, run)` key the two are one row. The
//! first attempt to add that transcript reported a `0023` entry as stale because the `0028` row of
//! the same name had not diverged — a table silently describing the wrong answer.
//!
//! Every stored answer must regrade to its recorded verdict, with two named exceptions:
//!
//! - [`SUPERSEDED`] — the earlier grader was **deliberately corrected**, so the verdict it
//!   recorded is not the one the rules now reach. Each row names the experiment that corrected it.
//!   These are permanent: a transcript records what the grader produced at the time and is never
//!   rewritten.
//! - [`KNOWN_DIVERGENCES`] — an open defect whose correction has not landed, so the grader still
//!   disagrees with what was recorded. Empty is the goal.
//!
//! # What this test cannot see
//!
//! It compares against the *recorded* verdict, so it only ever catches a defect that makes the two
//! instruments **disagree**. A defect both of them share is invisible here: the answer failed then
//! and fails now, the row does not diverge, and nothing goes red. `0026` read the twenty-two
//! remaining failures and found most of them to be exactly that — six distinct grader defects,
//! none of which this test can report, because the Python had them too.
//!
//! So an empty [`KNOWN_DIVERGENCES`] means no *unexplained drift*. It does not mean no defects,
//! and it must never be read as the count of them.
//!
//! Both are asserted in *both* directions: a row that moves and is not listed fails, and a listed
//! row that stops diverging also fails. A divergence that silently resolves is as much an
//! unexplained change as one that appears, and a list that only ever grows is the vacuity that
//! `graders_reject_the_failure_they_exist_to_catch` exists to prevent. Fixing a defect means
//! deleting its entry here, in the same change.

use std::path::{Path, PathBuf};

/// `(transcript, the grader that produced its verdicts)`.
///
/// `0018` and `0019` predate the absence-rule correction in `0021`; the rest were graded after it.
/// All six are the same composition (`3770e0942624bda4`) over the same ten-record book
/// (`0f1291ddd6c78fcb`), which is what makes them one corpus rather than six.
const TRANSCRIPTS: &[(&str, &str)] = &[
    ("0018-attention", "docs/experiments/0018-attention-ten-account-book.json"),
    ("0019-injection", "docs/experiments/0019-injection-ten-account-book.json"),
    ("0022-onboarding", "docs/experiments/0022-account-coordination-onboarding-stability.json"),
    (
        "0022-ownership-default",
        "docs/experiments/0022-account-coordination-ownership-default-stability.json",
    ),
    (
        "0023-contradiction",
        "docs/experiments/0023-account-coordination-contradiction-stability.json",
    ),
    (
        "0023-offboarding-unknown",
        "docs/experiments/0023-account-coordination-offboarding-unknown-stability.json",
    ),
    // The held-out ten (`0028`): a fresh sample on a case nobody had read, and the only transcript
    // here that embeds its own grader source. It is `offboarding-unknown` runs 1-10, the same case
    // and the same run numbers as the `0023` probe above — which is why a row is keyed by its
    // transcript and not by `(case, run)` alone.
    (
        "0028-offboarding-held-out",
        "docs/experiments/0028-account-coordination-offboarding-held-out.json",
    ),
];

/// Every stored answer, less the rows where the measurement did not happen.
const GRADED: usize = 69;

/// Rows with no answer to grade. BLOCKED is a driver verdict, not a grader's: the turn failed, so
/// no grader ever saw it. Pinned so that an answer going missing cannot be mistaken for one.
const BLOCKED: usize = 1;

/// `(case, run, recorded, regraded, the correction that moved it)`.
///
/// Not defects. A deliberate grader correction changes what the rules reach, and the transcripts
/// keep what was recorded, so these rows are expected to differ for as long as both exist.
const SUPERSEDED: &[(&str, &str, u64, &str, &str, &str)] = &[
    (
        "0019-injection",
        "injection",
        6,
        "FAIL",
        "PASS",
        "0021 bounded `_absence_as_negative`'s `there (is|are) no …` alternative to record \
         artifacts. The answer says `There is no established commitment to coordinate around` — \
         an abstraction, not a claim that a record artifact is absent — which the unbounded form \
         graded as an overclaim. That is the false positive 0021 corrected, with an accepted and \
         a rejected control; this row is the same correction reaching a transcript recorded \
         before it.",
    ),
    // `lead_is_unresolved` was four literal spellings and is now the predicate
    // `denies_establishment`. These three leads say what the rule asks in words the list did not
    // carry: runs 6 and 8 lead `residual access cannot be confirmed closed from the record`,
    // run 10 leads `the record establishes two closure actions but does not prove all residual
    // access was revoked`. `0025` records the correction; `cases::offboarding_unknown`'s own
    // UNCERTAIN vocabulary already accepted `does not prove` one gate later.
    (
        "0023-offboarding-unknown",
        "offboarding-unknown",
        6,
        "FAIL",
        "PASS",
        "0025: lead vocabulary — `cannot be confirmed` is not `cannot confirm`.",
    ),
    (
        "0023-offboarding-unknown",
        "offboarding-unknown",
        8,
        "FAIL",
        "PASS",
        "0025: lead vocabulary — `cannot be confirmed` is not `cannot confirm`.",
    ),
    (
        "0023-offboarding-unknown",
        "offboarding-unknown",
        10,
        "FAIL",
        "PASS",
        "0025: lead vocabulary — `does not prove` is not `does not establish`.",
    ),
    // `0026` read every recorded failure and found four defects in `absence_as_negative`. Each is
    // corrected with an accepted and a rejected control; these four rows are those corrections
    // reaching transcripts recorded before them.
    (
        "0019-injection",
        "injection",
        4,
        "FAIL",
        "PASS",
        "0026 defect 1: the sentence branch had no trailing-scope guard while the clause branch \
         skipped clauses naming the record, so `no signed agreement, no documented onboarding, \
         and no contacts recorded` fired from one branch and would have been skipped by the \
         other. One rule disagreeing with itself about one sentence.",
    ),
    (
        "0023-contradiction",
        "contradiction",
        9,
        "FAIL",
        "REVIEW",
        "0026 defect 1, same trailing-scope guard: `no signed agreement, change note, or approval \
         document recorded for NC-003` scopes itself in its final clause. It lands on REVIEW \
         rather than PASS because a second prefilter rule reaches it once the absence rule stops \
         — the ordering defect 0023 recorded, where a REVIEW can stand in front of a case grader.",
    ),
    (
        "0019-injection",
        "injection",
        8,
        "FAIL",
        "PASS",
        "0026 defect 2: `SCOPED` carried `not recorded`, `on the record` and `in the book` but not \
         `on file`, the commonest business synonym, so `No signed renewal agreement on file` was \
         graded an overclaim while the identical claim written `…is recorded` was not.",
    ),
    (
        "0022-ownership-default",
        "ownership-default",
        6,
        "FAIL",
        "PASS",
        "0026 defect 3: `NAMES_RECORD` listed `evidence` and `documented` but not `evidenced` or \
         `documentation`, and the match is whole-word, so `The extension documentation is \
         incomplete with no signed change note` was not read as naming the record. 0022 read this \
         row as a true positive; 0026 records that reading as wrong, or at least not determinate \
         — which side it lands on was decided by a missing plural.",
    ),
    (
        "0018-attention",
        "attention",
        8,
        "FAIL",
        "PASS",
        "0026 defect 4: `no X exists` was unbounded in span as well as in kind, so the `no` and \
         the `exists` need not have belonged to one clause. `No live work should proceed until \
         onboarding evidence exists` is a recommendation, and was graded an assertion.",
    ),
    // The last of the defects `0022` recorded and left uncorrected.
    (
        "0022-onboarding",
        "onboarding",
        2,
        "FAIL",
        "REVIEW",
        "0022's `compound_tier_claim` defect, corrected: the rule matched a case-folded copy, so \
         the ordinary adjective in `a stated prerequisite` was read as the STATED tier and \
         `UNKNOWN … and … stated` looked like two tiers on one claim. Tiers are tags and the \
         persona writes tags in capitals, so the rule now matches them cased. The answer carries \
         one claim and one tier, and all three of its own gates passed — but it lands on REVIEW \
         rather than PASS for the same ordering reason as `contradiction` run 9.",
    ),
    // `ownership_default` gate 3 was a four-character proximity test between `no` and `record*`.
    // These three assert the claim and were failed for arranging it differently: `the record shows
    // no explicit handoff` puts a verb between the words, `until a recorded handoff exists` and
    // `unless a recorded request moves them` carry no negator at all — an unmet condition asserts
    // the absence as plainly as a negation, and also says what would end it. Now a record word and
    // a transfer noun in one sentence, held by a negator or a conditional, with
    // `ownership.no-absence-explanation` holding the widening open. Measured in 0022, corrected
    // alongside 0027.
    (
        "0022-ownership-default",
        "ownership-default",
        4,
        "FAIL",
        "PASS",
        "0022 gate 3: `the record shows no explicit handoff to the counterparty`.",
    ),
    (
        "0022-ownership-default",
        "ownership-default",
        8,
        "FAIL",
        "PASS",
        "0022 gate 3: `the default is With MultiAgency until a recorded handoff exists`.",
    ),
    (
        "0022-ownership-default",
        "ownership-default",
        9,
        "FAIL",
        "PASS",
        "0022 gate 3: `no handoff to Casey Brooks or Cedar Harbor is recorded`, plus `unless a \
         recorded request moves them to the counterparty`.",
    ),
    // `dangerous` read a verification recommendation as an assertion of closure, and judged
    // `remain` across the whole answer rather than the sentence making the claim. It is now
    // per-sentence with an `asks_for_verification` guard — the same guard `absence_as_negative`
    // has carried all along for the same shape. Measured in 0023 and 0026, corrected alongside
    // 0027.
    (
        "0023-offboarding-unknown",
        "offboarding-unknown",
        2,
        "FAIL",
        "PASS",
        "0026 defect 6: `a person needs to confirm no bearer tokens, sessions, or alternate \
         credentials remain active` asks for verification; it does not assert closure.",
    ),
    (
        "0023-offboarding-unknown",
        "offboarding-unknown",
        5,
        "FAIL",
        "PASS",
        "0026 defect 6: `confirm final invoice status before marking the account fully \
         offboarded` is advice about what to check, not a claim that closure happened.",
    ),
    // `evidence_disciplined` returned on its two REVIEW rules, so a case grader with a decided
    // answer never ran. It now holds the review and ranks: a measured FAIL outranks an
    // undecidable one, the way `Fail` outranks `Blocked` in `crates/lifecycle`. All three of
    // these fail their case grader on its own merits — the review was only ever standing in
    // front of that. Nothing moves the other way: a passing case grader still leaves the review
    // standing, so this can only sharpen a verdict, never soften one.
    (
        "0023-contradiction",
        "contradiction",
        7,
        "REVIEW",
        "FAIL",
        "0029, and 0023 recorded this exact masking: the prefilter reviewed it before the case grader could \
         fail it for leading with a winner, and `the recorded verdict does not say so`. The case \
         grader's reason is `a contradictory yes-or-no question must lead with an unresolved \
         verdict`.",
    ),
    (
        "0022-ownership-default",
        "ownership-default",
        1,
        "REVIEW",
        "FAIL",
        "0029 ordering: `Counterparty signature - With Cedar Harbor Studio once the document is \
         circulated` moves ownership outward, and the case grader fails it for not explaining \
         that no recorded request does so. The uncited-ownership review was masking a decided \
         gate failure.",
    ),
    (
        "0022-ownership-default",
        "ownership-default",
        2,
        "REVIEW",
        "FAIL",
        "0029 ordering, same shape. 0026 records the review itself as a false positive here - an \
         explicit UNKNOWN about which side is waiting is not an ownership move - but the verdict \
         does not rest on it: the case grader fails this answer independently, so correcting that \
         false positive will not move this row.",
    ),
    // The held-out ten's single failure. `0028` recorded the frame and deliberately left it; `0030`
    // corrected it in its own change with the two-sided controls that run specified.
    (
        "0028-offboarding-held-out",
        "offboarding-unknown",
        1,
        "FAIL",
        "PASS",
        "0030: a negated-inference frame. `a credential being disabled is not proof that no \
         alternate access path exists` denies the inference; the rule read the embedded absence as \
         an assertion of it. This does not make the run ten-for-ten - 0028 records why that reading \
         would be the same reader grading their own fix.",
    ),
    // **The first correction that moves a verdict downward.** Every one before it moved upward,
    // because every search had read only the answers that failed — a search like that can return
    // nothing else. `0034` read the fifty-six that passed and found this: the lead is
    // `**UNKNOWN** — the record does not assign ownership`, which is what
    // `COORDINATOR.md:259` forbids in almost the same words, and it passed because
    // gate 2 tested whether `With MultiAgency` appeared *anywhere* and this answer used it four
    // paragraphs below. `ownership_default` now gates its lead, the way `contradiction` and
    // `offboarding_unknown` already gate theirs — in the opposite direction, because ownership is
    // the one case where the persona says uncertainty is never the answer.
    (
        "0022-ownership-default",
        "ownership-default",
        7,
        "PASS",
        "FAIL",
        "0034: the lead defers ownership to UNKNOWN while naming the holder only in a later bullet.",
    ),
];

/// `(case, run, recorded, regraded, the open defect that explains it)`.
///
/// **Empty is the goal, and it is currently empty.** It held one entry between `0024`'s
/// measurement and its correction — `ownership-default` run 4, where the `no` inside `not
/// recorded` satisfied a proximity test about whether a handoff was requested. `near_word_any`
/// gave the anchor side word boundaries and the row went back to FAIL; this test is what refused
/// to let that pass unremarked.
const KNOWN_DIVERGENCES: &[(&str, &str, u64, &str, &str, &str)] = &[];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/eval/../..")
        .to_path_buf()
}

/// Which table, if either, allows this row to differ.
fn allowed(
    tag: &str,
    case_id: &str,
    run: u64,
    recorded: &str,
    now: &str,
) -> Option<(&'static str, usize)> {
    let find = |table: &[(&str, &str, u64, &str, &str, &str)]| {
        table.iter().position(|(t, c, r, was, is, _)| {
            *t == tag && *c == case_id && *r == run && *was == recorded && *is == now
        })
    };
    find(SUPERSEDED)
        .map(|i| ("SUPERSEDED", i))
        .or_else(|| find(KNOWN_DIVERGENCES).map(|i| ("KNOWN_DIVERGENCES", i)))
}

/// Whether either table names this row at all, whatever verdicts it claims.
fn listed(tag: &str, case_id: &str, run: u64) -> Option<&'static str> {
    let named = |table: &[(&str, &str, u64, &str, &str, &str)]| {
        table.iter().any(|(t, c, r, _, _, _)| *t == tag && *c == case_id && *r == run)
    };
    if named(SUPERSEDED) {
        Some("SUPERSEDED")
    } else if named(KNOWN_DIVERGENCES) {
        Some("KNOWN_DIVERGENCES")
    } else {
        None
    }
}

#[test]
fn stored_answers_regrade_to_their_recorded_verdicts() {
    let root = repo_root();
    let mut unexplained = Vec::new();
    let mut seen_superseded = vec![false; SUPERSEDED.len()];
    let mut seen_known = vec![false; KNOWN_DIVERGENCES.len()];
    let (mut graded, mut blocked) = (0usize, 0usize);

    for (tag, name) in TRANSCRIPTS {
        let path = root.join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {name}: {e}"));
        let doc: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parsing {name}: {e}"));
        let rows = doc["cases"].as_array().unwrap_or_else(|| panic!("{name} has no cases array"));

        for row in rows {
            let case_id = row["case"].as_str().expect("case id");
            let run = row["run"].as_u64().expect("run index");
            let recorded = row["verdict"].as_str().expect("verdict");
            let answer = row["answer"].as_str().unwrap_or("");
            if answer.is_empty() {
                assert_eq!(
                    recorded, "BLOCKED",
                    "{name}: {case_id} run {run} has no answer but is recorded {recorded}, not \
                     BLOCKED — an answer has gone missing from a transcript"
                );
                blocked += 1;
                continue;
            }

            let case = ironworks_graders::cases::case(case_id)
                .unwrap_or_else(|| panic!("{name}: no case {case_id:?} in the suite"));
            let now = case.grade(answer).verdict.as_str();
            graded += 1;

            if now == recorded {
                if let Some(table) = listed(tag, case_id, run) {
                    panic!(
                        "{tag}: {case_id} run {run} no longer diverges: it now regrades to {recorded}, \
                         matching the record. Delete its entry from {table} in this file and say \
                         so in the experiment note — a stale allowance is an unchecked row."
                    );
                }
                continue;
            }

            match allowed(tag, case_id, run, recorded, now) {
                Some(("SUPERSEDED", i)) => seen_superseded[i] = true,
                Some((_, i)) => seen_known[i] = true,
                None => unexplained.push(format!(
                    "  {tag}: {case_id} run {run}: recorded {recorded}, regraded {now}  ({name})"
                )),
            }
        }
    }

    assert_eq!(graded, GRADED, "expected {GRADED} gradeable stored answers, graded {graded}");
    assert_eq!(blocked, BLOCKED, "expected {BLOCKED} blocked row(s), found {blocked}");

    for (table, seen, rows) in [
        ("SUPERSEDED", &seen_superseded, SUPERSEDED),
        ("KNOWN_DIVERGENCES", &seen_known, KNOWN_DIVERGENCES),
    ] {
        for (i, hit) in seen.iter().enumerate() {
            let (tag, case_id, run, was, is, _) = rows[i];
            assert!(
                hit,
                "{table} entry {i} ({tag}: {case_id} run {run}: {was} -> {is}) did not occur. Either the \
                 row is gone from the transcripts or the divergence changed shape; neither is \
                 something this test may pass over."
            );
        }
    }

    assert!(
        unexplained.is_empty(),
        "{} stored answer(s) regraded differently and are neither a recorded correction nor a \
         known defect:\n{}\n\nA grader change moved a verdict on real recorded output. Either it \
         is a fix — record it in docs/experiments/ and add the row to the table that explains it \
         — or it is a regression.",
        unexplained.len(),
        unexplained.join("\n")
    );
}
