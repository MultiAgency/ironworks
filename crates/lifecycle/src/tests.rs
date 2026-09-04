//! Crash boundaries and reconciliation semantics.
//!
//! The two that matter most are **P2** and **P4**, the windows where authority exists and nothing
//! local records it. Every absence-style assertion here carries a positive control, because an
//! empty store satisfies "nothing was created twice" vacuously — the same reason an empty database
//! passes every cross-org isolation check.

use super::*;
use ironworks_domain::NonEmptyReason;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// No row reported `locked`. The default for a fixture that is not exercising the locked set:
/// `undisableable` unions this with `UNDISABLEABLE`, so the two rows the runtime refuses are still
/// skipped and these tests keep asserting exactly what they asserted before.
fn unlocked() -> std::collections::BTreeSet<String> {
    std::collections::BTreeSet::new()
}

const NOW: &str = "2026-08-27T12:00:00+00:00";

fn spec(slug: &str) -> InstanceSpec {
    InstanceSpec {
        slug: Slug::parse(slug).unwrap(),
        org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
        service: ironworks_domain::ServiceId::parse("account-analysis").unwrap(),
        channel: ironworks_domain::ChannelRef::new("-100900001"),
    }
}

/// A reconciler that says exactly what a test tells it to, and nothing about anything it was not
/// asked about — because silence is one of the three answers.
#[derive(Default)]
struct Fixture(Vec<AuthorityFact>);

impl Fixture {
    fn present(mut self, authority: Authority, id: &str) -> Self {
        self.0.push(AuthorityFact {
            authority,
            presence: Presence::Present { id: OpaqueId::new(id) },
        });
        self
    }
    fn absent(mut self, authority: Authority) -> Self {
        self.0.push(AuthorityFact { authority, presence: Presence::Absent });
        self
    }
    fn unverifiable(mut self, authority: Authority, reason: BlockedReason) -> Self {
        self.0.push(AuthorityFact { authority, presence: Presence::Unverifiable(reason) });
        self
    }
    /// Everything measured absent. The **positive control** for every adoption test: it proves the
    /// planner does run the stage when nothing is out there, so an `AdoptExisting` elsewhere is a
    /// consequence of the fixture rather than of the planner ignoring it.
    fn nothing_exists() -> AuthorityInventory {
        let mut f = Fixture::default();
        for a in Authority::ALL {
            f = f.absent(a);
        }
        f.observe(&spec("acme"))
    }
}

impl Reconcile for Fixture {
    fn observe(&self, _spec: &InstanceSpec) -> AuthorityInventory {
        AuthorityInventory::Reconciled(self.0.clone())
    }
}

/// The identifier a stage's authority would be given, so tests record what provisioning would.
fn created_for(stage: Stage) -> Option<OpaqueId> {
    Authority::ALL
        .iter()
        .copied()
        .find(|a| a.created_at() == stage)
        .map(|a| OpaqueId::new(format!("{}-1", a.tag())))
}

/// What IronWorks recorded: how far it got, and which identifiers it created.
fn recorded(progress: ProvisionProgress, ids: &[(Authority, &str)]) -> Recorded {
    Recorded::from_created(progress, ids.iter().map(|(a, id)| (*a, OpaqueId::new(*id))))
}

/// Every authority recorded under the identifier `Fixture::present` uses by default.
fn recorded_all(progress: ProvisionProgress) -> Recorded {
    Recorded::from_created(
        progress,
        Authority::ALL.iter().map(|a| (*a, OpaqueId::new(format!("{}-1", a.tag())))),
    )
}

fn store() -> (Dir, LifecycleStore) {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    (dir, store)
}

/// Declare an instance and walk it to `through`, doing the work rather than adopting it.
fn walk_to(store: &LifecycleStore, slug: &str, through: Stage) -> Slug {
    let s = spec(slug);
    store.declare(&s, NOW).expect("declare");
    for stage in Stage::ALL {
        if stage > through {
            break;
        }
        if stage == Stage::MemberConfined {
            certify_stage(store, &s.slug, NOW);
        } else {
            store.reach(&s.slug, stage, false, created_for(stage).as_ref(), NOW).expect("reach");
        }
    }
    s.slug
}

fn certify_stage(store: &LifecycleStore, slug: &Slug, now: &str) {
    use crate::attempt::{ConfinementAttempt, ConfinementEvidence, Measurement};
    use crate::confinement::{ConfinementPolicy, PolicyBinding};
    use crate::runtime::{
        BoundSubject, ObservedAt, RuntimeSubject, RuntimeTenantId, RuntimeUserId,
    };
    use ironworks_domain::composition::{Composition, PartRef};

    let member = created_for(Stage::MemberMinted).expect("member id");
    let subject = RuntimeSubject::new(
        RuntimeTenantId::parse("runtime-tenant").unwrap(),
        RuntimeUserId::parse(member.as_str()).unwrap(),
    );
    let bound = BoundSubject::bind(member, subject).expect("same member");
    let definition = ironworks_domain::ServiceDefinition::new(
        ironworks_domain::ServiceId::parse("account-analysis").unwrap(),
        1,
        "test service",
        Composition::new(vec![PartRef::new("persona")], "guidance", PartRef::new("task")).unwrap(),
        ironworks_domain::CapabilityProfile::READ_ONLY_RECORDS,
        None,
    )
    .unwrap();
    let policy = ConfinementPolicy::read_only_analyst();
    let binding = PolicyBinding::of(&definition, &policy);
    let at = ObservedAt::parse(now).unwrap();
    let states = [("builtin.http".to_owned(), "disabled".to_owned())].into_iter().collect();
    let measurement = Measurement::from_assessment(
        &policy,
        &bound,
        &states,
        &std::collections::BTreeSet::new(),
        false,
        &at,
    );
    let attempt = ConfinementAttempt::measured(bound, binding.clone(), at, measurement);
    let evidence = ConfinementEvidence::from_attempt(attempt, &binding).expect("clean evidence");
    store.certify_confinement(slug, &evidence).expect("certify");
}

// ── P2: authority exists, the journal is silent ─────────────────────────────────────────────

/// **P2.** A process died after registering an org credential and before writing anything down.
/// The journal is empty; the credential authenticates. Registering again mints a *second* live
/// credential and leaves the first working — the reference calls this "credential accumulation
/// manufactured by the tool meant to prevent it".
#[test]
fn p2_an_org_credential_that_exists_is_adopted_not_registered_again() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::PreflightPassed };
    let world =
        Fixture::default().present(Authority::OrgRegistration, "acme-sales").observe(&spec("acme"));

    assert_eq!(
        plan(&progress, &world),
        NextAction::AdoptExisting {
            stage: Stage::OrgRegistered,
            authority: Authority::OrgRegistration,
            id: OpaqueId::new("acme-sales"),
        },
        "an empty journal was read as evidence that no credential exists"
    );

    // Positive control: with the same progress and nothing out there, the stage RUNS. Without
    // this, the assertion above would also pass for a planner that adopted unconditionally.
    assert_eq!(plan(&progress, &Fixture::nothing_exists()), NextAction::Run(Stage::OrgRegistered));
}

