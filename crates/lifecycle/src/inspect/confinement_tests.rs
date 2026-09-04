//! Every branch of the confinement verdict table, asserted by name.
//!
//! The two properties worth stating up front, because both are easy to lose to a well-meaning
//! edit:
//!
//! - **A live clean reading is necessary and not sufficient.** PASS also needs a durable baseline:
//!   the latest attempt for *this* member, clean, against the policy in force. `inspect` cannot
//!   append the measurement it just made, so certifying on the live half alone would be a judgment
//!   with nothing behind it.
//! - **A previous member's history is history.** Replacement over time is legitimate, so old
//!   attempts stay readable and never decide the current verdict — in either direction.

use super::*;

fn unlocked() -> std::collections::BTreeSet<String> {
    std::collections::BTreeSet::new()
}
use crate::attempt::{Blocked, ConfinementAttempt, Measurement};
use crate::confinement::{ConfinementPolicy, PolicyBinding};
use crate::runtime::{BoundSubject, ObservedAt, RuntimeSubject, RuntimeTenantId, RuntimeUserId};
use crate::state::SmokeOutcome;
use crate::{AuthorityFact, Presence, Recorded};
use std::collections::BTreeMap;

const NOW: &str = "2026-08-27T12:00:00+00:00";
const MEMBER: &str = "user_7f3a";

fn spec() -> InstanceSpec {
    InstanceSpec {
        slug: ironworks_domain::Slug::parse("acme").unwrap(),
        org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
        service: ironworks_domain::ServiceId::parse("account-analysis").unwrap(),
        channel: ironworks_domain::ChannelRef::new("-100900001"),
    }
}

fn definition(version: u32) -> ironworks_domain::ServiceDefinition {
    use ironworks_domain::composition::{Composition, PartRef};
    ironworks_domain::ServiceDefinition::new(
        ironworks_domain::ServiceId::parse("account-analysis").expect("id"),
        version,
        "read one organization's own account records and answer from them",
        Composition::new(
            vec![PartRef::new("personas/COORDINATOR.md")],
            "# GUIDANCE",
            PartRef::new("t"),
        )
        .expect("composition"),
        ironworks_domain::CapabilityProfile::READ_ONLY_RECORDS,
        None,
    )
    .expect("definition")
}

fn policy() -> ConfinementPolicy {
    ConfinementPolicy::read_only_analyst()
}

/// A different policy, so a fingerprint mismatch is constructible. The two shipped services both
/// declare `runtime_tools: none` and therefore share a fingerprint, which is exactly why this case
/// belongs in a unit test rather than in a live proof.
fn other_policy() -> ConfinementPolicy {
    ConfinementPolicy::sandbox_workspace()
}

fn binding() -> PolicyBinding {
    PolicyBinding::of(&definition(1), &policy())
}

fn other_binding() -> PolicyBinding {
    PolicyBinding::of(&definition(1), &other_policy())
}

fn subject(user: &str) -> RuntimeSubject {
    RuntimeSubject::new(
        RuntimeTenantId::parse("acme").expect("tenant"),
        RuntimeUserId::parse(user).expect("user"),
    )
}

fn bound_to(member: &str) -> BoundSubject {
    BoundSubject::bind(OpaqueId::new(member), subject(member)).expect("binds")
}

fn at() -> ObservedAt {
    ObservedAt::parse(NOW).expect("timestamp")
}

