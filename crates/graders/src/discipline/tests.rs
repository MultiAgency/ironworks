//! Controls for the service-wide evidence rules.
//!
//! Every rule is pinned in **both** directions. A rule with only accepted controls passes by
//! vacuity the moment it stops firing, which is exactly how a narrowing goes wrong — so each
//! accepted control here has a rejected sibling that must still trip the same rule.
//!
//! The strings are the ones the Python fixture uses, kept verbatim so a verdict that moves is a
//! change in the rule rather than in the example.

use super::*;
use crate::Verdict;

// ── the absence rule ────────────────────────────────────────────────────────────────────────

const ONBOARDING_LEAD: &str = "Onboarding is not complete — readiness is UNKNOWN. The agreement \
was signed, but the sample export and technical owner are absent (JW-A2, 2026-08-05). ";

#[test]
fn an_absence_scoped_to_the_record_is_not_an_overclaim() {
    // Four accepted forms, each scoping the claim a different way. All must pass.
    for scoped in [
        "There is no signed readiness approval on record.",
        "There is no evidence that technical review occurred.",
        "There are no recorded readiness events in the supplied book.",
        "For Vantage no agreement, contacts or scope is documented.",
    ] {
        let answer = format!("{ONBOARDING_LEAD}{scoped}");
        assert_eq!(absence_as_negative(&answer), None, "should be scoped: {scoped:?}");
    }
}

#[test]
fn an_unscoped_absence_is_an_overclaim() {
    // `no X exists` — the form with an unbounded subject, which stays unbounded.
    let answer = format!("{ONBOARDING_LEAD}No approval exists.");
    assert!(absence_as_negative(&answer).is_some(), "unscoped `no X exists` must fail");
}

#[test]
fn there_is_no_still_fires_on_a_record_artifact() {
    // **The non-vacuity control.** `there is no …` was narrowed to record artifacts after it
    // graded "there is no ambiguity" as an overclaim. If this ever passes, the narrowing went too
    // far and the branch has stopped detecting anything.
    let answer = "The last business day is the current deadline. The written variation dated \
August 18, 2026 replaced the former 15th-of-the-month requirement. There is no signed amendment \
after it.";
    assert!(
        absence_as_negative(answer).is_some(),
        "the `there is no <artifact>` form must still be caught"
    );
}

#[test]
fn there_is_no_does_not_fire_on_an_abstraction() {
    // The defect `0020` found: a claim about how clear the evidence is, in a clause that cites it.
    let answer = "The last business day is the current deadline. The written variation dated \
August 18, 2026 replaced the former 15th-of-the-month requirement, and both parties signed it, so \
there is no ambiguity.";
    assert_eq!(
        absence_as_negative(answer),
        None,
        "`no ambiguity` asserts nothing about the record"
    );
}

#[test]
fn a_bare_artifact_absence_is_caught_per_clause() {
    // "…records, and the pilot ends with no signed extension" names the record in one clause and
    // asserts in the other. Judged per sentence the second hides behind the first.
    let answer = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records, and the pilot ends with no signed extension.";
    assert!(absence_as_negative(answer).is_some(), "the bare artifact form must be caught");
}

#[test]
fn a_clause_naming_the_record_scopes_its_own_artifact_absence() {
    let answer = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records; the record contains no signed extension.";
    assert_eq!(absence_as_negative(answer), None);
}

#[test]
fn asking_someone_to_verify_an_absence_is_not_asserting_it() {
    // The verb must govern the phrase, so position is checked rather than mere presence.
    let answer = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records. A human should confirm no other obligations \
remain before closing Lakeshore.";
    assert_eq!(absence_as_negative(answer), None);
}

#[test]
fn recommendation_prose_is_not_a_claim_about_the_record() {
    // "No action needed" is why the bare form is bounded to artifacts at all.
    let answer = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records.\n\
Lakeshore offboarding is documented as complete. No action needed.";
    assert_eq!(absence_as_negative(answer), None);
}

// ── the four corrections in 0026, each with the control that keeps it honest ─────────────────

