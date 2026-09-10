mod common;
use common::serve;
use ironworks_domain::{
    CapabilityProfile, ChannelRef, ScopeEvidence, ServiceId, Slug, TenantRegistry,
};
use ironworks_lifecycle::{
    AdoptionEvidence, Authority, ConfinementAttempt, ConfinementEvidence, ConfinementPolicy,
    IdentityProvenance, LifecycleStore, Measurement, ObservedAt, PolicyBinding, RuntimeSubject,
    RuntimeTenantId, RuntimeUserId, Stage,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one instant this fixture records at, as the type the ledger orders by.
fn now() -> ironworks_lifecycle::ObservedAt {
    ironworks_lifecycle::ObservedAt::parse("2026-09-03T12:00:00+00:00")
        .expect("the fixture timestamp is the shape the ledger orders by")
}

/// A temporary directory. `ironworks_testkit::Scratch`, aliased so the tests read as they did —
/// this was one of six copies, already drifted between `path()` and `.0` and between `expect` and
/// `unwrap`.
type Dir = ironworks_testkit::Scratch;

fn dir() -> Dir {
    Dir::new("registry")
}

fn prepared(dir: &Dir) -> (PathBuf, PathBuf, PathBuf) {
    let store_path = dir.path().join("lifecycle.store");
    let registry_path = dir.path().join("registry.json");
    let guidance_path = dir.path().join("guidance.md");
    let guidance = format!(
        "<!-- tenant-guidance v1 slug: local-name service: relationship-intelligence -->\n\
         # Guidance\n{}",
        "Answer only from supplied records, distinguish evidence from inference, and name every \
         unresolved contradiction before recommending action.\n"
            .repeat(5)
    );
    std::fs::write(&guidance_path, guidance).unwrap();

    let slug = Slug::parse("local-name").unwrap();
    let service = ServiceId::parse("relationship-intelligence").unwrap();
    let channel = ChannelRef::new("-1004393614428");
    let scope = ScopeEvidence::from_record_store(
        ironworks_domain::OrgId::parse("measured-org").unwrap(),
        "http://records.invalid",
    );
    let subject = RuntimeSubject::new(
        RuntimeTenantId::parse("runtime-name").unwrap(),
        RuntimeUserId::parse("user_observed").unwrap(),
    );
    let adoption = AdoptionEvidence::observed(&scope, subject.clone());
    let store = LifecycleStore::open(&store_path).unwrap();
    store.adopt(&slug, &service, &channel, &adoption, &now()).unwrap();

    let definition = ironworks_services::service::load(service.as_str()).unwrap();
    assert_eq!(definition.capability_ceiling(), &CapabilityProfile::READ_ONLY_RECORDS);
    let policy = ConfinementPolicy::for_profile(definition.capability_ceiling());
    let binding = PolicyBinding::of(&definition, &policy);
    let bound = ironworks_lifecycle::BoundSubject::bind(
        ironworks_lifecycle::OpaqueId::new("user_observed"),
        subject,
    )
    .unwrap();
    let at = now();
    let states: BTreeMap<String, String> =
        [("builtin.http".into(), "disabled".into())].into_iter().collect();
    let measurement =
        Measurement::from_assessment(&policy, &bound, &states, &BTreeSet::new(), false, &at)
            .expect("complete catalogue");
    let attempt = ConfinementAttempt::measured(bound, binding.clone(), at, measurement);
    let evidence = ConfinementEvidence::from_attempt(attempt, &binding).unwrap();
    store.certify_confinement(&slug, &evidence).unwrap();
    drop(store);
    (store_path, registry_path, guidance_path)
}

fn publish(
    store: &Path,
    registry: &Path,
    guidance: &Path,
    records: &str,
    runtime: &str,
) -> std::process::Output {
    let model_pin =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../MODEL_PIN")).unwrap();
    let model_pin = model_pin.split('#').next().unwrap().trim();
    let policy_body = serde_json::json!({
        "selection_enabled": true,
        "workspace_default": model_pin,
        "models": [model_pin],
    })
    .to_string();
    let (policy, policy_thread) = serve("200 OK", &policy_body);
    let output = publish_command(store, registry, guidance, records, runtime)
        .env("IRONCLAW_API", policy)
        .env("IRONCLAW_OPERATOR_TOKEN", "operator-secret-sentinel")
        .output()
        .expect("run publish-one");
    let _ = policy_thread.join().unwrap();
    output
}

/// [`publish`], against a record store and a runtime that both answer as `prepared` recorded.
fn publish_agreeing(store: &Path, registry: &Path, guidance: &Path) -> std::process::Output {
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);
    let output = publish(store, registry, guidance, &records, &runtime);
    let _ = records_thread.join();
    let _ = runtime_thread.join();
    output
}

