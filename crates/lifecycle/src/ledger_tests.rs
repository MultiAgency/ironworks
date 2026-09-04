//! The confinement ledger: how it is opened, migrated, appended to, and refused.
//!
//! Two properties get the most attention here, because both fail silently when they fail:
//!
//! - **Discovery precedes mutation.** An unknown version must be refused with the file untouched,
//!   which is asserted against the bytes and the sidecars rather than against the error alone.
//!   (A read-only open of a WAL database does create sidecars — that is SQLite reading, not us
//!   writing, and `a_read_only_open_changes_no_data_and_does_not_migrate` says what is actually
//!   guaranteed.)
//! - **A corrupt row is never an absent one.** Every strict-read refusal is checked to be
//!   `Corrupt`, because reporting damage as "no observation" would turn a broken ledger into a
//!   clean-looking one.

use super::*;

fn unlocked() -> std::collections::BTreeSet<String> {
    std::collections::BTreeSet::new()
}
use crate::attempt::{
    AttemptVariant, Blocked, ConfinementAttempt, ConfinementEvidence, Historical, Measurement,
};
use crate::confinement::{ConfinementPolicy, Drift, PolicyBinding, ToolState};
use crate::runtime::{BoundSubject, ObservedAt, RuntimeSubject, RuntimeTenantId, RuntimeUserId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const NOW: &str = "2026-08-27T12:00:00+00:00";
const MEMBER: &str = "user_7f3a";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ironworks-ledger-{}-{}",
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

fn spec() -> InstanceSpec {
    InstanceSpec {
        slug: Slug::parse("acme").unwrap(),
        org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
        service: ironworks_domain::ServiceId::parse("account-analysis").unwrap(),
        channel: ironworks_domain::ChannelRef::new("-100900001"),
    }
}

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

fn subject(user: &str) -> RuntimeSubject {
    RuntimeSubject::new(
        RuntimeTenantId::parse("acme").expect("tenant"),
        RuntimeUserId::parse(user).expect("user"),
    )
}

fn bound_to(member: &str) -> BoundSubject {
    BoundSubject::bind(OpaqueId::new(member), subject(member)).expect("binds")
}

fn at(raw: &str) -> ObservedAt {
    ObservedAt::parse(raw).expect("timestamp")
}

fn catalogue(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn measurement(member: &str, entries: &[(&str, &str)], when: &str) -> Measurement {
    Measurement::from_assessment(
        &ConfinementPolicy::read_only_analyst(),
        &bound_to(member),
        &catalogue(entries),
        &unlocked(),
        false,
        &at(when),
    )
}

fn clean_evidence(member: &str, when: &str) -> ConfinementEvidence {
    let current = binding();
    let attempt = ConfinementAttempt::measured(
        bound_to(member),
        current.clone(),
        at(when),
        measurement(member, &[("builtin.http", "disabled")], when),
    );
    ConfinementEvidence::from_attempt(attempt, &current).expect("clean current-policy evidence")
}

/// A declared instance walked far enough that a member exists.
fn seeded(store: &LifecycleStore) -> Slug {
    let s = spec();
    store.declare(&s, NOW).expect("declare");
    for stage in Stage::ALL {
        let id = Authority::ALL.iter().copied().find(|a| a.created_at() == stage).map(|a| {
            OpaqueId::new(if a == Authority::Member {
                MEMBER.to_owned()
            } else {
                format!("{}-1", a.tag())
            })
        });
        store.reach(&s.slug, stage, false, id.as_ref(), NOW).expect("reach");
        if stage == Stage::MemberMinted {
            break;
        }
    }
    s.slug
}

/// The sidecars a WAL-mode open leaves. Used to establish a clean baseline before asserting that
/// a REFUSED open wrote nothing — a refusal happens before any pragma, so it leaves none.
fn sidecars(path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let p = PathBuf::from(p);
        if p.exists() {
            found.push(p);
        }
    }
    found
}

// ── open, discovery and migration ───────────────────────────────────────────────────────────

#[test]
fn a_brand_new_store_is_born_at_current_version() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    assert_eq!(store.schema_version(), 3);
}

/// The v1 fixture is built by the CURRENT writer and then walked back, so it is the shape a real
/// store has rather than one this test invented.
fn v1_store(path: &Path) {
    let store = LifecycleStore::open(path).expect("open");
    seeded(&store);
    let conn = store.conn.lock().expect("lock");
    for object in V2_OBJECTS {
        let kind = if object.contains("_latest") || object.contains("_member") {
            "INDEX"
        } else {
            "TABLE"
        };
        conn.execute_batch(&format!("DROP {kind} IF EXISTS {object};")).expect("drop v2");
    }
    conn.execute_batch("DROP TABLE authority_identities;").expect("drop v3");
    conn.execute("UPDATE lifecycle_meta SET value='1' WHERE key='schema_version'", [])
        .expect("walk back to v1");
}

