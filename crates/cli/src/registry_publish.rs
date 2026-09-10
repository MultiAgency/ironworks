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

const PRIOR_ABSENT: &[u8] = b"ironworks-registry-prior-absent-v1\n";
const ARTIFACT_PRIOR_ABSENT: &[u8] = b"ironworks-artifact-prior-absent-v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Published { preserved_prior: bool },
    AlreadyPublished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    Migrated { rebuilt_rollback_to_absence: bool },
    AlreadyMigrated { rebuilt_rollback_to_absence: bool },
}

/// A crash-released advisory lock for every operation on one live registry and its rollback.
///
/// File publication is only exposed through this guard. The CLI keeps it alive until the
/// corresponding lifecycle transaction commits, making two operator processes serialize the
/// otherwise unavoidable file/SQLite crash window.
pub struct RegistryLock {
    live: PathBuf,
    _file: File,
}

impl RegistryLock {
    pub fn acquire(live: &Path) -> Result<Self, String> {
        let parent = parent(live);
        if !parent.is_dir() {
            return Err(format!(
                "registry directory {} does not exist; installation must create it first",
                parent.display()
            ));
        }
        let path = lock_path(live)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("cannot open registry lock {}: {e}", path.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict registry lock {}: {e}", path.display()))?;
        file.lock().map_err(|e| format!("cannot lock registry {}: {e}", live.display()))?;
        Ok(Self { live: live.to_owned(), _file: file })
    }

    pub fn publish_one(&mut self, candidate: &[u8]) -> Result<PublishOutcome, String> {
        let mut ignore = |_| {};
        publish_durably(REGISTRY, &self.live, candidate, &mut ignore)
    }

    pub fn migrate_legacy_one(
        &mut self,
        legacy: &[u8],
        current: &[u8],
    ) -> Result<MigrationOutcome, String> {
        let mut ignore = |_| {};
        migrate_legacy_one_traced(&self.live, legacy, current, &mut ignore)
    }

    pub fn rollback_one(&mut self) -> Result<(), String> {
        rollback_durably(REGISTRY, &self.live)
    }

    /// Read the live registry while still holding the publication lock.
    ///
    /// For the caller that must compare rather than publish — an already-recorded binding is never
    /// replaced — so the comparison is still true when it acts on the answer.
    pub fn live_bytes(&self) -> std::io::Result<Vec<u8>> {
        fs::read(&self.live)
    }

    /// Publish an opaque companion artifact while holding the registry's publication lock.
    ///
    /// Guidance uses this rather than an independent lock: a registry publication and a guidance
    /// publication for the same cell cannot race through two different operator processes. The
    /// bytes are validated by the caller before this boundary; this layer owns only atomicity,
    /// custody, rollback, and durability.
    pub fn publish_artifact(
        &mut self,
        target: &Path,
        candidate: &[u8],
    ) -> Result<PublishOutcome, String> {
        let mut ignore = |_| {};
        publish_durably(ARTIFACT, target, candidate, &mut ignore)
    }

    /// Read the artifact that rollback would restore while still holding the publication lock.
    ///
    /// `None` is the explicit prior-absence marker. Callers that attach meaning to the bytes must
    /// validate `Some` before asking this module to perform the opaque durable replacement.
    pub fn rollback_artifact_bytes(&self, target: &Path) -> Result<Option<Vec<u8>>, String> {
        let rollback = rollback_path(target)?;
        let prior = fs::read(&rollback).map_err(|error| {
            format!("cannot read rollback artifact {}: {error}", rollback.display())
        })?;
        Ok((prior != ARTIFACT_PRIOR_ABSENT).then_some(prior))
    }

    /// Restore an opaque companion artifact without consuming its `.previous` evidence.
    pub fn rollback_artifact(&mut self, target: &Path) -> Result<(), String> {
        rollback_durably(ARTIFACT, target)
    }
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

pub struct TempFile {
    path: PathBuf,
    keep: bool,
}

impl TempFile {
    /// Where the staged bytes are, so a caller can rename them into place.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    pub fn renamed(&mut self) {
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

/// The file beside `live` carrying `suffix`. `rollback_path` and `lock_path` were this function
/// written out twice, differing only in the four characters of the suffix.
fn sibling(live: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = live
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("path {} has no usable file name", live.display()))?;
    Ok(live.with_file_name(format!("{name}{suffix}")))
}

/// Where the bytes a rollback would restore are kept.
pub fn rollback_path(live: &Path) -> Result<PathBuf, String> {
    sibling(live, ".previous")
}

/// Replace exactly one registry encoding emitted by the former no-`org` publisher.
///
/// `legacy` is not parsed permissively: the caller constructs the one historical byte string it
/// is prepared to migrate, and the live file must equal it byte-for-byte. A rollback that the
/// current strict loader can still read is preserved. A missing or legacy rollback cannot be
/// advertised as usable, so it is durably replaced with the explicit prior-absence marker.
fn migrate_legacy_one_traced(
    live: &Path,
    legacy: &[u8],
    current: &[u8],
    trace: &mut impl FnMut(Event),
) -> Result<MigrationOutcome, String> {
    validate_one(current, "current candidate")?;
    if legacy == current {
        return Err("legacy and current registry candidates are identical".into());
    }
    let parent = parent(live);
    if !parent.is_dir() {
        return Err(format!(
            "registry directory {} does not exist; installation must create it before migration",
            parent.display()
        ));
    }
    let live_bytes =
        fs::read(live).map_err(|e| format!("cannot read live registry {}: {e}", live.display()))?;
    let already = live_bytes == current;
    if !already && live_bytes != legacy {
        return Err(format!(
            "live registry {} is neither the exact former candidate nor the current candidate; refusing migration",
            live.display()
        ));
    }

    let rollback = rollback_path(live)?;
    let rollback_valid =
        fs::read(&rollback).ok().is_some_and(|bytes| validate_rollback(&bytes).is_ok());
    if !rollback_valid {
        let mut rollback_temp = write_temp(REGISTRY.noun, &rollback, PRIOR_ABSENT)?;
        trace(Event::RollbackFileSynced);
        sync_directory(REGISTRY.noun, parent)?;
        trace(Event::DirectorySyncedBeforeRollbackRename);
        fs::rename(&rollback_temp.path, &rollback).map_err(|e| {
            format!(
                "cannot publish migration rollback {} over {}: {e}",
                rollback_temp.path.display(),
                rollback.display()
            )
        })?;
        rollback_temp.renamed();
        trace(Event::RollbackRenamed);
        sync_directory(REGISTRY.noun, parent)?;
        trace(Event::DirectorySyncedAfterRollbackRename);
    }

    if already {
        resync_live(REGISTRY, live)?;
        return Ok(MigrationOutcome::AlreadyMigrated {
            rebuilt_rollback_to_absence: !rollback_valid,
        });
    }

    let mut candidate_temp = write_temp(REGISTRY.noun, live, current)?;
    trace(Event::CandidateFileSynced);
    sync_directory(REGISTRY.noun, parent)?;
    trace(Event::DirectorySyncedBeforePublishRename);
    fs::rename(&candidate_temp.path, live).map_err(|e| {
        format!(
            "cannot publish migrated registry {} over {}: {e}",
            candidate_temp.path.display(),
            live.display()
        )
    })?;
    candidate_temp.renamed();
    trace(Event::PublishedRenamed);
    sync_directory(REGISTRY.noun, parent)?;
    trace(Event::DirectorySyncedAfterPublishRename);
    Ok(MigrationOutcome::Migrated { rebuilt_rollback_to_absence: !rollback_valid })
}

/// What this module knows how to check about a set of bytes, and what to call them if it does not.
type Validator = fn(&[u8], &str) -> Result<(), String>;

/// What a durable replacement is replacing.
///
/// # One protocol, and it was written out twice
///
/// Publish is read prior → write both temps → sync → rename the rollback → sync → rename the
/// candidate → sync, and rollback is its inverse. Both existed once for the tenant registry and
/// once for its companion artifacts. Only the registry copy took the `trace` parameter, so
/// `every_migration_durability_boundary_leaves_a_recoverable_state` — the test that proves each
/// fsync boundary leaves a recoverable state — exercised the registry path and not the artifact
/// path. Since ADR 0049 the artifact path is the one a **guidance publication** takes, which is
/// the path that changes what is served.
///
/// Three things differ between them, and they are the three fields.
#[derive(Clone, Copy)]
struct Kind {
    /// The noun in every message an operator reads.
    noun: &'static str,
    /// The versioned marker standing for "there was no live file here", so a first publication is
    /// exactly as reversible as a replacement.
    absent: &'static [u8],
    /// What this module knows how to check about the bytes, if anything.
    ///
    /// `None` for an artifact: it is opaque at this layer and its caller validated it before the
    /// boundary, which is what `RegistryLock::publish_artifact` says it does. That is a different
    /// statement from "unvalidated", and the field is what makes the difference visible.
    validate: Option<Validator>,
}

const REGISTRY: Kind =
    Kind { noun: "registry", absent: PRIOR_ABSENT, validate: Some(validate_one) };
const ARTIFACT: Kind = Kind { noun: "artifact", absent: ARTIFACT_PRIOR_ABSENT, validate: None };

impl Kind {
    fn check(self, bytes: &[u8], what: &str) -> Result<(), String> {
        match self.validate {
            Some(validate) => validate(bytes, what),
            None => Ok(()),
        }
    }

    /// The rollback slot's content: the absence marker, or something this kind considers valid.
    fn check_rollback(self, bytes: &[u8]) -> Result<(), String> {
        if bytes == self.absent {
            return Ok(());
        }
        self.check(bytes, &format!("rollback {}", self.noun))
    }
}

/// Replace `live` with `candidate`, durably, keeping the prior beside it.
///
/// Every fsync boundary is announced through `trace`, so a test can crash the process at each one
/// and assert that what is on disk is recoverable. See [`Kind`] for why this takes one.
fn publish_durably(
    kind: Kind,
    live: &Path,
    candidate: &[u8],
    trace: &mut impl FnMut(Event),
) -> Result<PublishOutcome, String> {
    kind.check(candidate, "candidate")?;
    let parent = parent(live);
    if !parent.is_dir() {
        return Err(format!(
            "{} directory {} does not exist; installation must create it before publication",
            kind.noun,
            parent.display()
        ));
    }

    let prior = match fs::read(live) {
        Ok(prior) => Some(prior),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read live {} {}: {e}", kind.noun, live.display())),
    };

    let rollback = rollback_path(live)?;
    if prior.as_deref() == Some(candidate) {
        let rollback_bytes = fs::read(&rollback).map_err(|e| {
            format!(
                "live {} already matches but its durable rollback {} is unavailable: {e}",
                kind.noun,
                rollback.display()
            )
        })?;
        kind.check_rollback(&rollback_bytes)?;
        resync_live(kind, live)?;
        return Ok(PublishOutcome::AlreadyPublished);
    }

    if let Some(prior) = prior.as_deref() {
        kind.check(prior, &format!("prior live {}", kind.noun))?;
    }

    let mut candidate_temp = write_temp(kind.noun, live, candidate)?;
    trace(Event::CandidateFileSynced);

    // Absence is a prior state too. The one durable artifact is either the exact prior file or a
    // versioned marker instructing rollback to restore no live file. A first deployment is
    // therefore just as reversible as a replacement.
    let mut rollback_temp =
        write_temp(kind.noun, &rollback, prior.as_deref().unwrap_or(kind.absent))?;
    trace(Event::RollbackFileSynced);
    sync_directory(kind.noun, parent)?;
    trace(Event::DirectorySyncedBeforeRollbackRename);
    fs::rename(&rollback_temp.path, &rollback).map_err(|e| {
        format!(
            "cannot publish rollback {} {} over {}: {e}",
            kind.noun,
            rollback_temp.path.display(),
            rollback.display()
        )
    })?;
    rollback_temp.renamed();
    trace(Event::RollbackRenamed);
    sync_directory(kind.noun, parent)?;
    trace(Event::DirectorySyncedAfterRollbackRename);
    fs::rename(&candidate_temp.path, live).map_err(|e| {
        format!(
            "cannot publish candidate {} {} over {}: {e}",
            kind.noun,
            candidate_temp.path.display(),
            live.display()
        )
    })?;
    candidate_temp.renamed();
    trace(Event::PublishedRenamed);
    sync_directory(kind.noun, parent)?;
    trace(Event::DirectorySyncedAfterPublishRename);
    Ok(PublishOutcome::Published { preserved_prior: prior.is_some() })
}

/// Restore what [`publish_durably`] kept, without consuming the `.previous` evidence — so a
/// rollback stays repeatable after a restart.
fn rollback_durably(kind: Kind, live: &Path) -> Result<(), String> {
    let rollback = rollback_path(live)?;
    let prior = fs::read(&rollback)
        .map_err(|e| format!("cannot read rollback {} {}: {e}", kind.noun, rollback.display()))?;
    let parent = parent(live);
    if prior == kind.absent {
        if let Ok(file) = OpenOptions::new().read(true).open(live) {
            file.sync_all()
                .map_err(|e| format!("cannot sync live {} {}: {e}", kind.noun, live.display()))?;
        }
        sync_directory(kind.noun, parent)?;
        match fs::remove_file(live) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!("cannot remove live {} {}: {e}", kind.noun, live.display()));
            }
        }
        sync_directory(kind.noun, parent)?;
        return Ok(());
    }
    kind.check_rollback(&prior)?;

    let mut temp = write_temp(kind.noun, live, &prior)?;
    sync_directory(kind.noun, parent)?;
    fs::rename(&temp.path, live).map_err(|e| {
        format!(
            "cannot restore rollback {} {} over {}: {e}",
            kind.noun,
            temp.path.display(),
            live.display()
        )
    })?;
    temp.renamed();
    sync_directory(kind.noun, parent)
}