fn rollback_one(registry: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ironworks"))
        .args(["registry", "rollback-one", "--registry", registry.to_str().unwrap()])
        .output()
        .unwrap()
}

fn publish_command(
    store: &Path,
    registry: &Path,
    guidance: &Path,
    records: &str,
    runtime: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ironworks"));
    command
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .args([
            "registry",
            "publish-one",
            "local-name",
            "--store",
            store.to_str().unwrap(),
            "--registry",
            registry.to_str().unwrap(),
            "--guidance",
            guidance.to_str().unwrap(),
            "--records-credential",
            "env:PUBLISH_TEST_RECORDS",
            "--member-credential",
            "env:PUBLISH_TEST_MEMBER",
        ])
        .env("IRONWORKS_RECORDS", records)
        .env("IRONWORKS_BASE", runtime)
        .env("PUBLISH_TEST_RECORDS", "records-secret-sentinel")
        .env("PUBLISH_TEST_MEMBER", "member-secret-sentinel");
    command
}

fn prior_registry() -> &'static str {
    r#"{"tenants":[{"slug":"prior","channel":"-1001","org":"prior","service":"relationship-intelligence","guidance":"prior.md","records_credential":"env:PRIOR_RECORDS","runtime_credential":"env:PRIOR_RUNTIME"}]}"#
}

fn expected_candidate(guidance: &Path) -> Vec<u8> {
    let value = serde_json::json!({
        "tenants": [{
            "slug": "local-name",
            "channel": "-1004393614428",
            // The org the record store measured, not one the spec asserted: publication writes
            // what it proved, and `a_wrong_records_identity_cannot_change_the_live_registry`
            // is the control that a disagreeing store never reaches this file at all.
            "org": "measured-org",
            "service": "relationship-intelligence",
            "guidance": std::fs::canonicalize(guidance).unwrap().to_str().unwrap(),
            "records_credential": "env:PUBLISH_TEST_RECORDS",
            "runtime_credential": "env:PUBLISH_TEST_MEMBER",
        }]
    });
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn expected_legacy_candidate(guidance: &Path) -> Vec<u8> {
    let value = serde_json::json!({
        "tenants": [{
            "slug": "local-name",
            "channel": "-1004393614428",
            "service": "relationship-intelligence",
            "guidance": std::fs::canonicalize(guidance).unwrap().to_str().unwrap(),
            "records_credential": "env:PUBLISH_TEST_RECORDS",
            "runtime_credential": "env:PUBLISH_TEST_MEMBER",
        }]
    });
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn registry_id(bytes: &[u8]) -> ironworks_lifecycle::OpaqueId {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    ironworks_lifecycle::OpaqueId::new(
        digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect::<String>(),
    )
}

fn record_legacy_stage(store_path: &Path, guidance: &Path) {
    let store = LifecycleStore::open(store_path).unwrap();
    store
        .reach(
            &Slug::parse("local-name").unwrap(),
            Stage::Staged,
            IdentityProvenance::Created,
            Some(&registry_id(&expected_legacy_candidate(guidance))),
            &now(),
        )
        .unwrap();
}

fn append_clean_observation_for_another_policy(store_path: &Path) {
    let store = LifecycleStore::open(store_path).unwrap();
    let definition = ironworks_services::service::load("relationship-intelligence").unwrap();
    let policy = ConfinementPolicy::allowing(["builtin.time".to_owned()], false);
    let binding = PolicyBinding::of(&definition, &policy);
    let subject = RuntimeSubject::new(
        RuntimeTenantId::parse("runtime-name").unwrap(),
        RuntimeUserId::parse("user_observed").unwrap(),
    );
    let bound = ironworks_lifecycle::BoundSubject::bind(
        ironworks_lifecycle::OpaqueId::new("user_observed"),
        subject,
    )
    .unwrap();
    let at = ObservedAt::parse("2026-09-03T12:00:01+00:00").unwrap();
    let states = BTreeMap::from([
        ("builtin.http".to_owned(), "disabled".to_owned()),
        ("builtin.time".to_owned(), "always_allow".to_owned()),
    ]);
    let measurement =
        Measurement::from_assessment(&policy, &bound, &states, &BTreeSet::new(), false, &at)
            .expect("complete catalogue is clean under the alternate policy");
    let attempt = ConfinementAttempt::measured(bound, binding, at, measurement);
    store.record_attempt(&Slug::parse("local-name").unwrap(), &attempt).unwrap();
}

/// Guidance a tenant is committed to serving is checked where the commitment happens.
#[test]
fn publishing_unwritten_template_stubs_is_refused_before_external_checks() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let stubbed = format!(
        "<!-- tenant-guidance v1 slug: local-name service: relationship-intelligence -->\n\
         # Guidance\n\n## Vocabulary\n\nTO BE WRITTEN — what this organisation calls an \
         account.\n\n## Prohibited claims & actions\n\nTO BE WRITTEN — what it must never \
         claim.\n{}",
        "Answer only from supplied records and name every unresolved contradiction.\n".repeat(5)
    );
    std::fs::write(&guidance_path, stubbed).unwrap();
    let output = publish_command(
        &store_path,
        &registry_path,
        &guidance_path,
        "http://127.0.0.1:9",
        "http://127.0.0.1:9",
    )
    .output()
    .expect("run refused publish-one");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("guidance contains 2 unwritten template stub"),
        "publish-one must refuse the model-visible stub count: {stderr}"
    );
    // The refusal counts the stubs; it must not quote the tenant's own words back.
    assert!(!stderr.contains("what this organisation calls an account"), "{stderr}");
}