#[test]
fn a_v1_store_migrates_and_keeps_every_original_row() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    v1_store(&path);

    let (spec_before, recorded_before, history_before) = {
        let ro = LifecycleStore::open_readonly(&path).expect("v1 opens read-only");
        assert_eq!(ro.schema_version(), 1);
        let slug = Slug::parse("acme").unwrap();
        (
            ro.spec(&slug).expect("spec"),
            ro.created(&slug).expect("created"),
            ro.history(&slug).expect("history"),
        )
    };
    assert!(!history_before.is_empty(), "the fixture must have stages to preserve");

    let store = LifecycleStore::open(&path).expect("migrate");
    assert_eq!(store.schema_version(), 3);
    let slug = Slug::parse("acme").unwrap();
    assert_eq!(store.spec(&slug).expect("spec"), spec_before);
    assert_eq!(store.created(&slug).expect("created"), recorded_before);
    assert_eq!(store.history(&slug).expect("history"), history_before, "stage history moved");
    // And the ledger is usable afterwards.
    assert_eq!(store.latest_applicable(&slug, &OpaqueId::new(MEMBER)).expect("query"), None);
}

#[test]
fn a_v2_adopted_id_migrates_without_becoming_created() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let s = spec();
    {
        let store = LifecycleStore::open(&path).expect("open");
        store.declare(&s, NOW).expect("declare");
        store.reach(&s.slug, Stage::PreflightPassed, false, None, NOW).expect("preflight");
        store
            .reach(&s.slug, Stage::OrgRegistered, true, Some(&OpaqueId::new("existing-org")), NOW)
            .expect("adopt old-shape identity");
        let conn = store.conn.lock().expect("lock");
        conn.execute_batch(
            "DROP TABLE authority_identities;
             UPDATE lifecycle_meta SET value='2' WHERE key='schema_version';",
        )
        .expect("walk back to v2");
    }

    let store = LifecycleStore::open(&path).expect("migrate");
    assert_eq!(store.schema_version(), 3);
    assert_eq!(store.created(&s.slug).unwrap().created_id(Authority::OrgRegistration), None);
    let identity =
        store.recorded(&s.slug).unwrap().identity(Authority::OrgRegistration).unwrap().clone();
    assert_eq!(identity.id(), &OpaqueId::new("existing-org"));
    assert_eq!(identity.provenance(), IdentityProvenance::Adopted);
    assert_eq!(identity.runtime_subject(), None, "v2 recorded no runtime tenant to recover");
}

#[test]
fn migration_is_idempotent() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    v1_store(&path);
    LifecycleStore::open(&path).expect("first");
    let after_first = std::fs::read(&path).expect("read");
    LifecycleStore::open(&path).expect("second");
    assert_eq!(std::fs::read(&path).expect("read"), after_first, "a second open changed the file");
}

/// Interruption, through SQLite's own transactional semantics rather than a production hook.
#[test]
fn an_interrupted_migration_rolls_back_and_the_retry_succeeds() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    v1_store(&path);
    let before = std::fs::read(&path).expect("read");

    {
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute_batch("BEGIN; ").expect("begin");
        conn.execute_batch(SCHEMA_V2).expect("v2 ddl inside the transaction");
        conn.execute("UPDATE lifecycle_meta SET value='2' WHERE key='schema_version'", [])
            .expect("bump");
        conn.execute_batch("ROLLBACK;").expect("rollback");
    }

    let ro = LifecycleStore::open_readonly(&path).expect("still readable");
    assert_eq!(ro.schema_version(), 1, "a rolled-back migration must leave the version alone");
    assert!(!ro.history(&Slug::parse("acme").unwrap()).expect("history").is_empty());
    drop(ro);
    assert_eq!(std::fs::read(&path).expect("read"), before, "the rollback left bytes behind");

    // The retry is the point: idempotent-by-rollback rather than idempotent-by-ignoring.
    let store = LifecycleStore::open(&path).expect("retry migrates");
    assert_eq!(store.schema_version(), 3);
}