/// **P4.** The same window one stage later, and the more expensive one: a second sealed member is
/// a second live bearer token that nothing will revoke for up to a year.
#[test]
fn p4_a_member_that_exists_is_adopted_not_minted_again() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::DataInitializationSettled };
    let world = Fixture::default().present(Authority::Member, "user_7f3a").observe(&spec("acme"));

    assert_eq!(
        plan(&progress, &world),
        NextAction::AdoptExisting {
            stage: Stage::MemberMinted,
            authority: Authority::Member,
            id: OpaqueId::new("user_7f3a"),
        }
    );
    assert_eq!(
        plan(&progress, &Fixture::nothing_exists()),
        NextAction::Run(Stage::MemberMinted),
        "positive control: nothing out there means mint"
    );
}

/// Adoption is recorded as adoption. Months later, "did we create this?" has an answer.
#[test]
fn an_adopted_stage_is_distinguishable_from_one_we_performed() {
    let (_d, store) = store();
    let s = spec("acme");
    store.declare(&s, NOW).unwrap();
    store
        .reach(
            &s.slug,
            Stage::PreflightPassed,
            false,
            created_for(Stage::PreflightPassed).as_ref(),
            NOW,
        )
        .unwrap();
    store
        .reach(&s.slug, Stage::OrgRegistered, true, created_for(Stage::OrgRegistered).as_ref(), NOW)
        .unwrap();

    let history = store.history(&s.slug).unwrap();
    assert!(!history[0].2, "preflight was performed");
    assert!(history[1].2, "the org registration was adopted, not created");
}

// ── unverifiable is neither ─────────────────────────────────────────────────────────────────

/// The answer that must never be smoothed into one of the other two. Reading it as absent mints a
/// duplicate; reading it as present skips a step that never ran.
#[test]
fn an_unverifiable_reconciliation_blocks_rather_than_guessing() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::PreflightPassed };
    let reason = BlockedReason::Unreachable { system: "account service".into() };
    let world = Fixture::default()
        .unverifiable(Authority::OrgRegistration, reason.clone())
        .observe(&spec("acme"));

    assert_eq!(
        plan(&progress, &world),
        NextAction::Blocked { reason, about: Some(Authority::OrgRegistration) }
    );
}

/// Silence is not absence. A reconciler that never mentions an authority has told us nothing about
/// it — the failure mode of every "we checked and found nothing" report that never checked.
#[test]
fn an_authority_never_mentioned_is_unverifiable_not_absent() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::PreflightPassed };
    let says_nothing = AuthorityInventory::Reconciled(Vec::new());

    assert_eq!(
        plan(&progress, &says_nothing),
        NextAction::Blocked {
            reason: BlockedReason::NotObserved,
            about: Some(Authority::OrgRegistration)
        },
        "an empty inventory was read as an inventory of nothing"
    );
    assert_eq!(
        AuthorityInventory::Reconciled(Vec::new()).presence(Authority::Member),
        Presence::Unverifiable(BlockedReason::NotObserved)
    );
}

/// Reconciliation that could not run at all blocks everything, including authorities a stale
/// inventory might otherwise have answered for.
#[test]
fn a_blocked_reconciliation_blocks_every_authority() {
    let reason = BlockedReason::NoCredential { system: "ironclaw admin".into() };
    let blocked = AuthorityInventory::Blocked(reason.clone());
    let progress = ProvisionProgress::Provisioning { reached: Stage::PreflightPassed };

    assert_eq!(plan(&progress, &blocked), NextAction::Blocked { reason, about: None });
    for a in Authority::ALL {
        assert!(!blocked.presence(a).is_measured(), "{a:?} was treated as measured");
    }
}

// ── ordering ────────────────────────────────────────────────────────────────────────────────

/// Minted but not confined cannot stage. An unconfined member is a member with an unbounded tool
/// surface, and staging it makes it routable.
#[test]
fn a_minted_member_that_is_not_confined_cannot_stage() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::MemberMinted };
    assert_eq!(
        plan(&progress, &Fixture::nothing_exists()),
        NextAction::Run(Stage::MemberConfined),
        "the next action skipped confinement"
    );

    // And the store refuses to record the skip, so a caller that ignores the planner cannot
    // write the state anyway. Two independent refusals for one invariant, because this is the one
    // whose failure makes an unconfined member routable.
    let (_d, store) = store();
    let slug = walk_to(&store, "acme", Stage::MemberMinted);
    let err = store.reach(&slug, Stage::Staged, false, created_for(Stage::Staged).as_ref(), NOW);
    assert!(
        matches!(&err, Err(LifecycleError::StageSkipped { predecessor, .. })
                 if predecessor == "member_confined"),
        "{err:?}"
    );

    // Positive control: confine first and staging is recorded, so the refusal above is about the
    // gap rather than about `reach` refusing everything.
    certify_stage(&store, &slug, NOW);
    store.reach(&slug, Stage::Staged, false, created_for(Stage::Staged).as_ref(), NOW).unwrap();
    assert!(store.progress(&slug).unwrap().has_reached(Stage::Staged));
}

#[test]
fn a_presence_claim_cannot_adopt_confinement_without_certifying_evidence() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::MemberMinted };
    let world = Fixture::default()
        .present(Authority::MemberConfinement, "a-policy-fingerprint")
        .observe(&spec("acme"));
    assert_eq!(
        plan(&progress, &world),
        NextAction::Run(Stage::MemberConfined),
        "the planner prescribed generic adoption for an evidence-bearing stage"
    );
}

/// Resume restarts at the first stage not reached, and redoes nothing before it.
#[test]
fn every_resume_point_restarts_at_the_first_not_reached_stage() {
    let world = Fixture::nothing_exists();
    // Nothing recorded: start at the beginning.
    assert_eq!(
        plan(&ProvisionProgress::NotStarted, &world),
        NextAction::Run(Stage::PreflightPassed)
    );
    // From each stage, the next action is exactly its successor.
    for stage in Stage::ALL {
        let progress = ProvisionProgress::Provisioning { reached: stage };
        let expected = match stage.next() {
            Some(next) => NextAction::Run(next),
            None => NextAction::Activate,
        };
        assert_eq!(plan(&progress, &world), expected, "resume from {stage} was wrong");
    }
}

#[test]
fn smoke_passed_but_not_activated_means_activate_next() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::SmokePassed };
    assert_eq!(plan(&progress, &Fixture::nothing_exists()), NextAction::Activate);
}

#[test]
fn an_activated_instance_has_nothing_left_to_do() {
    let smoke = SmokeEvidence::from_outcome(
        &SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() },
        NOW,
    )
    .unwrap();
    assert_eq!(
        plan(&ProvisionProgress::Activated { smoke }, &Fixture::nothing_exists()),
        NextAction::Nothing
    );
}

// ── activation requires evidence ────────────────────────────────────────────────────────────

/// There is no constructor that yields evidence from a failed or blocked smoke, so an activation
/// nothing measured cannot be expressed.
#[test]
fn smoke_evidence_exists_only_for_a_passing_run() {
    let passed =
        SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() };
    assert!(SmokeEvidence::from_outcome(&passed, NOW).is_some());

    assert!(
        SmokeEvidence::from_outcome(&SmokeOutcome::Failed { leg: "isolation".into() }, NOW)
            .is_none()
    );
    assert!(
        SmokeEvidence::from_outcome(
            &SmokeOutcome::Blocked(BlockedReason::Unreachable { system: "runtime".into() }),
            NOW
        )
        .is_none(),
        "a smoke that could not run produced evidence that it passed"
    );
}

