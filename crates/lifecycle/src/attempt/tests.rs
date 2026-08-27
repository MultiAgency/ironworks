//! Attempts are facts; judgments are derived. These assert both halves.

use super::*;
use crate::confinement::{ConfinementPolicy, ToolState};
use crate::runtime::{RuntimeTenantId, RuntimeUserId};

/// No row reported `locked`. The default for a fixture that is not exercising the locked set:
/// `undisableable` unions this with `UNDISABLEABLE`, so the two rows the runtime refuses are still
/// skipped and these tests keep asserting exactly what they asserted before.
fn unlocked() -> std::collections::BTreeSet<String> {
    std::collections::BTreeSet::new()
}

fn at() -> ObservedAt {
    ObservedAt::parse("2026-09-02T12:34:56+00:00").expect("timestamp")
}

fn subject(user: &str) -> RuntimeSubject {
    RuntimeSubject::new(
        RuntimeTenantId::parse("acme").expect("tenant"),
        RuntimeUserId::parse(user).expect("user"),
    )
}

fn bound() -> BoundSubject {
    BoundSubject::bind(OpaqueId::new("user_7f3a"), subject("user_7f3a")).expect("binds")
}

/// A service definition built here rather than loaded, so these tests do not reach into `services`
/// merely to name a policy binding. Only the id and version are read out of it.
fn definition() -> ironworks_domain::ServiceDefinition {
    use ironworks_domain::composition::{Composition, PartRef};
    ironworks_domain::ServiceDefinition::new(
        ironworks_domain::ServiceId::parse("account-analysis").expect("id"),
        1,
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

fn binding() -> PolicyBinding {
    PolicyBinding::of(&definition(), &ConfinementPolicy::read_only_analyst())
}

fn catalogue(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

// ── the durable blocked projection ──────────────────────────────────────────────────────────

#[test]
fn the_durable_blocked_projection_drops_every_string_payload() {
    // The point of the projection. Each of these `BlockedReason`s carries text that an audit found
    // can hold a response body, a credential handle, or a URL with userinfo; none of it may reach
    // the store, so the projected value has nowhere to put it.
    let sentinel = "SENTINEL-tok_do_not_persist";
    let carriers = [
        BlockedReason::Unreachable { system: sentinel.into() },
        BlockedReason::Malformed { detail: sentinel.into() },
        BlockedReason::NoCredential { system: sentinel.into() },
    ];
    for reason in &carriers {
        let projected = Blocked::from(reason);
        assert!(
            !format!("{projected:?}{projected}{}", projected.tag()).contains(sentinel),
            "{reason:?} leaked its payload through the projection",
        );
    }
    // The positive control: the projection is not vacuous — it does classify these three, and it
    // classifies them apart from one another.
    assert_eq!(Blocked::from(&carriers[0]), Blocked::RuntimeUnreachable);
    assert_eq!(Blocked::from(&carriers[1]), Blocked::UnreadableAnswer);
    assert_eq!(Blocked::from(&carriers[2]), Blocked::NoCredential);
}

#[test]
fn every_blocked_reason_projects_and_every_projection_round_trips() {
    // Totality, asserted over the whole source enum rather than the three interesting variants.
    for reason in [
        BlockedReason::Unreachable { system: "x".into() },
        BlockedReason::Malformed { detail: "x".into() },
        BlockedReason::NoCredential { system: "x".into() },
        BlockedReason::CredentialRejected,
        BlockedReason::NotObserved,
        BlockedReason::NoRecordedIdentity,
        BlockedReason::NothingToReconcile,
        BlockedReason::NoReconciler,
    ] {
        let projected = Blocked::from(&reason);
        assert_eq!(Blocked::from_tag(projected.tag()), Some(projected), "{reason:?}");
    }
    for blocked in Blocked::ALL {
        assert_eq!(Blocked::from_tag(blocked.tag()), Some(blocked));
    }
    assert_eq!(Blocked::from_tag("something_this_build_never_wrote"), None);
}

#[test]
fn every_attempt_variant_and_drift_kind_round_trips() {
    for variant in AttemptVariant::ALL {
        assert_eq!(AttemptVariant::from_tag(variant.tag()), Some(variant));
    }
    assert_eq!(AttemptVariant::from_tag("measured_ish"), None);

    // Drift kinds are checked through a value of each variant, so a new variant with a forgotten
    // tag fails the exhaustive match in `kind_tag` rather than passing here vacuously.
    let all = [
        Drift::AuthorityChangingReachable {
            tool: "builtin.http".into(),
            state: ToolState::AlwaysAllow,
        },
        Drift::Reachable { tool: "builtin.time".into(), state: ToolState::AskEachTime },
        Drift::Unrecognized { tool: "builtin.x".into(), raw: "new_state".into() },
        Drift::AutoApprove { observed: true, expected: false },
        Drift::AllowedToolAbsent { tool: "builtin.shell".into() },
    ];
    let tags: Vec<&str> = all.iter().map(Drift::kind_tag).collect();
    assert_eq!(tags.len(), Drift::KIND_TAGS.len(), "a drift variant has no tag exercised here");
    for tag in &tags {
        assert!(Drift::KIND_TAGS.contains(tag), "{tag} is not in KIND_TAGS");
    }
}

// ── construction boundaries ─────────────────────────────────────────────────────────────────

#[test]
fn a_measurement_can_only_come_from_running_one_assessment() {
    // `from_assessment` takes raw inputs, not a finished facts/drift pair — so the drift in a
    // measurement provably belongs to the catalogue beside it. This asserts the pairing is real:
    // an open catalogue produces drift naming the tool that is open.
    let policy = ConfinementPolicy::read_only_analyst();
    let open = catalogue(&[("builtin.http", "always_allow"), ("builtin.time", "disabled")]);
    let m = Measurement::from_assessment(&policy, &bound(), &open, &unlocked(), false, &at());
    assert_eq!(m.catalogued(), 2);
    assert_eq!(m.reachable(), &BTreeSet::from(["builtin.http".to_owned()]));
    assert!(!m.is_clean());
    assert!(
        m.drift()
            .iter()
            .any(|d| matches!(d, Drift::Reachable { tool, .. } if tool == "builtin.http")),
        "{:?}",
        m.drift()
    );
}

#[test]
fn a_confined_catalogue_measures_clean() {
    // The positive control for the test above: without it, a `from_assessment` that always
    // produced drift would satisfy every assertion there.
    let policy = ConfinementPolicy::read_only_analyst();
    let shut = catalogue(&[("builtin.http", "disabled"), ("builtin.time", "disabled")]);
    let m = Measurement::from_assessment(&policy, &bound(), &shut, &unlocked(), false, &at());
    assert!(m.is_clean(), "{:?}", m.drift());
    assert_eq!(m.catalogued(), 2);
    assert!(m.reachable().is_empty());
}

#[test]
fn a_mismatch_attempt_can_only_be_built_from_a_failed_binding() {
    // `subject_mismatch` takes the error `bind` returns, so "recorded as a mismatch" and "the
    // comparison actually failed" are the same fact rather than two that must agree.
    let mismatch = BoundSubject::bind(OpaqueId::new("user_7f3a"), subject("reborn-cli"))
        .expect_err("different ids must not bind");
    let attempt = ConfinementAttempt::subject_mismatch(mismatch, at());
    assert_eq!(attempt.variant(), AttemptVariant::SubjectMismatch);
    assert_eq!(attempt.expected().as_str(), "user_7f3a");
    assert_eq!(attempt.authenticated().expect("subject").user().as_str(), "reborn-cli");
    // It carries no policy: nothing was measured against one, and recording one would imply a
    // comparison that did not happen.
    assert!(attempt.policy().is_none());
    assert!(attempt.measurement().is_none());
}

#[test]
fn a_binding_blocked_attempt_records_no_subject_at_all() {
    let attempt = ConfinementAttempt::binding_blocked(
        OpaqueId::new("user_7f3a"),
        at(),
        Blocked::RuntimeUnreachable,
    );
    assert_eq!(attempt.variant(), AttemptVariant::BindingBlocked);
    assert!(attempt.authenticated().is_none(), "no subject was established, so none is recorded");
    assert!(attempt.policy().is_none());
}

// ── one canonical home per fact ─────────────────────────────────────────────────────────────

#[test]
fn the_fact_view_is_derived_and_never_a_second_stored_copy() {
    let policy = ConfinementPolicy::read_only_analyst();
    let m = Measurement::from_assessment(
        &policy,
        &bound(),
        &catalogue(&[("builtin.http", "disabled")]),
        &unlocked(),
        false,
        &at(),
    );
    let attempt = ConfinementAttempt::measured(bound(), binding(), at(), m);
    let facts = attempt.facts().expect("a measured attempt has facts");
    // Each of these comes from the one place that owns it: the attempt's expected member, the
    // attempt's timestamp, and the policy binding's fingerprint.
    assert_eq!(&facts.member, attempt.expected());
    assert_eq!(facts.observed_at, attempt.at().as_str());
    assert_eq!(&facts.policy_fingerprint, binding().fingerprint());
    assert_eq!(facts.catalogued, 1);
}

// ── derived judgments ───────────────────────────────────────────────────────────────────────

#[test]
fn historical_readings_are_derived_from_the_facts() {
    let policy = ConfinementPolicy::read_only_analyst();
    let clean = Measurement::from_assessment(
        &policy,
        &bound(),
        &catalogue(&[("builtin.http", "disabled")]),
        &unlocked(),
        false,
        &at(),
    );
    let dirty = Measurement::from_assessment(
        &policy,
        &bound(),
        &catalogue(&[("builtin.http", "always_allow")]),
        &unlocked(),
        false,
        &at(),
    );

    let clean_attempt = ConfinementAttempt::measured(bound(), binding(), at(), clean);
    assert!(matches!(clean_attempt.historical(), Historical::CleanAt { .. }));

    let drifted = ConfinementAttempt::measured(bound(), binding(), at(), dirty);
    assert!(matches!(drifted.historical(), Historical::DriftedAt { .. }));

    let blocked = ConfinementAttempt::catalogue_blocked(
        bound(),
        binding(),
        at(),
        Blocked::CredentialRejected,
    );
    assert!(matches!(
        blocked.historical(),
        Historical::Unmeasured { reason: Blocked::CredentialRejected }
    ));

    let unbindable = ConfinementAttempt::binding_blocked(
        OpaqueId::new("user_7f3a"),
        at(),
        Blocked::RuntimeUnreachable,
    );
    assert!(matches!(unbindable.historical(), Historical::Unmeasured { .. }));

    let mismatch = ConfinementAttempt::subject_mismatch(
        BoundSubject::bind(OpaqueId::new("user_7f3a"), subject("reborn-cli")).expect_err("differs"),
        at(),
    );
    assert!(matches!(mismatch.historical(), Historical::Misbound { .. }));
}

#[test]
fn a_clean_measurement_under_another_policy_is_not_a_baseline() {
    // The rule that makes the fingerprint load-bearing. Without it, editing a service definition
    // would silently keep certifying a member confined to the policy it replaced.
    let policy = ConfinementPolicy::read_only_analyst();
    let clean = Measurement::from_assessment(
        &policy,
        &bound(),
        &catalogue(&[("builtin.http", "disabled")]),
        &unlocked(),
        false,
        &at(),
    );
    let attempt = ConfinementAttempt::measured(bound(), binding(), at(), clean);
    let historical = attempt.historical();

    // Positive control first: it IS a baseline for the policy it was taken against.
    assert!(historical.is_baseline_for(&binding()));

    let wider = ConfinementPolicy::sandbox_workspace();
    let other = PolicyBinding::of(&definition(), &wider);
    assert_ne!(binding().fingerprint(), other.fingerprint(), "the fixture policies must differ");
    assert!(!historical.is_baseline_for(&other));
}

#[test]
fn only_a_clean_measurement_is_ever_a_baseline() {
    let policy = ConfinementPolicy::read_only_analyst();
    let dirty = Measurement::from_assessment(
        &policy,
        &bound(),
        &catalogue(&[("builtin.http", "always_allow")]),
        &unlocked(),
        false,
        &at(),
    );
    let drifted = ConfinementAttempt::measured(bound(), binding(), at(), dirty).historical();
    assert!(!drifted.is_baseline_for(&binding()));

    let blocked =
        ConfinementAttempt::catalogue_blocked(bound(), binding(), at(), Blocked::UnreadableAnswer)
            .historical();
    assert!(!blocked.is_baseline_for(&binding()));

    let mismatch = ConfinementAttempt::subject_mismatch(
        BoundSubject::bind(OpaqueId::new("user_7f3a"), subject("reborn-cli")).expect_err("differs"),
        at(),
    )
    .historical();
    assert!(!mismatch.is_baseline_for(&binding()));
}