/// A pre-existing object of the wrong shape must fail loudly rather than be adopted. This is what
/// `CREATE TABLE` without `IF NOT EXISTS` buys.
#[test]
fn a_conflicting_partial_v2_object_refuses_and_keeps_refusing() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    v1_store(&path);
    {
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute_batch("CREATE TABLE confinement_attempts (wrong INTEGER);").expect("conflict");
    }
    assert!(LifecycleStore::open(&path).is_err(), "a conflicting object must not be adopted");
    let ro = LifecycleStore::open_readonly(&path).expect("still readable");
    assert_eq!(ro.schema_version(), 1, "the version must not have moved");
    assert!(!ro.history(&Slug::parse("acme").unwrap()).expect("history").is_empty());
    drop(ro);
    assert!(LifecycleStore::open(&path).is_err(), "and it must fail the same way again");
}

/// The ordering property, asserted against the filesystem. Before discovery moved ahead of the
/// pragmas, this store would have been rewritten into WAL mode by the command that refused it.
#[test]
fn an_unknown_version_is_refused_before_anything_is_written() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    {
        let store = LifecycleStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");
        conn.execute("UPDATE lifecycle_meta SET value='99' WHERE key='schema_version'", [])
            .expect("bump");
    }
    // A clean baseline: no journal sidecars, and a known byte image.
    for sidecar in sidecars(&path) {
        std::fs::remove_file(sidecar).expect("clear");
    }
    let before = std::fs::read(&path).expect("read");

    assert!(matches!(
        LifecycleStore::open(&path),
        Err(LifecycleError::Version { found: 99, expected: 3 })
    ));
    assert_eq!(std::fs::read(&path).expect("read"), before, "the refused open mutated the store");
    assert!(sidecars(&path).is_empty(), "the refused open created a WAL sidecar");
}

#[test]
fn objects_without_lifecycle_metadata_are_never_adopted() {
    let dir = Dir::new();

    // Another program's database.
    let foreign = dir.path().join("foreign.db");
    {
        let conn = rusqlite::Connection::open(&foreign).expect("raw open");
        conn.execute_batch("CREATE TABLE somebody_elses (x INTEGER);").expect("create");
    }
    assert!(matches!(LifecycleStore::open(&foreign), Err(LifecycleError::Uninitialized { .. })));
    assert!(matches!(
        LifecycleStore::open_readonly(&foreign),
        Err(LifecycleError::Uninitialized { .. })
    ));

    // A store whose creation was interrupted: the metadata table exists with no version row.
    let half = dir.path().join("half.db");
    {
        let conn = rusqlite::Connection::open(&half).expect("raw open");
        conn.execute_batch(
            "CREATE TABLE lifecycle_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE instances (slug TEXT PRIMARY KEY);",
        )
        .expect("create");
    }
    assert!(
        matches!(LifecycleStore::open(&half), Err(LifecycleError::Uninitialized { .. })),
        "a half-created store must not be initialised over"
    );
}

/// # What "writes nothing" means, measured rather than assumed
///
/// A read-only open of a WAL database **does** create `-wal` and `-shm` sidecars: they are
/// SQLite's shared-memory index and log, and it needs them to read at all. Measured directly
/// against the library, so this is not a property IronWorks can have and the design's "no sidecar
/// creation" clause was too strong.
///
/// What is guaranteed, and what this asserts: the database file itself is unchanged, no migration
/// happened, and the version did not move. That is the property `confine --dry-run` rests on.
#[test]
fn a_read_only_open_changes_no_data_and_does_not_migrate() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    v1_store(&path);
    for sidecar in sidecars(&path) {
        std::fs::remove_file(sidecar).expect("clear");
    }
    let before = std::fs::read(&path).expect("read");

    let ro = LifecycleStore::open_readonly(&path).expect("v1 read-only");
    assert_eq!(ro.schema_version(), 1);
    drop(ro);

    assert_eq!(std::fs::read(&path).expect("read"), before, "read-only changed the store");
    // And it did NOT migrate: this is what `confine --dry-run` relies on.
    let ro = LifecycleStore::open_readonly(&path).expect("still v1");
    assert_eq!(ro.schema_version(), 1, "a read-only open migrated the store");
}

#[test]
fn an_empty_file_opened_read_only_says_it_is_uninitialised() {
    let dir = Dir::new();
    let path = dir.path().join("nothing.db");
    std::fs::write(&path, b"").expect("touch");
    assert!(matches!(
        LifecycleStore::open_readonly(&path),
        Err(LifecycleError::Uninitialized { .. })
    ));
}

// ── appending ───────────────────────────────────────────────────────────────────────────────