#[test]
fn activation_refuses_before_every_stage_is_reached() {
    let (_d, store) = store();
    let slug = walk_to(&store, "acme", Stage::Staged);
    let smoke = SmokeEvidence::from_outcome(
        &SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() },
        NOW,
    )
    .unwrap();

    let err = store.activate(&slug, &smoke, NOW);
    assert!(matches!(err, Err(LifecycleError::NotReadyToActivate { .. })), "{err:?}");

    // Positive control: the same call succeeds once the last stage is recorded.
    store
        .reach(&slug, Stage::SmokePassed, false, created_for(Stage::SmokePassed).as_ref(), NOW)
        .unwrap();
    store.activate(&slug, &smoke, NOW).unwrap();
    assert!(matches!(store.progress(&slug).unwrap(), ProvisionProgress::Activated { .. }));
}

// ── routable is not serving ─────────────────────────────────────────────────────────────────

/// Activation makes an instance routable. Whether a process has loaded it is a different fact on a
/// different axis, and `Unknown` is the honest answer until something asks.
#[test]
fn activated_is_not_serving_and_serving_may_stay_unknown() {
    let (_d, store) = store();
    let slug = walk_to(&store, "acme", Stage::SmokePassed);
    let smoke = SmokeEvidence::from_outcome(
        &SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() },
        NOW,
    )
    .unwrap();
    store.activate(&slug, &smoke, NOW).unwrap();

    assert!(matches!(store.progress(&slug).unwrap(), ProvisionProgress::Activated { .. }));
    assert_eq!(
        store.serving(&slug).unwrap(),
        ServingObservation::Unknown,
        "activation was read as evidence that something is serving it"
    );

    // Positive control: an observation does change it, so Unknown above is a real absence of
    // evidence rather than a field nothing writes.
    store.observe_serving(&slug, &ServingObservation::Serving { observed_at: NOW.into() }).unwrap();
    assert_eq!(
        store.serving(&slug).unwrap(),
        ServingObservation::Serving { observed_at: NOW.into() }
    );
}

// ── tenant data ─────────────────────────────────────────────────────────────────────────────

/// An org with no records is a valid outcome, not a skipped step. The reference journals the stage
/// in its own `else` branch: "the org starts empty (valid: org existence is row scoping)".
#[test]
fn data_initialization_settles_with_no_rows_and_the_stage_still_advances() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::OrgRegistered };
    // The store reports no tenant data. That is the settled empty case, not a failure.
    let world = Fixture::default().absent(Authority::TenantData).observe(&spec("acme"));
    assert_eq!(plan(&progress, &world), NextAction::Run(Stage::DataInitializationSettled));

    let (_d, store) = store();
    let slug = walk_to(&store, "acme", Stage::DataInitializationSettled);
    assert!(
        store.progress(&slug).unwrap().has_reached(Stage::DataInitializationSettled),
        "an empty org could not settle its data stage"
    );
}

/// A failed run never schedules deletion of a tenant's business records. Every other authority may
/// be compensated; this one is reported and left for an explicit, confirmed deprovision.
#[test]
fn failed_provisioning_never_compensates_tenant_data() {
    assert!(!Authority::TenantData.reversible_by_compensation());
    for a in Authority::ALL {
        if a != Authority::TenantData {
            assert!(a.reversible_by_compensation(), "{a:?} should be compensable");
        }
    }
}

// ── what exists, and whether we account for it ──────────────────────────────────────────────

/// The 41-unknown-members case. Authority that exists which IronWorks never recorded creating is
/// reported as such — not adopted silently, and not invisible.
#[test]
fn authority_we_did_not_create_is_reported_as_unmanaged() {
    let progress = ProvisionProgress::Provisioning { reached: Stage::OrgRegistered };
    let world = Fixture::default()
        .present(Authority::OrgRegistration, "acme-sales")
        .present(Authority::Member, "user_stray")
        .absent(Authority::TenantData)
        .unverifiable(Authority::RegistryEntry, BlockedReason::NotObserved)
        .observe(&spec("acme"));

    let held = holdings(
        &recorded(progress.clone(), &[(Authority::OrgRegistration, "acme-sales")]),
        &world,
    );
    let by = |a: Authority| held.iter().find(|h| h.authority == a).unwrap().custody.clone();

    assert_eq!(by(Authority::OrgRegistration), Custody::Managed, "we recorded creating it");
    assert_eq!(
        by(Authority::Member),
        Custody::KnownUnmanaged,
        "a member exists that our journal never accounted for"
    );
    assert_eq!(by(Authority::TenantData), Custody::Absent);
    assert!(matches!(by(Authority::RegistryEntry), Custody::Unverifiable(_)));
    assert!(
        matches!(by(Authority::MemberConfinement), Custody::Unverifiable(_)),
        "never mentioned"
    );
}

// ── storage discipline ──────────────────────────────────────────────────────────────────────

/// The store outlives a crash, so anything in it outlives a crash. Asserted against the file's
/// bytes rather than by reading the code.
#[test]
fn the_store_holds_no_credential_material() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    {
        let store = LifecycleStore::open(&path).unwrap();
        let s = spec("acme");
        store.declare(&s, NOW).unwrap();
        store
            .reach(
                &s.slug,
                Stage::PreflightPassed,
                false,
                created_for(Stage::PreflightPassed).as_ref(),
                NOW,
            )
            .unwrap();
        // The only value a caller can put in that came from a foreign system is an opaque id, and
        // there is no method that accepts anything else.
        store
            .reach(
                &s.slug,
                Stage::OrgRegistered,
                true,
                created_for(Stage::OrgRegistered).as_ref(),
                NOW,
            )
            .unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    let haystack = String::from_utf8_lossy(&bytes);
    for sentinel in ["SECRET-TOKEN-VALUE", "Bearer ", "sk-", "password"] {
        assert!(!haystack.contains(sentinel), "{sentinel} reached the lifecycle store");
    }
    // Positive control: what it DOES hold is identifiers, so the assertions above are not passing
    // against an empty file.
    assert!(haystack.contains("acme") && haystack.contains("org_registered"));
}

#[test]
fn read_only_inspection_has_no_side_effects() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let s = spec("acme");
    {
        let store = LifecycleStore::open(&path).unwrap();
        store.declare(&s, NOW).unwrap();
        store
            .reach(
                &s.slug,
                Stage::PreflightPassed,
                false,
                created_for(Stage::PreflightPassed).as_ref(),
                NOW,
            )
            .unwrap();
    }
    let ro = LifecycleStore::open_readonly(&path).unwrap();
    assert!(!ro.is_writable());
    // Reads work.
    assert_eq!(ro.progress(&s.slug).unwrap().reached(), Some(Stage::PreflightPassed));
    assert_eq!(ro.unfinished().unwrap(), vec![s.slug.clone()]);
    // Every write refuses.
    assert!(matches!(ro.declare(&s, NOW), Err(LifecycleError::ReadOnly)));
    assert!(matches!(
        ro.reach(&s.slug, Stage::OrgRegistered, false, None, NOW),
        Err(LifecycleError::ReadOnly)
    ));
    assert!(matches!(
        ro.observe_serving(&s.slug, &ServingObservation::Unknown),
        Err(LifecycleError::ReadOnly)
    ));
}

