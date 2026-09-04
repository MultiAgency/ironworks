//! Durable publication of the one-tenant registry used by the attended cutover.
//!
//! The live file and its rollback artifact are both replaced through a same-directory temporary
//! file. File contents are synced first; the containing directory is synced before and after each
//! rename. A successful return therefore means the name-to-inode relationship, not merely the
//! bytes in page cache, survived the durability boundary.

use ironworks_domain::TenantRegistry;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PRIOR_ABSENT: &[u8] = b"rustworks-registry-prior-absent-v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Published { preserved_prior: bool },
    AlreadyPublished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    CandidateFileSynced,
    RollbackFileSynced,
    DirectorySyncedBeforeRollbackRename,
    RollbackRenamed,
    DirectorySyncedAfterRollbackRename,
    DirectorySyncedBeforePublishRename,
    PublishedRenamed,
    DirectorySyncedAfterPublishRename,
}

struct TempFile {
    path: PathBuf,
    keep: bool,
}

impl TempFile {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn renamed(&mut self) {
        self.keep = true;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn rollback_path(live: &Path) -> Result<PathBuf, String> {
    let name = live
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("registry path {} has no usable file name", live.display()))?;
    Ok(live.with_file_name(format!("{name}.previous")))
}

pub fn publish_one(live: &Path, candidate: &[u8]) -> Result<PublishOutcome, String> {
    let mut ignore = |_| {};
    publish_one_traced(live, candidate, &mut ignore)
}

fn publish_one_traced(
    live: &Path,
    candidate: &[u8],
    trace: &mut impl FnMut(Event),
) -> Result<PublishOutcome, String> {
    validate_one(candidate, "candidate")?;
    let parent = parent(live);
    if !parent.is_dir() {
        return Err(format!(
            "registry directory {} does not exist; installation must create it before publication",
            parent.display()
        ));
    }

    let prior = match fs::read(live) {
        Ok(prior) => Some(prior),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read live registry {}: {e}", live.display())),
    };

    if prior.as_deref() == Some(candidate) {
        let rollback = rollback_path(live)?;
        let rollback_bytes = fs::read(&rollback).map_err(|e| {
            format!(
                "live registry already matches but its durable rollback {} is unavailable: {e}",
                rollback.display()
            )
        })?;
        validate_rollback(&rollback_bytes)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(live)
            .map_err(|e| format!("cannot reopen live registry {}: {e}", live.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict live registry {}: {e}", live.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot sync live registry {}: {e}", live.display()))?;
        sync_directory(parent)?;
        return Ok(PublishOutcome::AlreadyPublished);
    }

    if let Some(prior) = prior.as_deref() {
        validate_one(prior, "prior live registry")?;
    }

    let mut candidate_temp = write_temp(live, candidate)?;
    trace(Event::CandidateFileSynced);

    // Absence is a prior state too. The one durable artifact is either the exact prior registry
    // or a versioned marker instructing rollback to restore no live file. A first deployment is
    // therefore just as reversible as a replacement.
    let rollback = rollback_path(live)?;
    let rollback_bytes = prior.as_deref().unwrap_or(PRIOR_ABSENT);
    let mut rollback_temp = write_temp(&rollback, rollback_bytes)?;
    trace(Event::RollbackFileSynced);
    sync_directory(parent)?;
    trace(Event::DirectorySyncedBeforeRollbackRename);
    fs::rename(&rollback_temp.path, &rollback).map_err(|e| {
        format!(
            "cannot publish rollback registry {} over {}: {e}",
            rollback_temp.path.display(),
            rollback.display()
        )
    })?;
    rollback_temp.renamed();
    trace(Event::RollbackRenamed);
    sync_directory(parent)?;
    trace(Event::DirectorySyncedAfterRollbackRename);

    sync_directory(parent)?;
    trace(Event::DirectorySyncedBeforePublishRename);
    fs::rename(&candidate_temp.path, live).map_err(|e| {
        format!(
            "cannot publish candidate registry {} over {}: {e}",
            candidate_temp.path.display(),
            live.display()
        )
    })?;
    candidate_temp.renamed();
    trace(Event::PublishedRenamed);
    sync_directory(parent)?;
    trace(Event::DirectorySyncedAfterPublishRename);
    Ok(PublishOutcome::Published { preserved_prior: prior.is_some() })
}

pub fn rollback_one(live: &Path) -> Result<(), String> {
    let rollback = rollback_path(live)?;
    let prior = fs::read(&rollback)
        .map_err(|e| format!("cannot read rollback registry {}: {e}", rollback.display()))?;
    let parent = parent(live);
    if prior == PRIOR_ABSENT {
        if let Ok(file) = OpenOptions::new().read(true).open(live) {
            file.sync_all()
                .map_err(|e| format!("cannot sync live registry {}: {e}", live.display()))?;
        }
        sync_directory(parent)?;
        match fs::remove_file(live) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove live registry {}: {e}", live.display())),
        }
        sync_directory(parent)?;
        return Ok(());
    }
    validate_rollback(&prior)?;

    // Copy rather than consume `.previous`: a rollback remains repeatable after a restart. The
    // same durable replacement primitive writes, syncs, directory-syncs, renames and syncs again.
    let mut temp = write_temp(live, &prior)?;
    sync_directory(parent)?;
    fs::rename(&temp.path, live).map_err(|e| {
        format!(
            "cannot restore rollback registry {} over {}: {e}",
            temp.path.display(),
            live.display()
        )
    })?;
    temp.renamed();
    sync_directory(parent)?;
    Ok(())
}