#[test]
fn only_clean_current_policy_measurements_become_certification_evidence() {
    let current = binding();
    let drifted = ConfinementAttempt::measured(
        bound_to(MEMBER),
        current.clone(),
        at(NOW),
        measurement(MEMBER, &[("builtin.http", "always_allow")], NOW),
    );
    assert!(ConfinementEvidence::from_attempt(drifted, &current).is_none());

    let other = PolicyBinding::of(
        &definition(),
        &ConfinementPolicy::allowing(["builtin.shell".to_owned()], false),
    );
    let stale = ConfinementAttempt::measured(
        bound_to(MEMBER),
        other,
        at(NOW),
        measurement(MEMBER, &[("builtin.http", "disabled")], NOW),
    );
    assert!(ConfinementEvidence::from_attempt(stale, &current).is_none());
}

#[test]
fn certification_appends_the_clean_measurement_and_advances_the_stage_atomically() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    let evidence = clean_evidence(MEMBER, NOW);

    store.certify_confinement(&slug, &evidence).expect("certify");

    assert_eq!(
        store.progress(&slug).unwrap(),
        ProvisionProgress::Provisioning { reached: Stage::MemberConfined }
    );
    assert_eq!(store.confinement_history(&slug, 10).unwrap(), vec![evidence.attempt().clone()]);
    let recorded = store.recorded(&slug).unwrap();
    assert_eq!(
        recorded.identity(Authority::MemberConfinement).unwrap().id().as_str(),
        binding().fingerprint().as_str()
    );
}

#[test]
fn certification_can_be_observed_again_but_the_stage_is_reached_only_once() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store.certify_confinement(&slug, &clean_evidence(MEMBER, NOW)).unwrap();
    store.certify_confinement(&slug, &clean_evidence(MEMBER, "2026-08-27T12:00:05+00:00")).unwrap();

    assert_eq!(
        store
            .history(&slug)
            .unwrap()
            .iter()
            .filter(|(s, _, _)| *s == Stage::MemberConfined)
            .count(),
        1
    );
    assert_eq!(store.confinement_history(&slug, 10).unwrap().len(), 2);
}

#[test]
fn a_failed_certification_rolls_back_its_attempt_and_stage_together() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute(
            "INSERT INTO authority_identities(
                 slug,authority,opaque_id,provenance,observed_at,subject_tenant,subject_user
             ) VALUES(?1,?2,'prior','created',?3,NULL,NULL)",
            params![slug.as_str(), Authority::MemberConfinement.tag(), NOW],
        )
        .expect("plant conflicting identity");
    }

    assert!(store.certify_confinement(&slug, &clean_evidence(MEMBER, NOW)).is_err());
    assert_eq!(
        store.progress(&slug).unwrap(),
        ProvisionProgress::Provisioning { reached: Stage::MemberMinted }
    );
    assert!(store.confinement_history(&slug, 10).unwrap().is_empty());
    assert!(!store.history(&slug).unwrap().iter().any(|(s, _, _)| *s == Stage::MemberConfined));
}

#[test]
fn certification_refuses_evidence_for_another_member_without_recording_it() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    let error = store
        .certify_confinement(&slug, &clean_evidence("user_somebody_else", NOW))
        .expect_err("wrong member");
    assert!(matches!(error, LifecycleError::ConfinementMemberMismatch { .. }));
    assert!(store.confinement_history(&slug, 10).unwrap().is_empty());
}

#[test]
fn a_measured_attempt_round_trips_with_its_children() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    let m =
        measurement(MEMBER, &[("builtin.http", "always_allow"), ("builtin.time", "disabled")], NOW);
    assert!(!m.is_clean(), "the fixture must produce drift to round-trip");
    let attempt = ConfinementAttempt::measured(bound_to(MEMBER), binding(), at(NOW), m.clone());
    store.record_attempt(&slug, &attempt).expect("append");

    let read = store
        .latest_applicable(&slug, &OpaqueId::new(MEMBER))
        .expect("query")
        .expect("one attempt");
    assert_eq!(read, attempt, "the attempt did not survive the round trip");
    assert_eq!(read.measurement().expect("measurement"), &m);
    assert_eq!(read.variant(), AttemptVariant::Measured);
}