#[test]
fn a_store_from_another_version_refuses_to_open() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    {
        let store = LifecycleStore::open(&path).unwrap();
        let conn = store.conn.lock().unwrap();
        conn.execute("UPDATE lifecycle_meta SET value='99' WHERE key='schema_version'", [])
            .unwrap();
    }
    // Asserted arm by arm rather than with one `matches!`. The three ways this can go wrong are
    // not equally serious and a boolean cannot tell them apart: a store that *opens* is a
    // fail-closed guard that stopped closing, a store that refuses for another reason is a
    // different defect, and a version mismatch carrying the wrong numbers is bookkeeping. A
    // `matches!` collapses all three into the same `false`, and that is what it did — this test
    // failed once against `expected: 1` after `SCHEMA_VERSION` moved to 2, and read as though the
    // guard had broken.
    //
    // `expected` is compared against the constant for the same reason: writing the number twice
    // makes the next bump fail this test for no reason and teaches whoever sees it that the
    // failure is noise.
    for opened in [LifecycleStore::open(&path), LifecycleStore::open_readonly(&path)] {
        match opened {
            Err(LifecycleError::Version { found, expected }) => {
                assert_eq!(found, 99, "read the wrong stamped version out of lifecycle_meta");
                assert_eq!(
                    expected, SCHEMA_VERSION,
                    "the refusal names a version this build does not write"
                );
            }
            Ok(_) => panic!(
                "a store stamped schema_version=99 opened: the version guard has stopped refusing"
            ),
            Err(other) => panic!("refused, but not as a version mismatch: {other:?}"),
        }
    }
}

/// A declared instance with nothing reached is `NotStarted`, and an *undeclared* one is too — the
/// two are indistinguishable from progress alone, which is exactly why progress may never be read
/// as a statement about the world.
#[test]
fn not_started_says_nothing_about_what_exists() {
    let (_d, store) = store();
    let unknown = Slug::parse("never-heard-of").unwrap();
    assert_eq!(store.progress(&unknown).unwrap(), ProvisionProgress::NotStarted);
    assert_eq!(store.serving(&unknown).unwrap(), ServingObservation::Unknown);

    let world = Fixture::default()
        .present(Authority::Member, "user_stray")
        .observe(&spec("never-heard-of"));
    assert!(
        matches!(
            plan(&ProvisionProgress::NotStarted, &world),
            NextAction::Run(Stage::PreflightPassed)
        ),
        "preflight creates no authority, so it runs before any adoption question"
    );
    // But the inventory still reports the member, and reports it as unmanaged.
    let held = holdings(&Recorded::from_progress(ProvisionProgress::NotStarted), &world);
    let member = held.iter().find(|h| h.authority == Authority::Member).unwrap();
    assert_eq!(member.custody, Custody::KnownUnmanaged);
}

// ── a temporary directory, without a dependency ─────────────────────────────────────────────

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ironworks-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── inspection ──────────────────────────────────────────────────────────────────────────────

fn activated() -> ProvisionProgress {
    ProvisionProgress::Activated {
        smoke: SmokeEvidence::from_outcome(
            &SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() },
            NOW,
        )
        .unwrap(),
    }
}

/// Everything IronWorks recorded creating is measured present. Nothing else is out there, and
/// nothing was unmeasurable.
#[test]
fn everything_accounted_for_passes() {
    let mut f = Fixture::default();
    for a in Authority::ALL {
        f = f.present(a, &format!("{}-1", a.tag()));
    }
    let world = f.observe(&spec("acme"));
    let report = inspect(
        &spec("acme"),
        &recorded_all(activated()),
        &world,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );

    assert_eq!(report.verdict, Verdict::Pass, "{:#?}", report.findings);
    assert!(report.verdict.promotable());
    assert_eq!(report.verdict.exit_code(), 0);
    assert!(report.holdings.iter().all(|h| h.custody == Custody::Managed));
}

/// Authority nobody accounted for blocks. It is reported, not adopted and not removed — deciding
/// what a stray member IS belongs to an operator, and this slice deliberately has no verb for it.
#[test]
fn known_unmanaged_authority_blocks() {
    // Activated, so every authority is one we recorded creating — except the confinement, which
    // the fixture reports for a member we never minted.
    let progress = ProvisionProgress::Provisioning { reached: Stage::PreflightPassed };
    let world = Fixture::default()
        .present(Authority::Member, "user_stray")
        .absent(Authority::OrgRegistration)
        .absent(Authority::TenantData)
        .absent(Authority::MemberConfinement)
        .absent(Authority::RegistryEntry)
        .observe(&spec("acme"));

    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(progress),
        &world,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
    assert!(!report.verdict.promotable());
    assert_eq!(report.verdict.exit_code(), 3);
    assert!(
        report.findings.iter().any(|f| f.authority == Some(Authority::Member)
            && f.note.contains("no record of creating it")),
        "{:#?}",
        report.findings
    );

    // Positive control: the identical inventory with the stage recorded is MANAGED and passes, so
    // the block above is about custody rather than about the member existing at all.
    let managed = ProvisionProgress::Provisioning { reached: Stage::MemberMinted };
    let ok = inspect(
        &spec("acme"),
        &recorded(managed, &[(Authority::Member, "user_stray")]),
        &world,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert!(
        ok.holdings
            .iter()
            .any(|h| h.authority == Authority::Member && h.custody == Custody::Managed)
    );
}

/// An authority that could not be measured blocks, and says why.
#[test]
fn unverifiable_authority_blocks() {
    let reason = BlockedReason::Unreachable { system: "ironclaw admin".into() };
    let mut f = Fixture::default().unverifiable(Authority::Member, reason);
    for a in Authority::ALL {
        if a != Authority::Member {
            f = f.absent(a);
        }
    }
    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(ProvisionProgress::NotStarted),
        &f.observe(&spec("acme")),
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
    assert!(report.findings.iter().any(|f| f.note.contains("not absence")));
}

/// A reconciler that asked nobody produces BLOCKED, never PASS. This is the vacuous-green case:
/// an inspection with no way to observe the world must not certify anything clean.
#[test]
fn an_inspection_with_no_reconciler_is_blocked_not_passing() {
    let world = NoReconciler.observe(&spec("acme"));
    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(activated()),
        &world,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );

    assert_eq!(report.verdict, Verdict::Blocked);
    assert_eq!(report.holdings.len(), Authority::ALL.len(), "every authority is still reported");
    assert!(report.holdings.iter().all(|h| matches!(h.custody, Custody::Unverifiable(_))));
}

