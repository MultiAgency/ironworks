use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// A temporary directory. `ironworks_testkit::Scratch`, aliased so the tests read as they did —
/// this was one of six copies, already drifted between `path()` and `.0` and between `expect` and
/// `unwrap`.
type Dir = ironworks_testkit::Scratch;

fn dir() -> Dir {
    Dir::new("guidance")
}

fn guidance(body: &str) -> String {
    format!("<!-- tenant-guidance v1 slug: acme service: relationship-intelligence -->\n{body}")
}

fn registry(path: &Path, guidance: &Path) {
    let value = serde_json::json!({
        "tenants": [{
            "slug": "acme",
            "channel": "-1001",
            "org": "acme",
            "service": "relationship-intelligence",
            "guidance": guidance,
            "records_credential": "env:ACME_RECORDS",
            "runtime_credential": "env:ACME_RUNTIME"
        }]
    });
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn command(operation: &str, registry: &Path, source: Option<&Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ironworks"));
    command.args(["guidance", operation, "acme", "--registry", registry.to_str().unwrap()]);
    if let Some(source) = source {
        command.args(["--source", source.to_str().unwrap()]);
    }
    command
}

#[test]
fn clean_guidance_publishes_privately_and_rolls_back_without_printing_content() {
    let dir = dir();
    let registry_path = dir.join("registry.json");
    let live = dir.join("guidance.md");
    let source = dir.join("candidate.md");
    let prior = guidance(&"Prior tenant-specific operating guidance. ".repeat(12));
    let candidate = guidance(&"Replacement tenant-specific operating guidance. ".repeat(12));
    std::fs::write(&live, &prior).unwrap();
    std::fs::write(&source, &candidate).unwrap();
    registry(&registry_path, &live);

    let published = command("publish-one", &registry_path, Some(&source)).output().unwrap();
    assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));
    assert_eq!(std::fs::read_to_string(&live).unwrap(), candidate);
    assert_eq!(
        std::fs::read_to_string(live.with_file_name("guidance.md.previous")).unwrap(),
        prior
    );
    assert_eq!(std::fs::metadata(&live).unwrap().permissions().mode() & 0o777, 0o600);
    let report = String::from_utf8(published.stdout).unwrap();
    assert!(report.starts_with("bytes=") && report.contains(" sha256="), "{report}");
    assert!(!report.contains("Replacement") && !report.contains(live.to_str().unwrap()));

    let rolled_back = command("rollback-one", &registry_path, None).output().unwrap();
    assert!(rolled_back.status.success(), "{}", String::from_utf8_lossy(&rolled_back.stderr));
    assert_eq!(std::fs::read_to_string(&live).unwrap(), prior);
    assert_eq!(
        String::from_utf8(rolled_back.stdout).unwrap(),
        format!("bytes={} sha256={}\n", prior.len(), sha256(prior.as_bytes()))
    );
}

/// Rolling back a first publication removes the live guidance, and the output says so.
///
/// The prior state was no file at all, which is what the absence marker in `.previous` restores.
/// Reported as `bytes=0` and the digest of nothing, that read exactly like restoring an empty file
/// — the one rollback that decides whether a tenant has guidance at all was the one an operator
/// could not tell apart. The ordinary restore above is its positive control.
#[test]
fn rolling_back_a_first_publication_reports_the_removal_rather_than_an_empty_file() {
    let dir = dir();
    let registry_path = dir.join("registry.json");
    let live = dir.join("guidance.md");
    let source = dir.join("candidate.md");
    std::fs::write(&source, guidance(&"First tenant-specific operating guidance. ".repeat(12)))
        .unwrap();
    registry(&registry_path, &live);

    let published = command("publish-one", &registry_path, Some(&source)).output().unwrap();
    assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));
    assert!(live.exists());

    let rolled_back = command("rollback-one", &registry_path, None).output().unwrap();
    assert!(rolled_back.status.success(), "{}", String::from_utf8_lossy(&rolled_back.stderr));
    assert!(!live.exists(), "rollback of a first publication must leave no live guidance");
    assert_eq!(
        String::from_utf8(rolled_back.stdout).unwrap(),
        "removed: the rollback restores no live guidance\n"
    );
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

#[test]
fn each_known_hazard_is_refused_while_clean_guidance_is_the_positive_control() {
    let dir = dir();
    let registry_path = dir.join("registry.json");
    let live = dir.join("guidance.md");
    let source = dir.join("candidate.md");
    let prior = guidance(&"Prior tenant-specific operating guidance. ".repeat(12));
    std::fs::write(&live, &prior).unwrap();
    registry(&registry_path, &live);

    for body in [
        format!("operator material-->\n{}", "Tenant operating guidance. ".repeat(20)),
        format!("TO BE WRITTEN\n{}", "Shared template method prose. ".repeat(20)),
    ] {
        std::fs::write(&source, guidance(&body)).unwrap();
        let refused = command("publish-one", &registry_path, Some(&source)).output().unwrap();
        assert_eq!(refused.status.code(), Some(2));
        assert_eq!(std::fs::read_to_string(&live).unwrap(), prior);
        assert!(String::from_utf8_lossy(&refused.stderr).contains("guidance contains"));
    }

    std::fs::write(&source, guidance(&"Tenant operating guidance. ".repeat(20))).unwrap();
    let clean = command("publish-one", &registry_path, Some(&source)).output().unwrap();
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stderr));
}

#[test]
fn rollback_refuses_a_hazardous_legacy_prior_and_keeps_the_clean_live_file() {
    let dir = dir();
    let registry_path = dir.join("registry.json");
    let live = dir.join("guidance.md");
    let source = dir.join("candidate.md");
    let hazardous = guidance(&format!("TO BE WRITTEN\n{}", "Legacy guidance. ".repeat(30)));
    let clean = guidance(&"Current tenant-specific operating guidance. ".repeat(12));
    std::fs::write(&live, &hazardous).unwrap();
    std::fs::write(&source, &clean).unwrap();
    registry(&registry_path, &live);

    let published = command("publish-one", &registry_path, Some(&source)).output().unwrap();
    assert!(published.status.success(), "{}", String::from_utf8_lossy(&published.stderr));
    assert_eq!(std::fs::read_to_string(&live).unwrap(), clean);

    let rolled_back = command("rollback-one", &registry_path, None).output().unwrap();
    assert_eq!(rolled_back.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rolled_back.stderr).contains("guidance contains"));
    assert_eq!(std::fs::read_to_string(&live).unwrap(), clean);
}