#[test]
fn every_drift_kind_round_trips_through_sql() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    // A catalogue that produces four kinds at once, plus an allowed tool the catalogue lacks.
    let policy = ConfinementPolicy::allowing(["builtin.shell".to_owned()], false);
    let m = Measurement::from_assessment(
        &policy,
        &bound_to(MEMBER),
        &catalogue(&[
            ("builtin.spawn_subagent", "always_allow"),
            ("builtin.http", "ask_each_time"),
            ("builtin.odd", "a_state_from_the_future"),
        ]),
        &unlocked(),
        true,
        &at(NOW),
    );
    let kinds: std::collections::BTreeSet<&str> = m.drift().iter().map(Drift::kind_tag).collect();
    assert_eq!(kinds.len(), 5, "the fixture must exercise every drift kind: {kinds:?}");

    let attempt = ConfinementAttempt::measured(
        bound_to(MEMBER),
        PolicyBinding::of(&definition(), &policy),
        at(NOW),
        m.clone(),
    );
    store.record_attempt(&slug, &attempt).expect("append");
    let read = store
        .latest_applicable(&slug, &OpaqueId::new(MEMBER))
        .expect("query")
        .expect("one attempt");
    assert_eq!(read.measurement().expect("measurement").drift(), m.drift());
}

#[test]
fn the_three_non_measuring_attempts_round_trip() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);

    let mismatch =
        BoundSubject::bind(OpaqueId::new(MEMBER), subject("reborn-cli")).expect_err("differs");
    let attempts = [
        ConfinementAttempt::binding_blocked(
            OpaqueId::new(MEMBER),
            at("2026-08-27T12:00:01+00:00"),
            Blocked::RuntimeUnreachable,
        ),
        ConfinementAttempt::subject_mismatch(mismatch, at("2026-08-27T12:00:02+00:00")),
        ConfinementAttempt::catalogue_blocked(
            bound_to(MEMBER),
            binding(),
            at("2026-08-27T12:00:03+00:00"),
            Blocked::CredentialRejected,
        ),
    ];
    for attempt in &attempts {
        store.record_attempt(&slug, attempt).expect("append");
    }
    let history = store.confinement_history(&slug, 10).expect("history");
    assert_eq!(history.len(), 3);
    // Newest first.
    assert_eq!(&history[0], &attempts[2]);
    assert_eq!(&history[1], &attempts[1]);
    assert_eq!(&history[2], &attempts[0]);
}

/// The audit record the design exists for: a repair leaves BOTH rows, so a run that fixed real
/// drift stays distinguishable from one that found nothing to do.
#[test]
fn a_later_clean_measurement_never_overwrites_an_earlier_drift() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);

    let drifted = ConfinementAttempt::measured(
        bound_to(MEMBER),
        binding(),
        at("2026-08-27T12:00:00+00:00"),
        measurement(MEMBER, &[("builtin.http", "always_allow")], "2026-08-27T12:00:00+00:00"),
    );
    let clean = ConfinementAttempt::measured(
        bound_to(MEMBER),
        binding(),
        at("2026-08-27T12:00:05+00:00"),
        measurement(MEMBER, &[("builtin.http", "disabled")], "2026-08-27T12:00:05+00:00"),
    );
    store.record_attempt(&slug, &drifted).expect("append");
    store.record_attempt(&slug, &clean).expect("append");

    let history = store.confinement_history(&slug, 10).expect("history");
    assert_eq!(history.len(), 2, "the drifted row was overwritten");
    assert!(matches!(history[0].historical(), Historical::CleanAt { .. }));
    assert!(matches!(history[1].historical(), Historical::DriftedAt { .. }));
    let latest = store.latest_applicable(&slug, &OpaqueId::new(MEMBER)).expect("q").expect("one");
    assert_eq!(latest, clean, "the latest must be the newer of the two");
}

/// `now_iso` has one-second granularity and a single `confine` run appends twice inside it, so the
/// rowid tiebreak decides which is latest. This is routine, not a corner case.
#[test]
fn attempts_sharing_a_timestamp_order_by_append_sequence() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);

    let first = ConfinementAttempt::measured(
        bound_to(MEMBER),
        binding(),
        at(NOW),
        measurement(MEMBER, &[("builtin.http", "always_allow")], NOW),
    );
    let second = ConfinementAttempt::measured(
        bound_to(MEMBER),
        binding(),
        at(NOW),
        measurement(MEMBER, &[("builtin.http", "disabled")], NOW),
    );
    store.record_attempt(&slug, &first).expect("append");
    store.record_attempt(&slug, &second).expect("append");

    let latest = store.latest_applicable(&slug, &OpaqueId::new(MEMBER)).expect("q").expect("one");
    assert_eq!(latest, second, "same timestamp, so the later append must win");
}

