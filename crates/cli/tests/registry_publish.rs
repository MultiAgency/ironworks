use ironworks_domain::{
    CapabilityProfile, ChannelRef, ScopeEvidence, ServiceId, Slug, TenantRegistry,
};
use ironworks_lifecycle::{
    AdoptionEvidence, Authority, ConfinementAttempt, ConfinementEvidence, ConfinementPolicy,
    LifecycleStore, Measurement, ObservedAt, PolicyBinding, RuntimeSubject, RuntimeTenantId,
    RuntimeUserId, Stage,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const NOW: &str = "2026-09-03T12:00:00+00:00";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ironworks-publish-one-{}-{}",
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

fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let address = listener.local_addr().expect("address");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = vec![0; 8192];
        let read = stream.read(&mut request).expect("read");
        stream.write_all(response.as_bytes()).expect("respond");
        String::from_utf8_lossy(&request[..read]).into_owned()
    });
    (format!("http://{address}"), thread)
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
    store.adopt(&slug, &service, &channel, &adoption, NOW).unwrap();

    let definition = ironworks_services::service::load(service.as_str()).unwrap();
    assert_eq!(definition.capability_ceiling(), &CapabilityProfile::READ_ONLY_RECORDS);
    let policy = ConfinementPolicy::for_profile(definition.capability_ceiling());
    let binding = PolicyBinding::of(&definition, &policy);
    let bound = ironworks_lifecycle::BoundSubject::bind(
        ironworks_lifecycle::OpaqueId::new("user_observed"),
        subject,
    )
    .unwrap();
    let at = ObservedAt::parse(NOW).unwrap();
    let states: BTreeMap<String, String> =
        [("builtin.http".into(), "disabled".into())].into_iter().collect();
    let measurement =
        Measurement::from_assessment(&policy, &bound, &states, &BTreeSet::new(), false, &at);
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
    Command::new(env!("CARGO_BIN_EXE_ironworks"))
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
        .env("PUBLISH_TEST_MEMBER", "member-secret-sentinel")
        .output()
        .expect("run publish-one")
}

fn prior_registry() -> &'static str {
    r#"{"tenants":[{"slug":"prior","channel":"-1001","service":"relationship-intelligence","guidance":"prior.md","records_credential":"env:PRIOR_RECORDS","runtime_credential":"env:PRIOR_RUNTIME"}]}"#
}

fn expected_candidate(guidance: &Path) -> Vec<u8> {
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

#[test]
fn publish_and_rollback_are_accepted_by_the_serving_registry_loader() {
    let dir = Dir::new();
    let (store_path, registry_path, guidance_path) = prepared(&dir);
    std::fs::write(&registry_path, prior_registry()).unwrap();
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = publish(&store_path, &registry_path, &guidance_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(records_thread.join().unwrap().starts_with("GET /list_accounts "));
    assert!(runtime_thread.join().unwrap().starts_with("GET /api/webchat/v2/session "));
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
    let dir = Dir::new();
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
    let dir = Dir::new();
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
    let dir = Dir::new();
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