fn validate_one(bytes: &[u8], what: &str) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("{what} is not UTF-8: {e}"))?;
    let registry = TenantRegistry::parse(text).map_err(|e| format!("{what} is invalid: {e}"))?;
    if registry.len() != 1 {
        return Err(format!(
            "{what} contains {} tenants; publish-one requires exactly one",
            registry.len()
        ));
    }
    Ok(())
}

fn validate_rollback(bytes: &[u8]) -> Result<(), String> {
    if bytes == PRIOR_ABSENT { Ok(()) } else { validate_one(bytes, "rollback registry") }
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."))
}

fn write_temp(target: &Path, bytes: &[u8]) -> Result<TempFile, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = parent(target);
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("registry");
    for _ in 0..100 {
        let path = parent.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!("cannot create temporary registry {}: {e}", path.display()));
            }
        };
        let temp = TempFile::new(path.clone());
        file.write_all(bytes)
            .map_err(|e| format!("cannot write temporary registry {}: {e}", path.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot sync temporary registry {}: {e}", path.display()))?;
        return Ok(temp);
    }
    Err(format!("cannot allocate a temporary registry beside {}", target.display()))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("cannot sync registry directory {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironworks_domain::{ChannelRef, Slug};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ironworks-registry-publish-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn registry(slug: &str, channel: &str) -> Vec<u8> {
        format!(
            r#"{{"tenants":[{{"slug":"{slug}","channel":"{channel}","service":"relationship-intelligence","guidance":"guidance.md","records_credential":"env:RECORDS","runtime_credential":"env:RUNTIME"}}]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn publication_syncs_the_rollback_before_replacing_live() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        fs::write(&live, registry("prior", "-1001")).unwrap();
        let mut events = Vec::new();

        publish_one_traced(&live, &registry("candidate", "-1002"), &mut |e| events.push(e))
            .unwrap();

        assert_eq!(
            events,
            vec![
                Event::CandidateFileSynced,
                Event::RollbackFileSynced,
                Event::DirectorySyncedBeforeRollbackRename,
                Event::RollbackRenamed,
                Event::DirectorySyncedAfterRollbackRename,
                Event::DirectorySyncedBeforePublishRename,
                Event::PublishedRenamed,
                Event::DirectorySyncedAfterPublishRename,
            ]
        );
    }

    #[test]
    fn publish_then_rollback_restores_the_prior_registry_to_the_serving_loader() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        let prior = registry("prior", "-1001");
        fs::write(&live, &prior).unwrap();

        assert_eq!(
            publish_one(&live, &registry("candidate", "-1002")).unwrap(),
            PublishOutcome::Published { preserved_prior: true }
        );
        let published = TenantRegistry::parse(&fs::read_to_string(&live).unwrap()).unwrap();
        assert!(published.tenant(&Slug::parse("candidate").unwrap()).is_some());

        rollback_one(&live).unwrap();
        let restored = TenantRegistry::parse(&fs::read_to_string(&live).unwrap()).unwrap();
        assert!(restored.tenant(&Slug::parse("prior").unwrap()).is_some());
        assert_eq!(
            restored.claims(),
            vec![(ChannelRef::new("-1001"), Slug::parse("prior").unwrap())]
        );
        assert_eq!(fs::read(&live).unwrap(), prior);
        assert_eq!(fs::metadata(&live).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(rollback_path(&live).unwrap().exists(), "rollback was consumed");
    }

    #[test]
    fn rerunning_an_already_published_candidate_preserves_the_real_rollback() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        let prior = registry("prior", "-1001");
        let candidate = registry("candidate", "-1002");
        fs::write(&live, &prior).unwrap();
        publish_one(&live, &candidate).unwrap();
        let rollback_before = fs::read(rollback_path(&live).unwrap()).unwrap();

        assert_eq!(publish_one(&live, &candidate).unwrap(), PublishOutcome::AlreadyPublished);
        assert_eq!(fs::read(rollback_path(&live).unwrap()).unwrap(), rollback_before);
    }

    #[test]
    fn a_matching_live_file_without_a_durable_rollback_is_not_accepted_as_published() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        let candidate = registry("candidate", "-1002");
        fs::write(&live, &candidate).unwrap();

        assert!(publish_one(&live, &candidate).is_err());
        assert_eq!(fs::read(&live).unwrap(), candidate);
    }

    #[test]
    fn an_invalid_prior_is_never_promoted_to_the_rollback_slot() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        fs::write(&live, b"not a registry").unwrap();
        assert!(publish_one(&live, &registry("candidate", "-1002")).is_err());
        assert_eq!(fs::read(&live).unwrap(), b"not a registry");
        assert!(!rollback_path(&live).unwrap().exists());
    }

    #[test]
    fn first_publication_rolls_back_to_no_live_registry() {
        let dir = Dir::new();
        let live = dir.0.join("registry.json");
        publish_one(&live, &registry("candidate", "-1002")).unwrap();
        assert!(live.exists());
        assert_eq!(fs::read(rollback_path(&live).unwrap()).unwrap(), PRIOR_ABSENT);

        rollback_one(&live).unwrap();
        assert!(!live.exists(), "rollback did not restore the prior absence");
        assert!(rollback_path(&live).unwrap().exists(), "rollback marker was consumed");
    }
}