#[test]
fn a_trailing_participle_scopes_the_absence_before_it() {
    // Accepted. Both sentences state what the record does not carry; the participle at the end
    // qualifies the whole list. Graded as overclaims because the sentence branch had no way to
    // see it (0026 defect 1).
    for scoped in [
        "There is no signed agreement, no documented onboarding, and no contacts recorded.",
        "There is no signed agreement, change note, or approval document recorded for NC-003.",
    ] {
        assert_eq!(absence_as_negative(scoped), None, "trailing scope: {scoped:?}");
    }
}

#[test]
fn a_record_word_before_the_assertion_does_not_scope_it() {
    // **Rejected, and the reason the fix above is positional.** The record is named in the first
    // clause and the second asserts anyway. A test for "does this sentence mention the record"
    // would pass this, which is exactly what `scoped_by_own_noun` was written tight to avoid.
    let answer = "The record shows a draft and there is no signed amendment.";
    assert!(absence_as_negative(answer).is_some(), "a leading record word must not scope");
}

#[test]
fn on_file_scopes_an_absence() {
    // Accepted. The commoner business idiom for the same claim (0026 defect 2).
    assert_eq!(absence_as_negative("No signed renewal agreement on file."), None);
}

#[test]
fn on_file_does_not_scope_what_it_does_not_qualify() {
    // Rejected. Adding a phrase to SCOPED widens every sentence containing it, so the unscoped
    // form must still fail when the phrase is absent.
    assert!(absence_as_negative("No signed renewal agreement.").is_some());
}

#[test]
fn record_words_are_matched_with_their_inflections() {
    // Accepted. `evidenced` and `documentation` name the record as plainly as `evidence` and
    // `documented`, and were missing (0026 defect 3).
    for scoped in [
        "No agreement is signed, no onboarding is evidenced.",
        "The extension documentation is incomplete with no signed change note.",
    ] {
        assert_eq!(absence_as_negative(scoped), None, "inflection: {scoped:?}");
    }
}

#[test]
fn an_inflection_does_not_scope_a_separate_assertion() {
    // Rejected. Widening NAMES_RECORD skips more clauses, so an assertion that merely shares a
    // sentence with one of the new words must still be caught — here by the sentence branch,
    // because the record word precedes it.
    let answer = "The documentation is thorough. There is no signed agreement.";
    assert!(absence_as_negative(answer).is_some(), "a nearby inflection must not scope");
}

#[test]
fn no_and_exists_must_belong_to_one_noun_phrase() {
    // Accepted. A recommendation, not an assertion: the `no` determines "live work" and the
    // `exists` predicates "onboarding evidence" (0026 defect 4).
    let answer = "No live work should proceed until onboarding evidence exists.";
    assert_eq!(absence_as_negative(answer), None);
}

#[test]
fn no_agreement_record_exists_is_scoped_by_its_own_noun() {
    // `0007`: the thing said not to exist is the *record*. Read as an overclaim once; it is not.
    assert_eq!(absence_as_negative("For Vantage no agreement record exists."), None);
}

#[test]
fn quoted_evidence_is_not_the_answers_own_assertion() {
    // The answer reports what the record says; it has asserted nothing.
    let answer =
        r#"The note reads "no signed extension exists" and that is evidence, not authority."#;
    assert_eq!(absence_as_negative(answer), None);
}

// ── compound evidence tiers ─────────────────────────────────────────────────────────────────

#[test]
fn two_tiers_joined_on_one_claim_fail() {
    let answer = "Juniper still needs its sample export and technical owner — renewal is FACT and \
UNKNOWN, 2026-08-05 — and Northline has contradictory renewal records.";
    assert!(compound_tier_claim(answer).is_some(), "FACT and UNKNOWN on one claim must fail");
}

#[test]
fn two_separately_tagged_claims_pass() {
    // The em-dash is the claim/citation delimiter, so these are two tagged claims, not one claim
    // tagged twice. Without honouring it a disciplined answer fails.
    let answer = "Juniper's security and billing contacts arrived — FACT, 2026-08-09 email — but \
its sample export and technical owner name are still outstanding — UNKNOWN, no later record. \
Northline has contradictory renewal records.";
    assert_eq!(compound_tier_claim(answer), None, "delimited tiers are separate claims");
}

#[test]
fn the_same_tier_twice_is_not_a_compound_claim() {
    assert_eq!(compound_tier_claim("the export is FACT and the owner is FACT"), None);
}