fn lock_path(live: &Path) -> Result<PathBuf, String> {
    sibling(live, ".lock")
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

/// The directory `path` is in, which for a bare file name is `.`.
///
/// `Path::parent` answers `Some("")` there rather than `None`, and `""` names no directory: an
/// `unwrap_or` on the `None` never fires, and the metadata of `""` fails with an error about a
/// file that was never asked for. `pub(crate)` so every caller that needs the containing directory
/// of an operator-supplied path takes this one.
pub(crate) fn parent(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."))
}

/// Write `bytes` beside `target` as a fsynced, owner-only temporary, removed on drop unless
/// [`TempFile::renamed`] is called.
///
/// The building block every durable replacement here is made of, and `pub(crate)` because
/// `crate::tenant`'s export publishes a *pair* of files rather than one with a rollback slot —
/// a different protocol built from the same piece. It had its own `create_new`-based version with
/// no drop guard, so a failure between its two writes left a `.pending` file behind that then
/// made every later run fail with "cannot create".
pub(crate) fn write_temp(noun: &str, target: &Path, bytes: &[u8]) -> Result<TempFile, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = parent(target);
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or(noun);
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
                return Err(format!("cannot create temporary {noun} {}: {e}", path.display()));
            }
        };
        let temp = TempFile::new(path.clone());
        file.write_all(bytes)
            .map_err(|e| format!("cannot write temporary {noun} {}: {e}", path.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot sync temporary {noun} {}: {e}", path.display()))?;
        return Ok(temp);
    }
    Err(format!("cannot allocate a temporary {noun} beside {}", target.display()))
}