#[test]
fn publication_refuses_when_the_latest_clean_confinement_used_another_policy() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    append_clean_observation_for_another_policy(&store_path);

    let output = publish_command(
        &store_path,
        &registry_path,
        &guidance_path,
        "http://127.0.0.1:9",
        "http://127.0.0.1:9",
    )
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no latest clean confinement observation"), "{stderr}");
    assert!(!registry_path.exists(), "a stale confinement baseline published a route");
}

#[test]
fn publication_refuses_a_widened_requested_model_policy_before_writing() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let model_pin =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../MODEL_PIN")).unwrap();
    let model_pin = model_pin.split('#').next().unwrap().trim();
    let widened = serde_json::json!({
        "selection_enabled": true,
        "workspace_default": model_pin,
        "models": [model_pin, "provider/other-model"],
    })
    .to_string();
    let (policy, policy_thread) = serve("200 OK", &widened);

    let output = publish_command(
        &store_path,
        &registry_path,
        &guidance_path,
        "http://127.0.0.1:9",
        "http://127.0.0.1:9",
    )
    .env("IRONCLAW_API", policy)
    .env("IRONCLAW_OPERATOR_TOKEN", "operator-secret-sentinel")
    .output()
    .unwrap();
    let _ = policy_thread.join().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requested-model policy disagrees"), "{stderr}");
    assert!(!stderr.contains("provider/other-model"), "remote values reached diagnostics");
    assert!(!registry_path.exists(), "a widened model policy published a route");
}