fn catalogue(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

const SHUT: &[(&str, &str)] = &[("builtin.http", "disabled")];
const OPEN: &[(&str, &str)] = &[("builtin.http", "always_allow")];

fn measurement(member: &str, entries: &[(&str, &str)]) -> Measurement {
    Measurement::from_assessment(
        &policy(),
        &bound_to(member),
        &catalogue(entries),
        &unlocked(),
        false,
        &at(),
    )
}

fn measured_attempt(
    member: &str,
    entries: &[(&str, &str)],
    binding: PolicyBinding,
) -> ConfinementAttempt {
    ConfinementAttempt::measured(bound_to(member), binding, at(), measurement(member, entries))
}

fn live_clean() -> LiveConfinement {
    LiveConfinement::Bound {
        subject: subject(MEMBER),
        confinement: crate::confinement::assess(
            &policy(),
            &OpaqueId::new(MEMBER),
            &catalogue(SHUT),
            &unlocked(),
            false,
            NOW,
        ),
    }
}

fn live_drifted() -> LiveConfinement {
    LiveConfinement::Bound {
        subject: subject(MEMBER),
        confinement: crate::confinement::assess(
            &policy(),
            &OpaqueId::new(MEMBER),
            &catalogue(OPEN),
            &unlocked(),
            false,
            NOW,
        ),
    }
}

/// A world in which every authority is accounted for, so the authority axis contributes PASS and
/// whatever this test asserts is coming from confinement alone.
fn clean_world() -> (Recorded, AuthorityInventory) {
    let ids: Vec<AuthorityFact> = Authority::ALL
        .iter()
        .map(|a| AuthorityFact {
            authority: *a,
            presence: Presence::Present {
                id: OpaqueId::new(if *a == Authority::Member {
                    MEMBER.to_owned()
                } else {
                    format!("{}-1", a.tag())
                }),
            },
        })
        .collect();
    let recorded = Recorded::from_created(
        ProvisionProgress::Activated {
            smoke: crate::state::SmokeEvidence::from_outcome(
                &SmokeOutcome::Passed {
                    isolation: "own org only".into(),
                    reachability: "200".into(),
                },
                NOW,
            )
            .expect("smoke"),
        },
        Authority::ALL.iter().map(|a| {
            (
                *a,
                OpaqueId::new(if *a == Authority::Member {
                    MEMBER.to_owned()
                } else {
                    format!("{}-1", a.tag())
                }),
            )
        }),
    );
    (recorded, AuthorityInventory::Reconciled(ids))
}

fn run(recorded: &Recorded, inventory: &AuthorityInventory, axis: &ConfinementAxis) -> Inspection {
    inspect(&spec(), recorded, inventory, &ServingObservation::Unknown, axis)
}

fn outcome(axis: &ConfinementAxis) -> Option<ConfinementOutcome> {
    let (recorded, inventory) = clean_world();
    run(&recorded, &inventory, axis).confinement.outcome
}

// ── applicability ───────────────────────────────────────────────────────────────────────────

#[test]
fn before_a_member_exists_confinement_is_not_applicable_and_changes_nothing() {
    // The rule that keeps a pre-provisioning inspection from being degraded by a question that
    // cannot be asked yet.
    let recorded = Recorded::from_progress(ProvisionProgress::Provisioning {
        reached: Stage::DataInitializationSettled,
    });
    let inventory = AuthorityInventory::Reconciled(
        Authority::ALL
            .iter()
            .map(|a| AuthorityFact { authority: *a, presence: Presence::Absent })
            .collect(),
    );
    let with = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Recorded { ledger: Ledger::Present, latest: None },
    );
    assert_eq!(with.confinement.outcome, None, "not yet applicable must contribute nothing");

    // And the overall verdict is byte-identical to the one the authority axis produces alone.
    let without = run(&recorded, &inventory, &ConfinementAxis::NotRequested);
    assert_eq!(with.verdict, without.verdict);
}

#[test]
fn a_reached_stage_with_no_recorded_member_blocks() {
    // Distinct from "not yet": the stage claims a member was minted and no identifier was written,
    // so there is nothing to bind a measurement against.
    let recorded =
        Recorded::from_progress(ProvisionProgress::Provisioning { reached: Stage::MemberMinted });
    let inventory = AuthorityInventory::Reconciled(vec![]);
    let report = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Recorded { ledger: Ledger::Present, latest: None },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Blocked));
}

#[test]
fn an_unasked_question_never_lowers_the_verdict() {
    // `inspect` without --confinement made no confinement claim. Reporting BLOCKED for a question
    // nobody asked would degrade every existing invocation for a reason about instrumentation.
    let (recorded, inventory) = clean_world();
    let report = run(&recorded, &inventory, &ConfinementAxis::NotRequested);
    assert_eq!(report.confinement.outcome, None);
    assert_eq!(report.verdict, Verdict::Pass);
    // It still says so, so confinement is never inferred from silence.
    assert!(
        report.confinement.lines.iter().any(|l| l.contains("not asked")),
        "{:?}",
        report.confinement.lines
    );
}

// ── the table ───────────────────────────────────────────────────────────────────────────────

#[test]
fn history_without_a_live_observation_can_never_pass() {
    // Even a perfect baseline. Historical evidence is a fact about a moment, and §L-6's custody
    // boundary means it can be false a second later.
    assert_eq!(
        outcome(&ConfinementAxis::Recorded {
            ledger: Ledger::Present,
            latest: Some(measured_attempt(MEMBER, SHUT, binding())),
        }),
        Some(ConfinementOutcome::Blocked)
    );
}

#[test]
fn a_v1_store_reports_that_it_predates_the_ledger() {
    let (recorded, inventory) = clean_world();
    let report = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::PredatesLedger,
            latest: None,
            live: Box::new(live_clean()),
            current: binding(),
        },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Blocked));
    assert!(
        report.confinement.lines.iter().any(|l| l.contains("predates the confinement ledger")),
        "{:?}",
        report.confinement.lines
    );
}