/// Re-establish the durability and mode guarantees of a live file that already holds the wanted
/// bytes, so an interrupted rerun ends where a first run would have.
fn resync_live(kind: Kind, live: &Path) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(live)
        .map_err(|e| format!("cannot reopen live {} {}: {e}", kind.noun, live.display()))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot restrict live {} {}: {e}", kind.noun, live.display()))?;
    file.sync_all()
        .map_err(|e| format!("cannot sync live {} {}: {e}", kind.noun, live.display()))?;
    sync_directory(kind.noun, parent(live))
}

/// fsync a directory, so a rename in it survives a power cut.
pub fn sync_directory(noun: &str, path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("cannot sync {noun} directory {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironworks_domain::{ChannelRef, Slug};

    fn dir() -> ironworks_testkit::Scratch {
        ironworks_testkit::Scratch::new("registry-publish")
    }

    fn registry(slug: &str, channel: &str) -> Vec<u8> {
        format!(
            r#"{{"tenants":[{{"slug":"{slug}","channel":"{channel}","org":"{slug}","service":"relationship-intelligence","guidance":"guidance.md","records_credential":"env:RECORDS","runtime_credential":"env:RUNTIME"}}]}}"#
        )
        .into_bytes()
    }

    fn legacy_registry(slug: &str, channel: &str) -> Vec<u8> {
        format!(
            r#"{{"tenants":[{{"slug":"{slug}","channel":"{channel}","service":"relationship-intelligence","guidance":"guidance.md","records_credential":"env:RECORDS","runtime_credential":"env:RUNTIME"}}]}}"#
        )
        .into_bytes()
    }

    fn publish(live: &Path, candidate: &[u8]) -> Result<PublishOutcome, String> {
        RegistryLock::acquire(live)?.publish_one(candidate)
    }

    fn publish_artifact(
        registry_path: &Path,
        artifact: &Path,
        candidate: &[u8],
    ) -> Result<PublishOutcome, String> {
        RegistryLock::acquire(registry_path)?.publish_artifact(artifact, candidate)
    }

    fn migrate(live: &Path, legacy: &[u8], current: &[u8]) -> Result<MigrationOutcome, String> {
        RegistryLock::acquire(live)?.migrate_legacy_one(legacy, current)
    }

    fn rollback(live: &Path) -> Result<(), String> {
        RegistryLock::acquire(live)?.rollback_one()
    }

    /// The directory of a bare file name is the current one — the case `Path::parent` gets wrong.
    #[test]
    fn a_bare_file_name_is_in_the_current_directory() {
        assert_eq!(Path::new("attestation.json").parent(), Some(Path::new("")), "the std answer");
        assert_eq!(parent(Path::new("attestation.json")), Path::new("."));
        assert_eq!(
            parent(Path::new("/etc/ironworks/attestation.json")),
            Path::new("/etc/ironworks")
        );
        assert_eq!(parent(Path::new("run/attestation.json")), Path::new("run"));
    }

    #[test]
    fn publication_syncs_the_rollback_before_replacing_live() {
        let dir = dir();
        let live = dir.join("registry.json");
        fs::write(&live, registry("prior", "-1001")).unwrap();
        let mut events = Vec::new();

        publish_durably(REGISTRY, &live, &registry("candidate", "-1002"), &mut |e| {
            events.push(e);
        })
        .unwrap();

        assert_eq!(
            events,
            vec![
                Event::CandidateFileSynced,
                Event::RollbackFileSynced,
                Event::DirectorySyncedBeforeRollbackRename,
                Event::RollbackRenamed,
                Event::DirectorySyncedAfterRollbackRename,
                Event::PublishedRenamed,
                Event::DirectorySyncedAfterPublishRename,
            ]
        );
    }

    #[test]
    fn publish_then_rollback_restores_the_prior_registry_to_the_serving_loader() {
        let dir = dir();
        let live = dir.join("registry.json");
        let prior = registry("prior", "-1001");
        fs::write(&live, &prior).unwrap();

        assert_eq!(
            publish(&live, &registry("candidate", "-1002")).unwrap(),
            PublishOutcome::Published { preserved_prior: true }
        );
        let published = TenantRegistry::parse(&fs::read_to_string(&live).unwrap()).unwrap();
        assert!(published.tenant(&Slug::parse("candidate").unwrap()).is_some());

        rollback(&live).unwrap();
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
    fn companion_artifact_publication_is_private_durable_and_repeatably_reversible() {
        let dir = dir();
        let registry_path = dir.join("registry.json");
        let guidance = dir.join("guidance.md");
        fs::write(&guidance, b"prior guidance").unwrap();

        assert_eq!(
            publish_artifact(&registry_path, &guidance, b"candidate guidance").unwrap(),
            PublishOutcome::Published { preserved_prior: true }
        );
        assert_eq!(fs::read(&guidance).unwrap(), b"candidate guidance");
        assert_eq!(fs::read(rollback_path(&guidance).unwrap()).unwrap(), b"prior guidance");
        assert_eq!(fs::metadata(&guidance).unwrap().permissions().mode() & 0o777, 0o600);

        let mut lock = RegistryLock::acquire(&registry_path).unwrap();
        lock.rollback_artifact(&guidance).unwrap();
        lock.rollback_artifact(&guidance).unwrap();
        assert_eq!(fs::read(&guidance).unwrap(), b"prior guidance");
        assert!(rollback_path(&guidance).unwrap().exists());
    }

    #[test]
    fn rerunning_an_already_published_candidate_preserves_the_real_rollback() {
        let dir = dir();
        let live = dir.join("registry.json");
        let prior = registry("prior", "-1001");
        let candidate = registry("candidate", "-1002");
        fs::write(&live, &prior).unwrap();
        publish(&live, &candidate).unwrap();
        let rollback_before = fs::read(rollback_path(&live).unwrap()).unwrap();

        assert_eq!(publish(&live, &candidate).unwrap(), PublishOutcome::AlreadyPublished);
        assert_eq!(fs::read(rollback_path(&live).unwrap()).unwrap(), rollback_before);
    }

    #[test]
    fn a_matching_live_file_without_a_durable_rollback_is_not_accepted_as_published() {
        let dir = dir();
        let live = dir.join("registry.json");
        let candidate = registry("candidate", "-1002");
        fs::write(&live, &candidate).unwrap();

        assert!(publish(&live, &candidate).is_err());
        assert_eq!(fs::read(&live).unwrap(), candidate);
    }

    #[test]
    fn an_invalid_prior_is_never_promoted_to_the_rollback_slot() {
        let dir = dir();
        let live = dir.join("registry.json");
        fs::write(&live, b"not a registry").unwrap();
        assert!(publish(&live, &registry("candidate", "-1002")).is_err());
        assert_eq!(fs::read(&live).unwrap(), b"not a registry");
        assert!(!rollback_path(&live).unwrap().exists());
    }

    #[test]
    fn first_publication_rolls_back_to_no_live_registry() {
        let dir = dir();
        let live = dir.join("registry.json");
        publish(&live, &registry("candidate", "-1002")).unwrap();
        assert!(live.exists());
        assert_eq!(fs::read(rollback_path(&live).unwrap()).unwrap(), PRIOR_ABSENT);

        rollback(&live).unwrap();
        assert!(!live.exists(), "rollback did not restore the prior absence");
        assert!(rollback_path(&live).unwrap().exists(), "rollback marker was consumed");
    }

    #[test]
    fn legacy_migration_accepts_only_the_exact_frozen_bytes() {
        let dir = dir();
        let live = dir.join("registry.json");
        let legacy = legacy_registry("candidate", "-1002");
        let current = registry("candidate", "-1002");
        fs::write(&live, &legacy).unwrap();

        assert_eq!(
            migrate(&live, &legacy, &current).unwrap(),
            MigrationOutcome::Migrated { rebuilt_rollback_to_absence: true }
        );
        assert_eq!(fs::read(&live).unwrap(), current);
        assert_eq!(fs::read(rollback_path(&live).unwrap()).unwrap(), PRIOR_ABSENT);

        fs::write(&live, [legacy.as_slice(), b"\n"].concat()).unwrap();
        let before = fs::read(&live).unwrap();
        assert!(migrate(&live, &legacy, &current).is_err());
        assert_eq!(fs::read(&live).unwrap(), before);
    }

    #[test]
    fn legacy_migration_preserves_a_strict_valid_rollback() {
        let dir = dir();
        let live = dir.join("registry.json");
        let rollback = rollback_path(&live).unwrap();
        let legacy = legacy_registry("candidate", "-1002");
        let current = registry("candidate", "-1002");
        let valid_prior = registry("prior", "-1001");
        fs::write(&live, &legacy).unwrap();
        fs::write(&rollback, &valid_prior).unwrap();

        assert_eq!(
            migrate(&live, &legacy, &current).unwrap(),
            MigrationOutcome::Migrated { rebuilt_rollback_to_absence: false }
        );
        assert_eq!(fs::read(&rollback).unwrap(), valid_prior);
    }

    #[test]
    fn a_current_file_repairs_an_invalid_legacy_rollback_idempotently() {
        let dir = dir();
        let live = dir.join("registry.json");
        let rollback = rollback_path(&live).unwrap();
        let legacy = legacy_registry("candidate", "-1002");
        let current = registry("candidate", "-1002");
        fs::write(&live, &current).unwrap();
        fs::write(&rollback, &legacy).unwrap();

        assert_eq!(
            migrate(&live, &legacy, &current).unwrap(),
            MigrationOutcome::AlreadyMigrated { rebuilt_rollback_to_absence: true }
        );
        assert_eq!(fs::read(&rollback).unwrap(), PRIOR_ABSENT);
        assert_eq!(
            migrate(&live, &legacy, &current).unwrap(),
            MigrationOutcome::AlreadyMigrated { rebuilt_rollback_to_absence: false }
        );
    }

    #[test]
    fn every_migration_durability_boundary_leaves_a_recoverable_state() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        for stop_after in 0..8 {
            let dir = dir();
            let live = dir.join("registry.json");
            let legacy = legacy_registry("candidate", "-1002");
            let current = registry("candidate", "-1002");
            fs::write(&live, &legacy).unwrap();
            fs::write(rollback_path(&live).unwrap(), &legacy).unwrap();
            let mut seen = 0;
            let stopped = catch_unwind(AssertUnwindSafe(|| {
                let _ = migrate_legacy_one_traced(&live, &legacy, &current, &mut |_| {
                    if seen == stop_after {
                        panic!("simulated process stop");
                    }
                    seen += 1;
                });
            }));
            assert!(stopped.is_err(), "boundary {stop_after} was not reached");

            let bytes = fs::read(&live).unwrap();
            assert!(bytes == legacy || bytes == current, "boundary {stop_after} exposed junk");
            migrate(&live, &legacy, &current).unwrap();
            assert_eq!(fs::read(&live).unwrap(), current);
            assert!(validate_rollback(&fs::read(rollback_path(&live).unwrap()).unwrap()).is_ok());
        }
    }

    /// The same crash sweep, on the path a **guidance** publication takes.
    ///
    /// The protocol used to exist twice and only the registry copy took a `trace`, so this sweep
    /// could not be run against the artifact path at all — the one that, since ADR 0049, changes
    /// what is served. Both now go through `publish_durably`, so one boundary list covers both.
    #[test]
    fn every_artifact_publication_boundary_leaves_a_recoverable_state() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let prior = b"prior guidance bytes\n".as_slice();
        let candidate = b"candidate guidance bytes\n".as_slice();

        // How many boundaries there are is read from a clean run rather than written down, so a
        // boundary added later is swept rather than silently uncovered.
        let boundaries = {
            let dir = dir();
            let target = dir.join("guidance.md");
            fs::write(&target, prior).unwrap();
            let mut events = Vec::new();
            publish_durably(ARTIFACT, &target, candidate, &mut |e| events.push(e)).unwrap();
            assert_eq!(events.first(), Some(&Event::CandidateFileSynced));
            assert_eq!(events.last(), Some(&Event::DirectorySyncedAfterPublishRename));
            events.len()
        };

        for stop_after in 0..boundaries {
            let dir = dir();
            let target = dir.join("guidance.md");
            fs::write(&target, prior).unwrap();
            let mut seen = 0;
            let stopped = catch_unwind(AssertUnwindSafe(|| {
                let _ = publish_durably(ARTIFACT, &target, candidate, &mut |_| {
                    if seen == stop_after {
                        panic!("simulated process stop");
                    }
                    seen += 1;
                });
            }));
            assert!(stopped.is_err(), "boundary {stop_after} was not reached");

            // Never a half-written file, whichever boundary the process died at.
            let bytes = fs::read(&target).unwrap();
            assert!(bytes == prior || bytes == candidate, "boundary {stop_after} exposed junk");

            // And the run is completable, after which rollback restores exactly the prior bytes.
            let mut lock = RegistryLock::acquire(&target).unwrap();
            lock.publish_artifact(&target, candidate).unwrap();
            assert_eq!(fs::read(&target).unwrap(), candidate);
            lock.rollback_artifact(&target).unwrap();
            assert_eq!(fs::read(&target).unwrap(), prior, "boundary {stop_after} lost the prior");
        }
    }

    /// A first publication is as reversible as a replacement: rollback removes the live file.
    #[test]
    fn a_first_artifact_publication_rolls_back_to_no_live_file() {
        let dir = dir();
        let target = dir.join("guidance.md");
        let mut lock = RegistryLock::acquire(&target).unwrap();
        lock.publish_artifact(&target, b"first").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        assert_eq!(lock.rollback_artifact_bytes(&target).unwrap(), None);
        lock.rollback_artifact(&target).unwrap();
        assert!(!target.exists(), "rollback of a first publication must leave no live artifact");
    }

    #[test]
    #[ignore = "invoked as a subprocess by registry_lock_serializes_different_candidates"]
    fn cross_process_lock_helper() {
        let Some(root) = std::env::var_os("IRONWORKS_REGISTRY_LOCK_HELPER") else {
            return;
        };
        let root = PathBuf::from(root);
        let live = root.join("registry.json");
        let role = std::env::var("IRONWORKS_REGISTRY_LOCK_ROLE").unwrap();
        let candidate =
            if role == "first" { registry("first", "-1001") } else { registry("second", "-1002") };
        let mut guard = RegistryLock::acquire(&live).unwrap();
        guard.publish_one(&candidate).unwrap();
        fs::write(root.join(format!("{role}.acquired")), b"ready").unwrap();
        if role == "first" {
            for _ in 0..500 {
                if root.join("release").exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("parent never released first lock holder");
        }
    }

    #[test]
    fn registry_lock_serializes_different_candidates_across_processes() {
        let dir = dir();
        let exe = std::env::current_exe().unwrap();
        let spawn = |role: &str| {
            std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "registry_publish::tests::cross_process_lock_helper",
                    "--ignored",
                ])
                .env("IRONWORKS_REGISTRY_LOCK_HELPER", dir.path())
                .env("IRONWORKS_REGISTRY_LOCK_ROLE", role)
                .spawn()
                .unwrap()
        };
        let mut first = spawn("first");
        for _ in 0..500 {
            if dir.join("first.acquired").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(dir.join("first.acquired").exists(), "first process never acquired lock");

        let mut second = spawn("second");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !dir.join("second.acquired").exists(),
            "second process entered publication while first still held the lock"
        );
        fs::write(dir.join("release"), b"release").unwrap();
        assert!(first.wait().unwrap().success());
        assert!(second.wait().unwrap().success());
        assert_eq!(fs::read(dir.join("registry.json")).unwrap(), registry("second", "-1002"));
        assert_eq!(
            fs::read(dir.join("registry.json.previous")).unwrap(),
            registry("first", "-1001")
        );
    }
}