/// An empty inventory is not an inventory of nothing. Five authorities unmentioned is five
/// unverifiable answers, and a blocked verdict.
#[test]
fn an_empty_inventory_does_not_pass_vacuously() {
    let nothing_said = AuthorityInventory::Reconciled(Vec::new());
    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(activated()),
        &nothing_said,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked, "silence was read as a clean bill of health");

    // Positive control: an inventory that actually says 'absent' for everything, with nothing
    // recorded as created, has no disagreement to report.
    let mut f = Fixture::default();
    for a in Authority::ALL {
        f = f.absent(a);
    }
    let measured = inspect(
        &spec("acme"),
        &Recorded::from_progress(ProvisionProgress::NotStarted),
        &f.observe(&spec("acme")),
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(measured.verdict, Verdict::Pass, "{:#?}", measured.findings);
}

/// Our records say we created it; the world says it is not there. That is a determinate
/// contradiction, so FAIL rather than BLOCKED — and FAIL outranks BLOCKED in the same report.
#[test]
fn recorded_but_measured_absent_is_a_failure_not_a_block() {
    let mut f = Fixture::default().absent(Authority::OrgRegistration);
    for a in Authority::ALL {
        if a != Authority::OrgRegistration {
            f = f.unverifiable(a, BlockedReason::NotObserved);
        }
    }
    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(ProvisionProgress::Provisioning { reached: Stage::OrgRegistered }),
        &f.observe(&spec("acme")),
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Fail, "a measured contradiction was softened to BLOCKED");
    assert_eq!(report.verdict.exit_code(), 2);
    assert!(report.findings.iter().any(|f| f.note.contains("disagree")));
}

/// Activated means routable. It is reported as exactly that, and serving stays Unknown rather
/// than being inferred from it.
#[test]
fn activated_reports_routable_and_serving_unknown_not_serving_true() {
    let mut f = Fixture::default();
    for a in Authority::ALL {
        f = f.present(a, "id");
    }
    let ids: Vec<(Authority, &str)> = Authority::ALL.iter().map(|a| (*a, "id")).collect();
    let report = inspect(
        &spec("acme"),
        &recorded(activated(), &ids),
        &f.observe(&spec("acme")),
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );

    assert_eq!(report.serving, ServingObservation::Unknown);
    let text = report.render();
    assert!(text.contains("activated (routable)"), "{text}");
    assert!(text.contains("unknown — nothing has observed it"), "{text}");
    assert!(text.contains("Routable is not serving"), "{text}");
    // The finding is reported but does not block: activation's guarantee is routability.
    assert_eq!(report.verdict, Verdict::Pass);
}