#[test]
fn publish_and_rollback_are_accepted_by_the_serving_registry_loader() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    std::fs::write(&registry_path, prior_registry()).unwrap();
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        records_thread.join().unwrap().expect("records client").starts_with("GET /list_accounts ")
    );
    assert!(
        runtime_thread
            .join()
            .unwrap()
            .expect("runtime client")
            .starts_with("GET /api/webchat/v2/session ")
    );
    let live = TenantRegistry::parse(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(live.tenant(&Slug::parse("local-name").unwrap()).is_some());
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert!(
        store.progress(&Slug::parse("local-name").unwrap()).unwrap().has_reached(Stage::Staged)
    );
    assert!(
        store
            .recorded(&Slug::parse("local-name").unwrap())
            .unwrap()
            .identity(Authority::RegistryEntry)
            .is_some()
    );

    let rollback = Command::new(env!("CARGO_BIN_EXE_ironworks"))
        .args(["registry", "rollback-one", "--registry", registry_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(rollback.status.success(), "{}", String::from_utf8_lossy(&rollback.stderr));
    let restored =
        TenantRegistry::parse(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(restored.tenant(&Slug::parse("prior").unwrap()).is_some());

    let bytes = std::fs::read(&registry_path).unwrap();
    assert!(
        !bytes.windows("records-secret-sentinel".len()).any(|w| w == b"records-secret-sentinel")
    );
    assert!(!bytes.windows("member-secret-sentinel".len()).any(|w| w == b"member-secret-sentinel"));
}

#[test]
fn a_wrong_records_identity_cannot_change_the_live_registry() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    std::fs::write(&registry_path, prior_registry()).unwrap();
    let before = std::fs::read(&registry_path).unwrap();
    let (records, records_thread) = serve("200 OK", r#"{"org":"somebody-else","accounts":[]}"#);

    let output =
        publish(&store_path, &registry_path, &guidance_path, &records, "http://127.0.0.1:9");
    assert_eq!(output.status.code(), Some(2));
    records_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), before);
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert_eq!(
        store.progress(&Slug::parse("local-name").unwrap()).unwrap().reached(),
        Some(Stage::MemberConfined)
    );
}

#[test]
fn a_wrong_runtime_subject_cannot_change_the_live_registry() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    std::fs::write(&registry_path, prior_registry()).unwrap();
    let before = std::fs::read(&registry_path).unwrap();
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"wrong-tenant","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert_eq!(output.status.code(), Some(2));
    records_thread.join().unwrap();
    runtime_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), before);
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert_eq!(
        store.progress(&Slug::parse("local-name").unwrap()).unwrap().reached(),
        Some(Stage::MemberConfined)
    );
}

#[test]
fn rerun_after_file_publication_finishes_sql_without_replacing_the_rollback() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let candidate = expected_candidate(&guidance_path);
    std::fs::write(&registry_path, &candidate).unwrap();
    let rollback_path = registry_path.with_file_name("registry.json.previous");
    std::fs::write(&rollback_path, prior_registry()).unwrap();
    let rollback_before = std::fs::read(&rollback_path).unwrap();
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    records_thread.join().unwrap();
    runtime_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), candidate);
    assert_eq!(std::fs::read(&rollback_path).unwrap(), rollback_before);
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert!(
        store.progress(&Slug::parse("local-name").unwrap()).unwrap().has_reached(Stage::Staged)
    );
}

/// A recorded binding is never replaced by publish-one — not even with the candidate it recorded.
///
/// `registry rollback-one` does not walk lifecycle back, so the rerun finds the instance `staged`
/// against this very candidate while the live file holds whatever the rollback restored. Replacing
/// it there undoes the operator's rollback and reports that nothing changed. Both priors are
/// covered: a registry the rollback put back, and the absence a first publication rolls back to.
#[test]
fn rerunning_publish_one_after_a_rollback_refuses_and_writes_nothing() {
    for prior in [Some(prior_registry()), None] {
        let dir = dir();
        let (store_path, registry_path, guidance_path) = prepared(&dir);
        if let Some(prior) = prior {
            std::fs::write(&registry_path, prior).unwrap();
        }
        let published = publish_agreeing(&store_path, &registry_path, &guidance_path);
        assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));
        let rolled_back = rollback_one(&registry_path);
        assert!(rolled_back.status.success(), "{}", String::from_utf8_lossy(&rolled_back.stderr));

        let previous = registry_path.with_file_name("registry.json.previous");
        let live_before = std::fs::read(&registry_path).ok();
        let previous_before = std::fs::read(&previous).unwrap();
        assert_eq!(live_before.as_deref(), prior.map(str::as_bytes), "the rollback did not take");

        let rerun = publish_agreeing(&store_path, &registry_path, &guidance_path);
        let stderr = String::from_utf8_lossy(&rerun.stderr);
        assert_eq!(rerun.status.code(), Some(2), "{stderr}");
        let refusal = if prior.is_some() { "the live registry differs" } else { "cannot be read" };
        assert!(stderr.contains("already staged") && stderr.contains(refusal), "{stderr}");
        assert_eq!(std::fs::read(&registry_path).ok(), live_before);
        assert_eq!(std::fs::read(&previous).unwrap(), previous_before);
    }
}

