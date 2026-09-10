mod common;
use common::serve;
use ironworks_domain::Slug;
use ironworks_lifecycle::{Authority, IdentityProvenance, LifecycleStore, Stage};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A temporary directory. `ironworks_testkit::Scratch`, aliased so the tests read as they did —
/// this was one of six copies, already drifted between `path()` and `.0` and between `expect` and
/// `unwrap`.
type Dir = ironworks_testkit::Scratch;

fn dir() -> Dir {
    Dir::new("adopt")
}

fn adopt(store: &Path, records: &str, runtime: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ironworks"))
        .args([
            "lifecycle",
            "adopt",
            "local-name",
            "--store",
            store.to_str().expect("utf8 path"),
            "--service",
            "relationship-intelligence",
            "--channel",
            "-1004393614428",
            "--records-credential",
            "env:ADOPT_TEST_RECORDS",
            "--member-credential",
            "env:ADOPT_TEST_MEMBER",
        ])
        .env("IRONWORKS_RECORDS", records)
        .env("IRONCLAW_API", runtime)
        .env("ADOPT_TEST_RECORDS", "records-secret-sentinel")
        .env("ADOPT_TEST_MEMBER", "member-secret-sentinel")
        .output()
        .expect("run ironworks")
}

#[test]
fn credential_backed_adoption_records_both_namespaces_without_equating_them() {
    let dir = dir();
    let store_path = dir.path().join("lifecycle.store");
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) =
        serve("200 OK", r#"{"tenant_id":"runtime-name","user_id":"user_observed"}"#);

    let output = adopt(&store_path, &records, &runtime);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let records_request = records_thread.join().expect("records fixture").expect("records client");
    let runtime_request = runtime_thread.join().expect("runtime fixture").expect("runtime client");
    assert!(records_request.starts_with("GET /list_accounts "));
    assert!(runtime_request.starts_with("GET /api/webchat/v2/session "));

    let slug = Slug::parse("local-name").unwrap();
    let store = LifecycleStore::open_readonly(&store_path).expect("read adopted store");
    let spec = store.spec(&slug).unwrap().expect("instance");
    assert_eq!(spec.org.as_str(), "measured-org");
    assert!(store.progress(&slug).unwrap().has_reached(Stage::MemberMinted));
    let recorded = store.recorded(&slug).unwrap();
    let member = recorded.identity(Authority::Member).expect("member identity");
    assert_eq!(member.provenance(), IdentityProvenance::Adopted);
    let subject = member.runtime_subject().expect("measured runtime subject");
    assert_eq!(subject.tenant().as_str(), "runtime-name");
    assert_eq!(subject.user().as_str(), "user_observed");
    assert_eq!(store.created(&slug).unwrap().created_id(Authority::Member), None);

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("records-secret-sentinel"));
    assert!(!combined.contains("member-secret-sentinel"));
    for path in [
        store_path.clone(),
        PathBuf::from(format!("{}-wal", store_path.display())),
        PathBuf::from(format!("{}-shm", store_path.display())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            assert!(
                !bytes
                    .windows(b"records-secret-sentinel".len())
                    .any(|w| { w == b"records-secret-sentinel" })
            );
            assert!(
                !bytes
                    .windows(b"member-secret-sentinel".len())
                    .any(|w| { w == b"member-secret-sentinel" })
            );
        }
    }
}

#[test]
fn an_unanswered_session_leaves_no_partial_instance() {
    let dir = dir();
    let store_path = dir.path().join("lifecycle.store");
    let (records, records_thread) = serve("200 OK", r#"{"org":"measured-org","accounts":[]}"#);
    let (runtime, runtime_thread) = serve("503 Service Unavailable", r#"{"error":"down"}"#);

    let output = adopt(&store_path, &records, &runtime);
    assert_eq!(output.status.code(), Some(3));
    records_thread.join().expect("records fixture");
    runtime_thread.join().expect("runtime fixture");

    let store = LifecycleStore::open_readonly(&store_path).expect("schema still readable");
    let slug = Slug::parse("local-name").unwrap();
    assert_eq!(store.spec(&slug).unwrap(), None);
    assert!(store.history(&slug).unwrap().is_empty());
}