/// ACCEPTED_RISK cannot be produced without a written reason, and nothing in this slice produces
/// it at all — unmanaged authority is never normalized into it.
#[test]
fn accepted_risk_carries_its_reason_and_is_never_reached_by_inference() {
    let reason =
        NonEmptyReason::parse("laptop self-test credential, tracked in ticket 41").unwrap();
    let v = Verdict::AcceptedRisk(reason);
    assert!(v.promotable());
    assert_eq!(v.exit_code(), 0);
    assert!(v.to_string().contains("ticket 41"), "the reason must survive into the report");
    assert_ne!(v.tag(), Verdict::Pass.tag(), "never indistinguishable from a clean pass");

    // No inspection produces it. Unmanaged authority blocks; it is not waived.
    let world = Fixture::default().present(Authority::Member, "user_stray").observe(&spec("acme"));
    let report = inspect(
        &spec("acme"),
        &Recorded::from_progress(ProvisionProgress::NotStarted),
        &world,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
}

/// The report is identifiers and prose. Nothing it reads from can hold a credential, so this
/// asserts the consequence rather than a redaction step.
#[test]
fn the_rendered_report_carries_no_secret_material() {
    let mut f = Fixture::default();
    for a in Authority::ALL {
        f = f.present(a, "user_7f3a");
    }
    let ids: Vec<(Authority, &str)> = Authority::ALL.iter().map(|a| (*a, "user_7f3a")).collect();
    let text = inspect(
        &spec("acme"),
        &recorded(activated(), &ids),
        &f.observe(&spec("acme")),
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    )
    .render();
    for sentinel in ["Bearer", "sk-", "token", "secret", "password"] {
        assert!(
            !text.to_lowercase().contains(&sentinel.to_lowercase()),
            "{sentinel} in the report"
        );
    }
    // Positive control: it does carry the identifiers an operator needs.
    assert!(text.contains("user_7f3a") && text.contains("acme-sales"));
}

/// Inspection reads. Asserted against the database file's bytes, because "the code does not call
/// a write method" is a claim about the code and this is a claim about the file.
#[test]
fn inspection_leaves_the_store_byte_identical() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let s = spec("acme");
    {
        let store = LifecycleStore::open(&path).unwrap();
        store.declare(&s, NOW).unwrap();
        store
            .reach(
                &s.slug,
                Stage::PreflightPassed,
                false,
                created_for(Stage::PreflightPassed).as_ref(),
                NOW,
            )
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();

    let ro = LifecycleStore::open_readonly(&path).unwrap();
    let report = inspect(
        &ro.spec(&s.slug).unwrap().unwrap(),
        &ro.created(&s.slug).unwrap(),
        &NoReconciler.observe(&s),
        &ro.serving(&s.slug).unwrap(),
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
    drop(ro);

    assert_eq!(std::fs::read(&path).unwrap(), before, "inspection changed the store");
}

/// **The stray-member case, which the first version of this crate got wrong.**
///
/// The stage was reached, so "did we get this far?" says yes — and the member out there is still
/// not the one we minted. Custody must be decided by identity, not by progress, or the state that
/// exists to catch 41 unaccounted members reports them all as managed.
#[test]
fn a_different_identifier_at_a_stage_we_reached_is_unmanaged() {
    let world = Fixture::default().present(Authority::Member, "user_stray").observe(&spec("acme"));
    let ours = recorded(
        ProvisionProgress::Provisioning { reached: Stage::MemberMinted },
        &[(Authority::Member, "user_7f3a")],
    );

    let held = holdings(&ours, &world);
    let member = held.iter().find(|h| h.authority == Authority::Member).unwrap();
    assert_eq!(
        member.custody,
        Custody::KnownUnmanaged,
        "a member we did not mint was reported as ours because the stage had been reached"
    );

    // Positive control: the same progress with the matching identifier IS ours.
    let matching = recorded(
        ProvisionProgress::Provisioning { reached: Stage::MemberMinted },
        &[(Authority::Member, "user_stray")],
    );
    let held = holdings(&matching, &world);
    let member = held.iter().find(|h| h.authority == Authority::Member).unwrap();
    assert_eq!(member.custody, Custody::Managed);
}

/// A stage reached with no identifier recorded cannot claim what it finds. Neither `Managed` (we
/// cannot show it is ours) nor `Absent` (it plainly exists) — the third answer.
#[test]
fn a_stage_reached_without_a_recorded_identifier_cannot_claim_what_exists() {
    let world = Fixture::default().present(Authority::Member, "user_7f3a").observe(&spec("acme"));
    let no_id =
        Recorded::from_progress(ProvisionProgress::Provisioning { reached: Stage::MemberMinted });
    let held = holdings(&no_id, &world);
    let member = held.iter().find(|h| h.authority == Authority::Member).unwrap();
    assert!(
        matches!(member.custody, Custody::Unverifiable(BlockedReason::NoRecordedIdentity)),
        "{:?}",
        member.custody
    );
}

/// The identifier IronWorks created survives a restart, which is the whole point of storing it.
#[test]
fn created_identifiers_are_durable_and_map_back_to_their_authority() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let s = spec("acme");
    {
        let store = LifecycleStore::open(&path).unwrap();
        store.declare(&s, NOW).unwrap();
        store.reach(&s.slug, Stage::PreflightPassed, false, None, NOW).unwrap();
        store
            .reach(&s.slug, Stage::OrgRegistered, false, Some(&OpaqueId::new("acme-sales")), NOW)
            .unwrap();
    }
    let store = LifecycleStore::open(&path).unwrap();
    let recorded = store.created(&s.slug).unwrap();
    assert_eq!(recorded.created_id(Authority::OrgRegistration), Some(&OpaqueId::new("acme-sales")));
    assert_eq!(recorded.created_id(Authority::Member), None, "we minted no member");
}

#[test]
fn adoption_records_measured_identities_with_provenance_in_one_instance() {
    let (_d, store) = store();
    let slug = Slug::parse("local-name").unwrap();
    let service = ironworks_domain::ServiceId::parse("account-analysis").unwrap();
    let channel = ironworks_domain::ChannelRef::new("-100900001");
    let scope = ironworks_domain::ScopeEvidence::from_record_store(
        ironworks_domain::OrgId::parse("measured-org").unwrap(),
        "http://records.invalid",
    );
    let subject = RuntimeSubject::new(
        RuntimeTenantId::parse("runtime-namespace").unwrap(),
        RuntimeUserId::parse("user_observed").unwrap(),
    );
    let evidence = AdoptionEvidence::observed(&scope, subject.clone());

    let spec = store.adopt(&slug, &service, &channel, &evidence, NOW).unwrap();
    assert_eq!(spec.org.as_str(), "measured-org");
    assert_eq!(
        store.progress(&slug).unwrap(),
        ProvisionProgress::Provisioning { reached: Stage::MemberMinted }
    );
    let history = store.history(&slug).unwrap();
    assert_eq!(
        history.iter().map(|(stage, _, adopted)| (*stage, *adopted)).collect::<Vec<_>>(),
        vec![
            (Stage::PreflightPassed, false),
            (Stage::OrgRegistered, true),
            (Stage::DataInitializationSettled, true),
            (Stage::MemberMinted, true),
        ]
    );

    let recorded = store.recorded(&slug).unwrap();
    let org = recorded.identity(Authority::OrgRegistration).unwrap();
    assert_eq!(org.id(), &OpaqueId::new("measured-org"));
    assert_eq!(org.provenance(), IdentityProvenance::Adopted);
    let member = recorded.identity(Authority::Member).unwrap();
    assert_eq!(member.id(), &OpaqueId::new("user_observed"));
    assert_eq!(member.provenance(), IdentityProvenance::Adopted);
    assert_eq!(member.runtime_subject(), Some(&subject));

    let created = store.created(&slug).unwrap();
    assert_eq!(created.created_id(Authority::OrgRegistration), None);
    assert_eq!(created.created_id(Authority::Member), None);
}

#[test]
fn adoption_refuses_an_existing_slug_instead_of_upserting_it() {
    let (_d, store) = store();
    let s = spec("acme");
    let scope =
        ironworks_domain::ScopeEvidence::from_record_store(s.org.clone(), "http://records.invalid");
    let evidence = AdoptionEvidence::observed(
        &scope,
        RuntimeSubject::new(
            RuntimeTenantId::parse("tenant-a").unwrap(),
            RuntimeUserId::parse("user-a").unwrap(),
        ),
    );
    store.adopt(&s.slug, &s.service, &s.channel, &evidence, NOW).expect("first adoption");
    let before = store.recorded(&s.slug).unwrap();

    let replacement = AdoptionEvidence::observed(
        &scope,
        RuntimeSubject::new(
            RuntimeTenantId::parse("tenant-b").unwrap(),
            RuntimeUserId::parse("user-b").unwrap(),
        ),
    );
    assert!(matches!(
        store.adopt(&s.slug, &s.service, &s.channel, &replacement, NOW),
        Err(LifecycleError::AlreadyExists { .. })
    ));
    assert_eq!(store.recorded(&s.slug).unwrap(), before);
}

#[test]
fn a_failed_adoption_transaction_leaves_no_partial_instance() {
    let (_d, store) = store();
    let s = spec("atomic");
    let scope =
        ironworks_domain::ScopeEvidence::from_record_store(s.org.clone(), "http://records.invalid");
    let evidence = AdoptionEvidence::observed(
        &scope,
        RuntimeSubject::new(
            RuntimeTenantId::parse("tenant-a").unwrap(),
            RuntimeUserId::parse("user-a").unwrap(),
        ),
    );
    // The v3 table rejects this timestamp after the instance and stage rows have been attempted.
    assert!(store.adopt(&s.slug, &s.service, &s.channel, &evidence, "bad").is_err());
    assert_eq!(store.spec(&s.slug).unwrap(), None);
    assert!(store.history(&s.slug).unwrap().is_empty());
}

/// A stage is never walked backwards.
///
/// The guard existed with nothing asserting it — the review's one weak guard. A later run finding
/// less than an earlier one is evidence about the observer, not about the world, and recording it
/// would let a resume redo work whose authority already exists.
#[test]
fn a_stage_is_never_walked_backwards() {
    let (_d, store) = store();
    let slug = walk_to(&store, "acme", Stage::MemberMinted);

    // Backwards by one, and all the way back, both refused.
    for earlier in [Stage::DataInitializationSettled, Stage::PreflightPassed] {
        let err = store.reach(&slug, earlier, false, created_for(earlier).as_ref(), NOW);
        assert!(
            matches!(&err, Err(LifecycleError::StageWouldRegress { recorded, attempted, .. })
                     if recorded == "member_minted" && attempted == earlier.tag()),
            "{earlier} was recorded over member_minted: {err:?}"
        );
    }
    // Re-recording the CURRENT stage is also a regression, not a no-op: it would append a second
    // history row claiming the work happened twice.
    assert!(matches!(
        store.reach(&slug, Stage::MemberMinted, false, None, NOW),
        Err(LifecycleError::StageWouldRegress { .. })
    ));

    assert!(matches!(
        store.reach(&slug, Stage::MemberConfined, false, None, NOW),
        Err(LifecycleError::ConfinementEvidenceRequired { .. })
    ));

    // Positive control: the immediate next stage still succeeds, so the refusals above are about
    // direction rather than about `reach` refusing everything once a stage is set.
    certify_stage(&store, &slug, NOW);
    assert!(store.progress(&slug).unwrap().has_reached(Stage::MemberConfined));
    assert_eq!(store.history(&slug).unwrap().len(), 5, "no refused attempt was recorded");
}

// ── confinement ─────────────────────────────────────────────────────────────────────────────

use crate::confinement::{
    AUTHORITY_CHANGING, Confinement, ConfinementPolicy, Drift, ToolState, assess,
};
use std::collections::BTreeMap;

fn member() -> OpaqueId {
    OpaqueId::new("user_7f3a")
}

/// A **sample** of the 1.4.0 fresh-member catalogue, not the whole of it: 13 `always_allow` rows
/// chosen to span the interesting cases, plus the 2 `ask_each_time` rows the runtime refuses to
/// disable. The real catalogue is 53 tools, 51 `always_allow`, 2 `ask_each_time` (§L-1) — measured
/// on a fresh member, not derived from this fixture.
///
/// The sample is deliberate. These tests are about the *policy function*, which cannot see how many
/// rows exist; the enumeration that must stay faithful lives in `docs/INVARIANTS.md` §L-1, where it
/// is printed and checkable. A fixture that tried to be the catalogue would be a second copy of it,
/// and a second copy is a second thing to disagree with the first.
fn fresh_catalog() -> BTreeMap<String, String> {
    let mut c = BTreeMap::new();
    for t in [
        "builtin.admin_configuration_replace",
        "builtin.apply_patch",
        "builtin.echo",
        "builtin.extension_install",
        "builtin.http",
        "builtin.http.save",
        "builtin.json",
        "builtin.outbound_deliver",
        "builtin.read_file",
        "builtin.spawn_subagent",
        "builtin.time",
        "builtin.write_file",
        "ironclaw.memory.write",
    ] {
        c.insert(t.to_owned(), "always_allow".to_owned());
    }
    for t in
        ["builtin.operator_config_set_tool_permission", "builtin.operator_config_set_auto_approve"]
    {
        c.insert(t.to_owned(), "ask_each_time".to_owned());
    }
    c
}

fn all_disabled() -> BTreeMap<String, String> {
    fresh_catalog().into_keys().map(|k| (k, "disabled".to_owned())).collect()
}

/// The declared ceiling decides the policy, in both directions.
///
/// Two-sided on purpose. Asserting only that a sandbox profile yields a shell would pass against a
/// function that returned `sandbox_workspace()` for everything — which is the failure that matters,
/// because it grants a shell to a service that declared none.
#[test]
fn the_policy_is_a_function_of_the_declared_ceiling() {
    use ironworks_domain::{CapabilityProfile, ToolAccess};

    let none = ConfinementPolicy::for_profile(&CapabilityProfile::READ_ONLY_RECORDS);
    assert!(none.allowed().is_empty(), "a record-only service must reach no built-in");
    assert!(!none.auto_approve());

    let sandbox = ConfinementPolicy::for_profile(&CapabilityProfile::SANDBOX_WORKSPACE);
    assert_eq!(
        sandbox.allowed().iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["builtin.shell"],
        "a workspace service reaches the sandbox shell and nothing else"
    );
    assert!(!sandbox.auto_approve(), "automatic approval stays off in every shipped policy");

    // The vacuity guard: the two must actually differ, or both assertions above could hold against
    // one degenerate answer.
    assert_ne!(none.fingerprint(), sandbox.fingerprint());

    // And the axis is what decides it — not `records`, `writes`, `egress` or `outreach`. A profile
    // differing from READ_ONLY_RECORDS only in `runtime_tools` must move the policy.
    let flipped = CapabilityProfile {
        runtime_tools: ToolAccess::Sandbox,
        egress: ironworks_domain::Access::ReadOnly,
        ..CapabilityProfile::READ_ONLY_RECORDS
    };
    assert_eq!(ConfinementPolicy::for_profile(&flipped).fingerprint(), sandbox.fingerprint());
}

/// What the derived policy actually *does* to a fresh member, which is the half a fingerprint
/// comparison cannot show.
#[test]
fn a_sandbox_ceiling_disables_everything_except_the_shell() {
    use ironworks_domain::CapabilityProfile;

    let policy = ConfinementPolicy::for_profile(&CapabilityProfile::SANDBOX_WORKSPACE);
    let mut catalog = fresh_catalog();
    catalog.insert("builtin.shell".to_owned(), "always_allow".to_owned());

    let plan = crate::confinement::plan(&policy, &catalog, &unlocked(), true);
    let disabled: Vec<&str> = plan
        .steps()
        .iter()
        .filter_map(|s| match s {
            crate::confinement::Step::Disable { tool } => Some(tool.as_str()),
            crate::confinement::Step::ApprovalOff => None,
        })
        .collect();

    assert!(!disabled.contains(&"builtin.shell"), "the allowed tool must survive the plan");
    for t in ["builtin.http", "builtin.outbound_deliver", "builtin.spawn_subagent"] {
        assert!(disabled.contains(&t), "{t} must still be disabled under a workspace policy");
    }
    // Approval-off is still step one: a workspace is not a reason to stop asking.
    assert!(matches!(plan.steps().first(), Some(crate::confinement::Step::ApprovalOff)));

    // The positive control for the negative above: under the record-only ceiling the shell is
    // disabled too, so `!disabled.contains(shell)` is a property of the policy rather than of the
    // planner never seeing the tool.
    let strict = ConfinementPolicy::for_profile(&CapabilityProfile::READ_ONLY_RECORDS);
    let strict_plan = crate::confinement::plan(&strict, &catalog, &unlocked(), true);
    assert!(
        strict_plan.steps().iter().any(|s| matches!(
            s, crate::confinement::Step::Disable { tool } if tool == "builtin.shell"
        )),
        "a record-only ceiling must disable the shell"
    );
}

/// A freshly minted 1.4.0 member is NOT at the settings-catalogue policy, and the report leads with
/// conservatively ordered authority-changing rows rather than alphabetical order.
#[test]
fn a_fresh_member_is_not_confined() {
    let policy = ConfinementPolicy::read_only_analyst();
    let out = assess(&policy, &member(), &fresh_catalog(), &unlocked(), true, NOW);

    let Confinement::Drifted { facts, drift } = out else { panic!("a fresh member was certified") };
    assert_eq!(facts.member, member());
    assert!(matches!(drift[0], Drift::AuthorityChangingReachable { .. }), "{:?}", drift[0]);
    assert!(
        drift.iter().any(|d| matches!(d, Drift::AutoApprove { observed: true, expected: false }))
    );
}

/// The policy applied exactly: every catalogued tool disabled, approval off.
#[test]
fn a_fully_denied_member_with_approval_off_is_verified() {
    let policy = ConfinementPolicy::read_only_analyst();
    let out = assess(&policy, &member(), &all_disabled(), &unlocked(), false, NOW);

    let Confinement::Verified(facts) = out else { panic!("expected Verified, got {out:?}") };
    assert!(facts.reachable.is_empty());
    assert_eq!(facts.catalogued, 15);
    assert_eq!(facts.policy_fingerprint, policy.fingerprint());
}

/// One denied tool re-enabled is drift — the case a live re-enable produces.
#[test]
fn a_single_re_enabled_tool_is_drift() {
    let policy = ConfinementPolicy::read_only_analyst();
    let mut c = all_disabled();
    c.insert("builtin.http".into(), "always_allow".into());

    let Confinement::Drifted { drift, .. } =
        assess(&policy, &member(), &c, &unlocked(), false, NOW)
    else {
        panic!("a re-enabled tool was certified as confined")
    };
    assert_eq!(drift.len(), 1);
    assert!(matches!(&drift[0], Drift::Reachable { tool, .. } if tool == "builtin.http"));
}

/// `ask_each_time` is NOT a denial. With `auto_approve_tools` true the approval arrives without a
/// human, so certifying it as denied would bless authority the model can actually use.
#[test]
fn ask_each_time_is_reachable_not_denied() {
    assert!(ToolState::parse("ask_each_time").is_reachable());
    assert!(!ToolState::parse("disabled").is_reachable());

    // An ORDINARY tool: the two the runtime refuses to disable have their own floor, tested
    // separately, and using one here would measure that exemption instead of this rule.
    let policy = ConfinementPolicy::read_only_analyst();
    let mut c = all_disabled();
    c.insert("builtin.http".into(), "ask_each_time".into());
    let out = assess(&policy, &member(), &c, &unlocked(), false, NOW);
    assert!(!out.is_verified(), "ask_each_time was treated as a denial");
}

/// Approval wider than policy is drift on its own, with every tool correctly denied.
#[test]
fn auto_approve_wider_than_policy_is_drift_by_itself() {
    let policy = ConfinementPolicy::read_only_analyst();
    let Confinement::Drifted { drift, .. } =
        assess(&policy, &member(), &all_disabled(), &unlocked(), true, NOW)
    else {
        panic!("a wider approval setting was certified")
    };
    assert_eq!(drift, vec![Drift::AutoApprove { observed: true, expected: false }]);
    assert_eq!(
        drift[0].to_string(),
        "automatic approval is ON where policy expects off — it does not bypass `ask_each_time`; \
         while on, it makes eligible globally governed tools report `always_allow`"
    );
}

/// A state this build cannot read is never counted as denied.
#[test]
fn an_unrecognized_state_is_never_read_as_denied() {
    let policy = ConfinementPolicy::read_only_analyst();
    let mut c = all_disabled();
    c.insert("builtin.http".into(), "quarantined_pending_review".into());

    let Confinement::Drifted { facts, drift } =
        assess(&policy, &member(), &c, &unlocked(), false, NOW)
    else {
        panic!("an unreadable state was certified as confined")
    };
    assert!(
        matches!(&drift[0], Drift::Unrecognized { raw, .. } if raw == "quarantined_pending_review")
    );
    assert!(facts.reachable.contains("builtin.http"), "it was counted as denied");
}

/// A policy naming a tool the catalogue does not offer was never fully applied. Not a hole — but
/// not a clean verification either.
#[test]
fn a_policy_allowing_an_absent_tool_is_drift() {
    let policy = ConfinementPolicy::allowing(["builtin.nonexistent".to_owned()], false);
    let Confinement::Drifted { drift, .. } =
        assess(&policy, &member(), &all_disabled(), &unlocked(), false, NOW)
    else {
        panic!("a policy written for another build was certified")
    };
    assert!(
        matches!(&drift[0], Drift::AllowedToolAbsent { tool } if tool == "builtin.nonexistent")
    );
}

/// Every authority-changing tool is ranked ahead of ordinary drift, because none of the other
/// denials can be relied on while one of these is reachable.
#[test]
fn every_authority_changing_tool_outranks_ordinary_drift() {
    let policy = ConfinementPolicy::read_only_analyst();
    for tool in AUTHORITY_CHANGING {
        let mut c = all_disabled();
        c.insert("builtin.echo".into(), "always_allow".into()); // ordinary drift, alphabetically first
        c.insert((*tool).to_owned(), "always_allow".into());
        let Confinement::Drifted { drift, .. } =
            assess(&policy, &member(), &c, &unlocked(), false, NOW)
        else {
            panic!("{tool} reachable was certified as confined")
        };
        assert!(
            matches!(&drift[0], Drift::AuthorityChangingReachable { tool: t, .. } if t == tool),
            "{tool} did not outrank ordinary drift: {:?}",
            drift[0]
        );
    }
}

/// The empty allow-set is the product answer, and the fingerprint pins which policy a verdict was
/// taken against — so "confined" cannot silently come to mean something else.
#[test]
fn the_read_only_policy_allows_nothing_and_is_fingerprinted() {
    let p = ConfinementPolicy::read_only_analyst();
    assert!(p.allowed().is_empty());
    assert!(!p.auto_approve());
    let wider = ConfinementPolicy::allowing(["builtin.time".to_owned()], false);
    assert_ne!(p.fingerprint(), wider.fingerprint(), "two policies share a fingerprint");
    assert_eq!(p.fingerprint(), ConfinementPolicy::read_only_analyst().fingerprint());
}

/// The best state IronClaw 1.4.0 permits IS verified confinement.
///
/// Measured: the runtime answers `400 invalid_value` to disabling
/// `builtin.operator_config_set_*`, and `200` to disabling `builtin.echo`. Treating their
/// unavoidable `ask_each_time` as drift would make `Verified` unreachable and the reconciler
/// useless — so the floor is encoded, and approval-off is what makes it safe.
#[test]
fn the_best_achievable_state_is_verified_and_approval_off_is_what_makes_it_so() {
    let policy = ConfinementPolicy::read_only_analyst();
    let mut c = all_disabled();
    for t in crate::confinement::UNDISABLEABLE {
        c.insert((*t).to_owned(), "ask_each_time".to_owned());
    }

    assert!(
        assess(&policy, &member(), &c, &unlocked(), false, NOW).is_verified(),
        "the achievable floor was rejected"
    );

    // Positive control: approval-on is independently wider than policy even when the two locked
    // settings rows remain at their catalogue floor. This does not assert they are model-visible.
    let Confinement::Drifted { drift, .. } = assess(&policy, &member(), &c, &unlocked(), true, NOW)
    else {
        panic!("approval-on with reachable authority-changing tools was certified")
    };
    assert!(drift.iter().any(|d| matches!(d, Drift::AuthorityChangingReachable { .. })));
    assert!(drift.iter().any(|d| matches!(d, Drift::AutoApprove { observed: true, .. })));

    // And `always_allow` on those two is drift even with approval off — the floor is
    // `ask_each_time`, not "anything goes for these".
    let mut worse = c.clone();
    worse.insert("builtin.operator_config_set_tool_permission".into(), "always_allow".into());
    assert!(!assess(&policy, &member(), &worse, &unlocked(), false, NOW).is_verified());
}

#[cfg(unix)]
fn mode_of(p: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).expect("stat the store").permissions().mode() & 0o777
}

/// The lifecycle store is the only record of which tenants exist and what a half-finished
/// provisioning run left behind.
#[cfg(unix)]
#[test]
fn the_lifecycle_store_is_owner_only() {
    let (dir, _store) = store();
    assert_eq!(
        mode_of(&dir.path().join("lifecycle.db")),
        0o600,
        "the lifecycle store is group- or world-readable"
    );
}

/// B10 for the lifecycle store. Its `Corrupt` variant already means an unreadable *row*; this is
/// the file itself, and the two must not be confused — one is a bad record among good ones, the
/// other is a store with nothing readable in it at all.
#[test]
fn a_lifecycle_store_that_is_not_a_database_refuses_and_says_not_to_delete_it() {
    let dir = Dir::new();
    let p = dir.path().join("lifecycle.db");
    std::fs::write(&p, b"this is not a database".repeat(10)).expect("write");
    let err = LifecycleStore::open(&p).expect_err("a corrupt store was opened");
    assert!(matches!(err, LifecycleError::NotADatabase { .. }), "{err:?}");
    assert!(err.to_string().contains("do NOT delete it blind"), "{err}");
}