/// The positive control for the refusal above: while the live file is still the recorded
/// candidate, a rerun succeeds — and rewrites neither the live file nor its rollback.
#[test]
fn rerunning_publish_one_over_its_own_unchanged_file_succeeds_without_writing() {
    use std::os::unix::fs::MetadataExt;
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    std::fs::write(&registry_path, prior_registry()).unwrap();
    let published = publish_agreeing(&store_path, &registry_path, &guidance_path);
    assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));

    let previous = registry_path.with_file_name("registry.json.previous");
    // Bytes and inode both: identical bytes renamed over the same name are still a rewrite.
    let snapshot =
        |path: &Path| (std::fs::read(path).unwrap(), std::fs::metadata(path).unwrap().ino());
    let (live_before, previous_before) = (snapshot(&registry_path), snapshot(&previous));

    let rerun = publish_agreeing(&store_path, &registry_path, &guidance_path);
    assert!(rerun.status.success(), "{}", String::from_utf8_lossy(&rerun.stderr));
    let stdout = String::from_utf8_lossy(&rerun.stdout);
    assert!(stdout.contains("already staged; live registry matches"), "{stdout}");
    assert_eq!(snapshot(&registry_path), live_before);
    assert_eq!(snapshot(&previous), previous_before);
}

#[test]
fn exact_former_candidate_migrates_file_then_recorded_binding() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let legacy = expected_legacy_candidate(&guidance_path);
    let current = expected_candidate(&guidance_path);
    std::fs::write(&registry_path, &legacy).unwrap();
    record_legacy_stage(&store_path, &guidance_path);
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    records_thread.join().unwrap();
    runtime_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), current);
    assert_eq!(
        std::fs::read(registry_path.with_file_name("registry.json.previous")).unwrap(),
        b"ironworks-registry-prior-absent-v1\n"
    );
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert_eq!(
        store
            .recorded(&Slug::parse("local-name").unwrap())
            .unwrap()
            .accounted_id(Authority::RegistryEntry)
            .unwrap(),
        &registry_id(&expected_candidate(&guidance_path))
    );
    assert_eq!(
        store
            .history(&Slug::parse("local-name").unwrap())
            .unwrap()
            .iter()
            .filter(|(stage, _, _)| *stage == Stage::Staged)
            .count(),
        1
    );
}

#[test]
fn byte_different_legacy_file_is_refused_unchanged() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let mut different = expected_legacy_candidate(&guidance_path);
    different.push(b'\n');
    std::fs::write(&registry_path, &different).unwrap();
    record_legacy_stage(&store_path, &guidance_path);
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert_eq!(output.status.code(), Some(2));
    records_thread.join().unwrap();
    runtime_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), different);
    assert!(!registry_path.with_file_name("registry.json.previous").exists());
}

#[test]
fn file_before_ledger_crash_is_completed_without_replacing_valid_rollback() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let current = expected_candidate(&guidance_path);
    let prior = prior_registry().as_bytes();
    std::fs::write(&registry_path, &current).unwrap();
    let rollback = registry_path.with_file_name("registry.json.previous");
    std::fs::write(&rollback, prior).unwrap();
    record_legacy_stage(&store_path, &guidance_path);
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    records_thread.join().unwrap();
    runtime_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), current);
    assert_eq!(std::fs::read(&rollback).unwrap(), prior);
    let store = LifecycleStore::open_readonly(&store_path).unwrap();
    assert_eq!(
        store
            .recorded(&Slug::parse("local-name").unwrap())
            .unwrap()
            .accounted_id(Authority::RegistryEntry)
            .unwrap(),
        &registry_id(&expected_candidate(&guidance_path))
    );
}

#[test]
fn migration_still_requires_live_authority_to_match() {
    let dir = dir();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    let legacy = expected_legacy_candidate(&guidance_path);
    std::fs::write(&registry_path, &legacy).unwrap();
    record_legacy_stage(&store_path, &guidance_path);
    let (records, records_thread) = serve("200 OK", r#"{"org":"somebody-else","accounts":[]}"#);

    let output =
        publish(&store_path, &registry_path, &guidance_path, &records, "http://127.0.0.1:9");
    assert_eq!(output.status.code(), Some(2));
    records_thread.join().unwrap();
    assert_eq!(std::fs::read(&registry_path).unwrap(), legacy);
}