/// Replacement over time is legitimate. A previous member's history stays readable and never
/// becomes the current member's baseline.
#[test]
fn history_is_kept_across_members_and_filtered_for_the_current_one() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);

    let old = ConfinementAttempt::measured(
        bound_to("user_old"),
        binding(),
        at("2026-08-27T12:00:00+00:00"),
        measurement("user_old", &[("builtin.http", "disabled")], "2026-08-27T12:00:00+00:00"),
    );
    store.record_attempt(&slug, &old).expect("append");

    // The current member has no attempt of its own, even though a newer-looking clean one exists.
    assert_eq!(store.latest_applicable(&slug, &OpaqueId::new(MEMBER)).expect("q"), None);
    assert_eq!(
        store.latest_applicable(&slug, &OpaqueId::new("user_old")).expect("q"),
        Some(old.clone())
    );
    // And history shows both members, unfiltered.
    assert_eq!(store.confinement_history(&slug, 10).expect("history"), vec![old]);
}

#[test]
fn a_read_only_store_refuses_to_append() {
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let slug = {
        let store = LifecycleStore::open(&path).expect("open");
        seeded(&store)
    };
    let ro = LifecycleStore::open_readonly(&path).expect("read-only");
    let attempt =
        ConfinementAttempt::binding_blocked(OpaqueId::new(MEMBER), at(NOW), Blocked::NoCredential);
    assert!(matches!(ro.record_attempt(&slug, &attempt), Err(LifecycleError::ReadOnly)));
}

/// No credential material may reach the database or its journal. Asserted against raw bytes,
/// because "the code has no column for one" is a claim about the code.
#[test]
fn no_credential_material_reaches_the_store_or_its_wal() {
    const SENTINEL: &str = "sk-SENTINEL-do-not-persist";
    let dir = Dir::new();
    let path = dir.path().join("lifecycle.db");
    let store = LifecycleStore::open(&path).expect("open");
    let slug = seeded(&store);
    // Every blocked path, carrying the sentinel in the payload the projection must drop.
    for reason in [
        BlockedReason::Unreachable { system: format!("ironclaw admin ({SENTINEL})") },
        BlockedReason::Malformed { detail: format!("HTTP 500: {SENTINEL}") },
        BlockedReason::NoCredential { system: format!("member credential at env:{SENTINEL}") },
    ] {
        let attempt = ConfinementAttempt::binding_blocked(
            OpaqueId::new(MEMBER),
            at(NOW),
            Blocked::from(&reason),
        );
        store.record_attempt(&slug, &attempt).expect("append");
    }
    drop(store);

    for file in std::iter::once(path.clone()).chain(sidecars(&path)) {
        let bytes = std::fs::read(&file).expect("read");
        assert!(
            !bytes.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes()),
            "{} carries credential material",
            file.display()
        );
    }
}

// ── strict reads ────────────────────────────────────────────────────────────────────────────

/// Every one of these is damage, and every one must be `Corrupt` rather than `Ok(None)`.
#[test]
fn a_row_this_build_cannot_interpret_is_corrupt_and_never_absent() {
    // An unknown *variant* is not here: the SQL CHECK refuses it on write, so it can never be
    // read back. That is asserted in `sql_refuses_every_variant_to_column_contradiction`.
    let cases: &[(&str, &str)] = &[
        ("an unknown blocked kind", "UPDATE confinement_attempts SET blocked_kind='mystery'"),
        (
            "a timestamp that cannot order",
            "UPDATE confinement_attempts SET at='sometime last tuesday'",
        ),
    ];
    for (label, sql) in cases {
        let dir = Dir::new();
        let path = dir.path().join("lifecycle.db");
        let store = LifecycleStore::open(&path).expect("open");
        let slug = seeded(&store);
        store
            .record_attempt(
                &slug,
                &ConfinementAttempt::binding_blocked(
                    OpaqueId::new(MEMBER),
                    at(NOW),
                    Blocked::RuntimeUnreachable,
                ),
            )
            .expect("append");
        {
            let conn = store.conn.lock().expect("lock");
            conn.execute(sql, []).expect("damage");
        }
        let read = store.latest_applicable(&slug, &OpaqueId::new(MEMBER));
        assert!(
            matches!(read, Err(LifecycleError::Corrupt { .. })),
            "{label} was not reported as corrupt: {read:?}"
        );
    }
}

#[test]
fn child_rows_under_a_non_measured_parent_make_the_whole_attempt_corrupt() {
    // SQLite cannot express this: a child-table CHECK cannot see the parent's variant.
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::catalogue_blocked(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                Blocked::UnreadableAnswer,
            ),
        )
        .expect("append");
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute(
            "INSERT INTO confinement_reachable(attempt_id, tool)
             SELECT id, 'builtin.http' FROM confinement_attempts LIMIT 1",
            [],
        )
        .expect("graft a child");
    }
    assert!(matches!(
        store.latest_applicable(&slug, &OpaqueId::new(MEMBER)),
        Err(LifecycleError::Corrupt { .. })
    ));
}