#[test]
fn a_live_clean_reading_alone_is_not_enough() {
    // The rule the whole design turns on, in its four shapes: no history, and each of the three
    // history shapes that is not a current-policy baseline.
    let mismatch =
        BoundSubject::bind(OpaqueId::new(MEMBER), subject("reborn-cli")).expect_err("differs");
    let cases: [(&str, Option<ConfinementAttempt>); 5] = [
        ("no applicable history", None),
        (
            "a blocked binding",
            Some(ConfinementAttempt::binding_blocked(
                OpaqueId::new(MEMBER),
                at(),
                Blocked::RuntimeUnreachable,
            )),
        ),
        (
            "a historical subject mismatch",
            Some(ConfinementAttempt::subject_mismatch(mismatch, at())),
        ),
        (
            "a blocked catalogue read",
            Some(ConfinementAttempt::catalogue_blocked(
                bound_to(MEMBER),
                binding(),
                at(),
                Blocked::UnreadableAnswer,
            )),
        ),
        ("a drifted measurement", Some(measured_attempt(MEMBER, OPEN, binding()))),
    ];
    for (label, latest) in cases {
        assert_eq!(
            outcome(&ConfinementAxis::Measured {
                ledger: Ledger::Present,
                latest,
                live: Box::new(live_clean()),
                current: binding(),
            }),
            Some(ConfinementOutcome::Blocked),
            "{label} must not certify on the live half alone"
        );
    }
}

#[test]
fn a_baseline_taken_against_another_policy_does_not_certify() {
    // What makes the fingerprint load-bearing: editing a service definition must not keep
    // certifying a member confined to the policy it replaced.
    assert_eq!(
        outcome(&ConfinementAxis::Measured {
            ledger: Ledger::Present,
            // Clean, for this member, and against the OTHER policy.
            latest: Some(measured_attempt(MEMBER, SHUT, other_binding())),
            live: Box::new(live_clean()),
            current: binding(),
        }),
        Some(ConfinementOutcome::Blocked)
    );
    assert_ne!(binding().fingerprint(), other_binding().fingerprint(), "the fixtures must differ");
}

/// The positive control for every BLOCKED above. Without it, an axis that never passed would
/// satisfy all of them.
#[test]
fn a_live_clean_reading_with_a_current_policy_baseline_passes() {
    let (recorded, inventory) = clean_world();
    let report = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: Some(measured_attempt(MEMBER, SHUT, binding())),
            live: Box::new(live_clean()),
            current: binding(),
        },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Pass));
    assert_eq!(report.verdict, Verdict::Pass);
    assert_eq!(report.verdict.exit_code(), 0);
    // And the scope travels with the pass, where an operator reads it.
    assert!(
        report.confinement.lines.iter().any(|l| l.contains("NOT a claim")),
        "{:?}",
        report.confinement.lines
    );
}

#[test]
fn a_live_subject_mismatch_fails() {
    let report = ConfinementAxis::Measured {
        ledger: Ledger::Present,
        latest: Some(measured_attempt(MEMBER, SHUT, binding())),
        live: Box::new(LiveConfinement::Mismatch {
            expected: OpaqueId::new(MEMBER),
            authenticated: subject("reborn-cli"),
        }),
        current: binding(),
    };
    assert_eq!(outcome(&report), Some(ConfinementOutcome::Fail));

    let (recorded, inventory) = clean_world();
    let full = run(&recorded, &inventory, &report);
    assert_eq!(full.verdict, Verdict::Fail);
    assert_eq!(full.verdict.exit_code(), 2);
    // Both identities reach the operator.
    let text = full.confinement.lines.join("\n");
    assert!(text.contains(MEMBER) && text.contains("reborn-cli"), "{text}");
}

#[test]
fn live_drift_fails_even_with_a_clean_baseline() {
    // The baseline is a fact about the past. A measured contradiction now outranks it.
    let (recorded, inventory) = clean_world();
    let report = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: Some(measured_attempt(MEMBER, SHUT, binding())),
            live: Box::new(live_drifted()),
            current: binding(),
        },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Fail));
    let text = report.confinement.lines.join("\n");
    assert!(text.contains("historically certified"), "the historical fact disappeared: {text}");
    assert!(text.contains("outside policy now"), "the current drift was blurred: {text}");
}