/// Accepted control for `0022`'s defect, now corrected.
///
/// The answer `0022` measured failing: one claim, one tier, and the ordinary adjective in *"a
/// stated prerequisite"* read as the STATED tier because the rule matched a case-folded copy. It
/// failed the service-wide prefilter while all three of its own gates passed.
#[test]
fn a_lowercase_adjective_is_not_an_evidence_tier() {
    let sound = "No record shows sample export was supplied — **UNKNOWN**, this was explicitly \
requested and is a stated prerequisite for technical review.";
    assert_eq!(
        compound_tier_claim(sound),
        None,
        "`stated` the adjective is ordinary English; only the capitalised tag is a tier"
    );
    // Every lowercase tier-word is ordinary English, and none of them is a tag.
    for word in ["fact", "stated", "inference", "unknown"] {
        let prose = format!("The renewal is UNKNOWN, and the handoff is a {word} condition.");
        assert_eq!(compound_tier_claim(&prose), None, "lowercase `{word}` is not a tier");
    }
}

/// Rejected control for the same correction — **required**, or the narrowing passes by vacuity.
///
/// Requiring capitals must not stop the rule catching a real compound claim. If this ever passes,
/// the rule has been narrowed into doing nothing and the accepted control above is worthless.
#[test]
fn capitalised_tiers_on_one_claim_still_fail() {
    let compound = "No record shows sample export was supplied — UNKNOWN and STATED, this was \
explicitly requested.";
    assert!(
        compound_tier_claim(compound).is_some(),
        "two capitalised tiers joined on one claim must still fail"
    );
}

// ── markdown tables ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_markdown_table_fails_by_rows_or_by_separator() {
    assert!(has_markdown_table("| Account | Issue |\n| Juniper | export |"), "consecutive rows");
    assert!(has_markdown_table("Account | Issue\n|---|---|\nJuniper | export"), "separator row");
    assert!(!has_markdown_table("Juniper needs its export.\nNorthline has a conflict."));
    assert!(!has_markdown_table("a | b"), "one inline pipe is not a table");
}

// ── counterparty ownership ──────────────────────────────────────────────────────────────────

#[test]
fn ownership_moved_outward_needs_a_cited_request() {
    let uncited = "Juniper still needs its sample export and technical owner. With Juniper Works.";
    assert!(uncited_counterparty_ownership(uncited).is_some());

    let cited = "Juniper still needs its sample export and technical owner. With Juniper Works — \
MultiAgency asked for both in JW-A2 on 2026-08-05. Northline has contradictory renewal records.";
    assert_eq!(
        uncited_counterparty_ownership(cited),
        None,
        "a request plus a citation discharges it"
    );
}

#[test]
fn work_kept_with_the_operator_needs_no_citation() {
    // Ownership defaults inward; keeping it there is not a claim that needs evidence.
    assert_eq!(uncited_counterparty_ownership("The change note remains with MultiAgency."), None);
}

// ── finding counts ──────────────────────────────────────────────────────────────────────────

#[test]
fn a_lead_count_must_match_the_numbered_findings() {
    let consistent = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records.";
    assert_eq!(inconsistent_finding_count(consistent), None);

    let mismatched = "Three material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records.";
    assert_eq!(inconsistent_finding_count(mismatched), Some((3, 2)));
}

#[test]
fn a_lead_count_is_found_where_a_lead_actually_puts_it() {
    // Both controls above lead with the number, and that is the one shape the rule could read:
    // the scan returned from the whole function on the first word that was not a number, so the
    // loop below it never ran a second time. Every lead this service writes begins with something
    // else — a tier the persona teaches, or ordinary prose — and each of these answered `None`.
    let with_tier = "UNKNOWN — three material issues need a person.\n\
1. Juniper still needs its sample export.\n\
2. Northline has contradictory renewal records.";
    assert_eq!(inconsistent_finding_count(with_tier), Some((3, 2)));

    let with_prose = "There are 4 findings.\n1. Juniper.\n2. Northline.";
    assert_eq!(inconsistent_finding_count(with_prose), Some((4, 2)));

    // The rejected control's partner: the same shapes, counted correctly, must stay clean. A scan
    // that reached past word one and then matched anything would pass the two above and fail here.
    let agreeing = "UNKNOWN — two material issues need a person.\n1. Juniper.\n2. Northline.";
    assert_eq!(inconsistent_finding_count(agreeing), None);
}