#[test]
fn a_measurement_reachable_in_more_tools_than_it_catalogued_is_corrupt() {
    // Also a cross-table aggregate SQLite cannot check.
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::measured(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                measurement(MEMBER, &[("builtin.http", "always_allow")], NOW),
            ),
        )
        .expect("append");
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute("UPDATE confinement_attempts SET catalogued=0", []).expect("damage");
    }
    assert!(matches!(
        store.latest_applicable(&slug, &OpaqueId::new(MEMBER)),
        Err(LifecycleError::Corrupt { .. })
    ));
}

/// The two invariants reconstruction gets for free by routing through `BoundSubject::bind`.
#[test]
fn a_stored_subject_that_contradicts_its_variant_is_corrupt() {
    // A `measured` row whose subject is not its expected member.
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::measured(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                measurement(MEMBER, &[("builtin.http", "disabled")], NOW),
            ),
        )
        .expect("append");
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute("UPDATE confinement_attempts SET subject_user='somebody-else'", [])
            .expect("damage");
    }
    assert!(matches!(
        store.latest_applicable(&slug, &OpaqueId::new(MEMBER)),
        Err(LifecycleError::Corrupt { .. })
    ));

    // And the mirror image: a `subject_mismatch` row whose subject matches.
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    let mismatch =
        BoundSubject::bind(OpaqueId::new(MEMBER), subject("reborn-cli")).expect_err("differs");
    store
        .record_attempt(&slug, &ConfinementAttempt::subject_mismatch(mismatch, at(NOW)))
        .expect("append");
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute(&format!("UPDATE confinement_attempts SET subject_user='{MEMBER}'"), [])
            .expect("damage");
    }
    assert!(matches!(
        store.latest_applicable(&slug, &OpaqueId::new(MEMBER)),
        Err(LifecycleError::Corrupt { .. })
    ));
}

#[test]
fn a_reachable_drift_naming_an_uninterpretable_state_is_corrupt() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::measured(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                measurement(MEMBER, &[("builtin.http", "always_allow")], NOW),
            ),
        )
        .expect("append");
    {
        let conn = store.conn.lock().expect("lock");
        conn.execute("UPDATE confinement_drift SET state='a_state_from_the_future'", [])
            .expect("damage");
    }
    // `assess` emits `Unrecognized` for an unknown state and `Reachable` only for a known one, so
    // this pairing could never have been written.
    assert!(matches!(
        store.latest_applicable(&slug, &OpaqueId::new(MEMBER)),
        Err(LifecycleError::Corrupt { .. })
    ));
}

// ── the SQL constraints themselves ──────────────────────────────────────────────────────────

#[test]
fn sql_refuses_every_variant_to_column_contradiction() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    seeded(&store);
    let conn = store.conn.lock().expect("lock");

    // Positive control FIRST: a well-formed row of each variant inserts. Without it, every
    // rejection below would pass against a schema that refused everything.
    for (variant, columns, values) in [
        ("binding_blocked", "blocked_kind", "'runtime_unreachable'"),
        ("subject_mismatch", "subject_tenant, subject_user", "'acme','somebody'"),
    ] {
        conn.execute(
            &format!(
                "INSERT INTO confinement_attempts(slug, variant, expected, at, {columns})
                 VALUES('acme','{variant}','{MEMBER}','{NOW}',{values})"
            ),
            [],
        )
        .unwrap_or_else(|e| panic!("a well-formed {variant} row must insert: {e}"));
    }

    let refusals: &[(&str, &str)] = &[
        (
            "binding_blocked carrying a subject",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, subject_tenant,
                 subject_user, blocked_kind)
             VALUES('acme','binding_blocked','m','t-timestamp-000000','acme','u','no_credential')",
        ),
        (
            "subject_mismatch carrying a policy",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, subject_tenant,
                 subject_user, service, service_version, policy_fingerprint)
             VALUES('acme','subject_mismatch','m','t-timestamp-000000','acme','u','svc',1,
                    '0123456789abcdef')",
        ),
        (
            "catalogue_blocked carrying a catalogue count",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, subject_tenant,
                 subject_user, service, service_version, policy_fingerprint, catalogued,
                 auto_approve, blocked_kind)
             VALUES('acme','catalogue_blocked','m','t-timestamp-000000','acme','u','svc',1,
                    '0123456789abcdef',3,0,'unreadable_answer')",
        ),
        (
            "measured with no subject",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, service,
                 service_version, policy_fingerprint, catalogued, auto_approve)
             VALUES('acme','measured','m','t-timestamp-000000','svc',1,'0123456789abcdef',3,0)",
        ),
        (
            "a fingerprint of the wrong width",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, subject_tenant,
                 subject_user, service, service_version, policy_fingerprint, catalogued,
                 auto_approve)
             VALUES('acme','measured','m','t-timestamp-000000','acme','u','svc',1,'abc',3,0)",
        ),
        (
            "an unknown variant tag",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, blocked_kind)
             VALUES('acme','something_else','m','t-timestamp-000000','no_credential')",
        ),
        (
            "an attempt for an instance that does not exist",
            "INSERT INTO confinement_attempts(slug, variant, expected, at, blocked_kind)
             VALUES('nobody','binding_blocked','m','t-timestamp-000000','no_credential')",
        ),
    ];
    for (label, sql) in refusals {
        assert!(conn.execute(sql, []).is_err(), "SQL admitted {label}");
    }
}