#[test]
fn a_blocked_live_observation_blocks() {
    for live in [
        LiveConfinement::Blocked(Blocked::RuntimeUnreachable),
        LiveConfinement::Blocked(Blocked::CredentialRejected),
        LiveConfinement::Bound {
            subject: subject(MEMBER),
            confinement: Confinement::Unverifiable(crate::BlockedReason::CredentialRejected),
        },
    ] {
        assert_eq!(
            outcome(&ConfinementAxis::Measured {
                ledger: Ledger::Present,
                latest: Some(measured_attempt(MEMBER, SHUT, binding())),
                live: Box::new(live),
                current: binding(),
            }),
            Some(ConfinementOutcome::Blocked)
        );
    }
}

// ── temporal member semantics ───────────────────────────────────────────────────────────────

/// A previous member's clean measurement is not the current member's baseline. The caller filters
/// by the current member, so this asserts the shape `latest_applicable` produces: `None`.
#[test]
fn a_previous_members_clean_history_does_not_certify_the_current_one() {
    assert_eq!(
        outcome(&ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: None,
            live: Box::new(live_clean()),
            current: binding(),
        }),
        Some(ConfinementOutcome::Blocked),
        "member replacement must not inherit a baseline"
    );
}

/// And the mirror image, which is the correction that matters: a previous member's *failure* must
/// not condemn the current one either. Replacement over time is legitimate.
#[test]
fn a_previous_members_mismatch_does_not_fail_the_current_one() {
    let report = run(
        &clean_world().0,
        &clean_world().1,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            // `latest_applicable` filters by the current member, so an old member's mismatch is
            // simply not here — and the verdict must be BLOCKED for want of a baseline, never FAIL.
            latest: None,
            live: Box::new(live_clean()),
            current: binding(),
        },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Blocked));
    assert_ne!(report.verdict, Verdict::Fail);
}

/// A *stored* mismatch for the current member is a past failure, not a current one. Without a live
/// observation it can only block.
#[test]
fn a_stored_mismatch_without_a_live_reading_blocks_rather_than_fails() {
    let mismatch =
        BoundSubject::bind(OpaqueId::new(MEMBER), subject("reborn-cli")).expect_err("differs");
    let report = run(
        &clean_world().0,
        &clean_world().1,
        &ConfinementAxis::Recorded {
            ledger: Ledger::Present,
            latest: Some(ConfinementAttempt::subject_mismatch(mismatch, at())),
        },
    );
    assert_eq!(report.confinement.outcome, Some(ConfinementOutcome::Blocked));
    assert_eq!(report.verdict, Verdict::Blocked);
    assert_eq!(report.verdict.exit_code(), 3);
}

// ── precedence and rendering ────────────────────────────────────────────────────────────────

#[test]
fn fail_outranks_blocked_across_the_two_axes() {
    // A blocked authority inventory and a failing confinement: the sharper fact wins.
    let recorded = clean_world().0;
    let blocked_world = AuthorityInventory::Blocked(crate::BlockedReason::NoReconciler);
    let report = run(
        &recorded,
        &blocked_world,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: Some(measured_attempt(MEMBER, SHUT, binding())),
            live: Box::new(live_drifted()),
            current: binding(),
        },
    );
    assert_eq!(report.verdict, Verdict::Fail);
}

#[test]
fn a_blocked_confinement_is_not_masked_by_a_clean_inventory() {
    // The two axes are kept apart on the way in and on the way out.
    let (recorded, inventory) = clean_world();
    let report = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: None,
            live: Box::new(live_clean()),
            current: binding(),
        },
    );
    assert_eq!(report.verdict, Verdict::Blocked, "a clean inventory masked a blocked confinement");
}

#[test]
fn the_report_renders_history_and_live_state_separately() {
    let (recorded, inventory) = clean_world();
    let text = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: Some(measured_attempt(MEMBER, SHUT, binding())),
            live: Box::new(live_clean()),
            current: binding(),
        },
    )
    .render();
    assert!(text.contains("confinement"), "{text}");
    assert!(text.contains("history:"), "the stored half must be labelled: {text}");
    assert!(text.contains("live:"), "the current half must be labelled: {text}");
}

#[test]
fn a_blocked_report_always_names_the_remedy() {
    // A dead end is worse than a refusal. Every BLOCKED shape points at the one idempotent command
    // that can append the missing baseline.
    let (recorded, inventory) = clean_world();
    let text = run(
        &recorded,
        &inventory,
        &ConfinementAxis::Measured {
            ledger: Ledger::Present,
            latest: None,
            live: Box::new(live_clean()),
            current: binding(),
        },
    )
    .confinement
    .lines
    .join("\n");
    assert!(text.contains("lifecycle confine"), "{text}");
}