#[test]
fn the_count_is_read_in_the_noun_this_service_actually_uses() {
    // No recorded lead in the whole corpus contains the word `issue` or `finding`; this service
    // counts accounts. With only the original nouns the rule could not fire on a single answer it
    // was ever shown, which is `0025`'s defect — a vocabulary that was a list, not a rule.
    let mismatched = "THREE ACCOUNTS DESERVING ATTENTION THIS WEEK\n\
1. Juniper still needs its sample export.\n\
2. Northline has contradictory renewal records.";
    assert_eq!(inconsistent_finding_count(mismatched), Some((3, 2)));

    let agreeing = "Two accounts need attention.\n1. Juniper.\n2. Northline.";
    assert_eq!(inconsistent_finding_count(agreeing), None);
}

#[test]
fn a_lead_that_states_a_breakdown_states_no_single_count() {
    // The rejected controls, and they are the reason the noun set could not simply be widened.
    // Every lead here is real — these are the shapes this service leads with — and taking the
    // first number followed by a noun reads a denominator or one part of a breakdown as the
    // length of the list. Measured across the corpus, that is seventeen false failures.
    let items = "\n1. Juniper.\n2. Northline.\n3. Lakeshore.\n4. Cedar Harbor.";
    for lead in [
        "**Six account records examined. Four need attention now.**",
        "**Three accounts need your attention now** — I examined all 6 account records.",
        "**Four of six accounts need attention now** — two time-sensitive, one contradictory.",
        "**Three accounts need decisions now; two need verification; one is complete.**",
    ] {
        assert_eq!(
            inconsistent_finding_count(&format!("{lead}{items}")),
            None,
            "{lead:?} names no single count and must not be guessed at"
        );
    }
}

#[test]
fn a_lead_naming_its_evidence_is_not_naming_a_count() {
    // `record(s)` is deliberately outside the noun set. No recorded lead says `<number> records`,
    // so admitting it would be neutral today — and `two records disagree` is the natural lead for
    // the `contradiction` case, where it names the conflicting evidence the answer was asked
    // about. Reading it as a finding count would fail an answer for doing its job.
    let contradiction_lead = "Whether Northline renewed is UNKNOWN. Two records disagree.\n\
1. The twelve-month renewal is recorded.\n\
2. Procurement has not approved it.\n\
3. Neither record resolves the other.";
    assert_eq!(inconsistent_finding_count(contradiction_lead), None);
}

#[test]
fn an_answer_that_claims_no_count_cannot_contradict_itself() {
    let no_claim = "These need attention.\n1. Juniper.\n2. Northline.";
    assert_eq!(inconsistent_finding_count(no_claim), None);
}

// ── the lead, and affirmative claims ────────────────────────────────────────────────────────

#[test]
fn the_lead_must_carry_the_unresolved_verdict() {
    assert!(lead_is_unresolved("Whether Northline renewed is UNKNOWN. Two records disagree."));
    assert!(lead_is_unresolved("Full offboarding is not established.\nTwo items are closed."));
    assert!(
        !lead_is_unresolved("Northline renewed.\nBut it is UNKNOWN whether procurement approved."),
        "burying the uncertainty on a later line does not discharge the rule"
    );
}

#[test]
fn refusing_a_claim_is_not_making_it() {
    // The whole point of `affirmative`: an answer that names a forbidden claim in order to reject
    // it must not be read as asserting it.
    let refusal = "The record does not establish a confirmed budget.";
    assert!(!affirmative(refusal).contains("confirmed budget"), "{:?}", affirmative(refusal));

    let assertion = "The account has a confirmed budget.";
    assert!(affirmative(assertion).contains("confirmed budget"));
}

// ── the combinator ──────────────────────────────────────────────────────────────────────────

#[test]
fn service_wide_rules_run_before_the_case_grader() {
    // A case grader that would pass must not be reached when a service-wide rule is broken.
    let always_pass = |_: &str| Graded::pass("case grader ran");
    let broken = format!("{ONBOARDING_LEAD}No approval exists.");
    let got = evidence_disciplined(&broken, always_pass);
    assert_eq!(got.verdict, Verdict::Fail);
    assert!(got.why.contains("asserted non-existence"), "{}", got.why);
}