#[test]
fn sql_refuses_every_per_kind_drift_contradiction() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::measured(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                measurement(MEMBER, &[("builtin.http", "disabled")], NOW),
            ),
        )
        .expect("append");
    let conn = store.conn.lock().expect("lock");
    let id: i64 = conn
        .query_row("SELECT id FROM confinement_attempts LIMIT 1", [], |r| r.get(0))
        .expect("id");

    // Positive control: a well-formed drift row of each shape inserts.
    for (ordinal, kind, columns, values) in [
        (10, "reachable", "tool, state", "'builtin.http','always_allow'"),
        (11, "allowed_tool_absent", "tool", "'builtin.shell'"),
        (12, "auto_approve", "observed, expected", "1,0"),
    ] {
        conn.execute(
            &format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, {columns})
                 VALUES({id},{ordinal},'{kind}',{values})"
            ),
            [],
        )
        .unwrap_or_else(|e| panic!("a well-formed {kind} row must insert: {e}"));
    }

    let refusals: &[(&str, String)] = &[
        (
            "auto_approve with a tool",
            format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool, observed, expected)
                 VALUES({id},20,'auto_approve','builtin.http',1,0)"
            ),
        ),
        (
            "auto_approve with no booleans",
            format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind)
                 VALUES({id},21,'auto_approve')"
            ),
        ),
        (
            "reachable with no state",
            format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool)
                 VALUES({id},22,'reachable','builtin.http')"
            ),
        ),
        (
            "allowed_tool_absent carrying a state",
            format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool, state)
                 VALUES({id},23,'allowed_tool_absent','builtin.shell','always_allow')"
            ),
        ),
        (
            "an unknown drift kind",
            format!(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool, state)
                 VALUES({id},24,'something_else','builtin.http','always_allow')"
            ),
        ),
        (
            "a child of an attempt that does not exist",
            "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool, state)
             VALUES(999999,0,'reachable','builtin.http','always_allow')"
                .to_owned(),
        ),
    ];
    for (label, sql) in refusals {
        assert!(conn.execute(sql, []).is_err(), "SQL admitted {label}");
    }
}

#[test]
fn a_tool_cannot_be_reachable_twice_in_one_measurement() {
    let dir = Dir::new();
    let store = LifecycleStore::open(dir.path().join("lifecycle.db")).expect("open");
    let slug = seeded(&store);
    store
        .record_attempt(
            &slug,
            &ConfinementAttempt::measured(
                bound_to(MEMBER),
                binding(),
                at(NOW),
                measurement(MEMBER, &[("builtin.http", "always_allow")], NOW),
            ),
        )
        .expect("append");
    let conn = store.conn.lock().expect("lock");
    let id: i64 = conn
        .query_row("SELECT id FROM confinement_attempts LIMIT 1", [], |r| r.get(0))
        .expect("id");
    assert!(
        conn.execute(
            &format!(
                "INSERT INTO confinement_reachable(attempt_id, tool) VALUES({id},'builtin.http')"
            ),
            [],
        )
        .is_err(),
        "the same tool was admitted twice"
    );
}

#[test]
fn tool_states_round_trip_through_their_wire_spelling() {
    // The ledger stores a state and reads it back as itself, including one this build cannot
    // interpret — normalising that away would lose the only fact worth keeping about it.
    for raw in ["always_allow", "ask_each_time", "disabled", "a_state_from_the_future"] {
        assert_eq!(ToolState::parse(raw).wire(), raw);
    }
}