#[test]
fn a_definite_defect_outranks_an_unresolvable_one() {
    // Ordering: the FAIL rules are checked before the REVIEW rules, so a real defect is never
    // downgraded to "needs a human".
    let always_pass = |_: &str| Graded::pass("case grader ran");
    let both = "| Account | Issue |\n| Juniper | export |\nWith Juniper Works.";
    assert_eq!(evidence_disciplined(both, always_pass).verdict, Verdict::Fail);
}

#[test]
fn a_clean_answer_reaches_its_case_grader() {
    let always_pass = |_: &str| Graded::pass("case grader ran");
    let clean = "Two material issues need attention.\n\
1. Juniper still needs its sample export and technical owner.\n\
2. Northline has contradictory renewal records.";
    let got = evidence_disciplined(clean, always_pass);
    assert_eq!(got.verdict, Verdict::Pass);
    assert_eq!(got.why, "case grader ran", "the combinator must not shadow the grader's reason");
}

// ── uncited counterparty ownership: a question is not a claim ────────────────────────────────

/// Accepted. The three lines `0026` recorded as a false positive, verbatim in shape.
///
/// Each names something the record does not settle and tags it UNKNOWN. That is the evidence
/// discipline working; reading it as an uncited ownership move penalised the answer for saying
/// what it did not know.
#[test]
fn an_indirect_question_is_not_an_ownership_move() {
    for asked in [
        "Whether MultiAgency is waiting on Cedar Harbor's response or has not yet sent it — UNKNOWN",
        "Whether Cedar Harbor has been asked to sign — UNKNOWN",
        "It is unclear whether the signature sits with Cedar Harbor Studio — UNKNOWN",
    ] {
        assert_eq!(
            uncited_counterparty_ownership(asked),
            None,
            "an indirect question is not a claim: {asked:?}"
        );
    }
}

/// Rejected. **The control that stops the guard swallowing the rule.**
///
/// The opener must *govern* the claim. An answer that moves ownership outward and later admits
/// something is unclear has still moved it, and an uncited move is what this rule is for.
#[test]
fn an_uncertainty_word_after_the_claim_does_not_excuse_it() {
    let asserted = "The signature sits with Cedar Harbor Studio, though whether they received it \
is unclear.";
    assert!(
        uncited_counterparty_ownership(asserted).is_some(),
        "a trailing `whether` must not excuse an uncited ownership move"
    );
    // And the plain uncited move is untouched.
    assert!(
        uncited_counterparty_ownership("Continuation documentation — With Cedar Harbor Studio.")
            .is_some()
    );
}

// ── the negated-inference frame (0028's held-out failure) ────────────────────────────────────

/// Accepted. The exact sentence `0028`'s held-out run failed on.
///
/// The answer denies that a disabled credential licenses the conclusion; the rule read the
/// conclusion as the answer's own claim. `0028` verified the clause alone fires and the same
/// sentence without the embedded absence does not, which is what identified the frame.
#[test]
fn an_absence_inside_a_denied_inference_is_not_asserted() {
    for denied in [
        "A credential being disabled is not proof that no alternate access path exists.",
        "A member deletion is not evidence that no bearer session exists.",
        "That the directory entry was removed does not prove that no credential exists.",
    ] {
        assert_eq!(absence_as_negative(denied), None, "denied inference: {denied:?}");
    }
}

/// Rejected — **both halves required by `0028`**, which named them before the fix existed:
/// *"an accepted one for `is not proof that no X exists`, and a rejected one keeping the plain
/// `no X exists` form failing — the two differ by a frame, which is precisely the kind of
/// narrowing that goes wrong quietly."*
#[test]
fn the_plain_form_still_fails_and_a_loose_frame_does_not_excuse_it() {
    // The bare assertion, which the frame must not have made unreachable.
    assert!(
        absence_as_negative("No alternate access path exists.").is_some(),
        "the plain `no X exists` form must still fail"
    );
    // A negator and an establishing word in the sentence are not the frame. This is a real
    // overclaim sharing a sentence with a denial, and `that` never governs the assertion.
    let separate = "The record does not establish the handover, and no signed amendment exists.";
    assert!(
        absence_as_negative(separate).is_some(),
        "a denial in a neighbouring clause must not scope a separate assertion"
    );
}
