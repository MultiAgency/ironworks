//! Encrypted off-box state snapshots and offline restoration verification.

use ironworks_domain::{Guidance, Slug, TenantRegistry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const CONFIG_VERSION: u32 = 1;
const MANIFEST_VERSION: u32 = 2;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupConfig {
    version: u32,
    repository: String,
    password_file: PathBuf,
    lifecycle_root: PathBuf,
    lifecycle_store: PathBuf,
    tenants: Vec<TenantConfig>,
    #[serde(default)]
    fleet_files: Vec<ConfiguredFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantConfig {
    slug: String,
    state_root: PathBuf,
    config_root: PathBuf,
    journal: PathBuf,
    proposals: PathBuf,
    registry: PathBuf,
    guidance: PathBuf,
    #[serde(default)]
    credentials: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredFile {
    source: PathBuf,
    root: PathBuf,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotManifest {
    version: u32,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    /// Absolute destination this file is intended to occupy after an attended restore.
    restore_path: String,
    kind: FileKind,
    sha256: String,
    mode: u32,
    /// Present only where root ownership is part of the custody contract.
    required_owner_uid: Option<u32>,
    schema_version: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum FileKind {
    Journal,
    Proposals,
    Lifecycle,
    Registry,
    Guidance,
    Credential,
    Configuration,
}

pub fn snapshot(config_path: &str, output: &str) -> Result<(), String> {
    let config: BackupConfig = serde_json::from_slice(
        &fs::read(config_path).map_err(|e| format!("cannot read backup configuration: {e}"))?,
    )
    .map_err(|e| format!("backup configuration is not valid JSON: {e}"))?;
    validate_config(&config)?;

    let output = absolute_clean(Path::new(output), "output directory")?;
    fs::create_dir_all(&output).map_err(|e| format!("cannot create output directory: {e}"))?;
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot restrict output directory: {e}"))?;
    let staging = output.join(format!(".snapshot-staging-{}", std::process::id()));
    fs::create_dir(&staging).map_err(|e| format!("cannot create plaintext staging area: {e}"))?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot restrict plaintext staging area: {e}"))?;
    let cleanup = PlaintextCleanup(staging.clone());

    let mut files = Vec::new();
    copy_file(
        Path::new(config_path),
        &staging.join("fleet/backup-config.json"),
        FileKind::Configuration,
        &mut files,
    )?;
    snapshot_database(
        &config.lifecycle_store,
        &staging.join("fleet/lifecycle.db"),
        FileKind::Lifecycle,
        ironworks_lifecycle::SCHEMA_VERSION,
        &mut files,
    )?;
    for tenant in &config.tenants {
        let slug = Slug::parse(&tenant.slug).map_err(|e| format!("invalid tenant slug: {e}"))?;
        let base = PathBuf::from("tenants").join(slug.as_str());
        snapshot_database(
            &tenant.journal,
            &staging.join(&base).join("serve.journal"),
            FileKind::Journal,
            ironworks_journal::SCHEMA_VERSION,
            &mut files,
        )?;
        snapshot_database(
            &tenant.proposals,
            &staging.join(&base).join("proposals.db"),
            FileKind::Proposals,
            ironworks_proposals::SCHEMA_VERSION,
            &mut files,
        )?;
        let registry_rel = base.join("registry.json");
        let guidance_rel = base.join("guidance.md");
        copy_file(&tenant.registry, &staging.join(&registry_rel), FileKind::Registry, &mut files)?;
        copy_file(&tenant.guidance, &staging.join(&guidance_rel), FileKind::Guidance, &mut files)?;
        for (index, credential) in tenant.credentials.iter().enumerate() {
            copy_file(
                credential,
                &staging.join(&base).join(format!("credential-{index}")),
                FileKind::Credential,
                &mut files,
            )?;
        }
    }
    for file in &config.fleet_files {
        let rel = PathBuf::from("fleet/files").join(&file.name);
        copy_file(&file.source, &staging.join(rel), FileKind::Configuration, &mut files)?;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = SnapshotManifest { version: MANIFEST_VERSION, files };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let staging_manifest = staging.join("snapshot-manifest.json");
    write_private_new(&staging_manifest, &manifest_bytes)?;
    verify(&staging_manifest)?;

    let upload = Command::new("restic")
        .arg("--repo")
        .arg(&config.repository)
        .arg("--password-file")
        .arg(&config.password_file)
        .arg("backup")
        .arg(&staging)
        .arg("--tag")
        .arg("ironworks-state-v2")
        .output()
        .map_err(|e| format!("could not invoke restic: {e}"))?;
    if !upload.status.success() {
        let detail = String::from_utf8_lossy(&upload.stderr);
        if repository_unavailable(&detail) {
            return Err(format!(
                "repository unavailable: restic upload failed with {}",
                upload.status
            ));
        }
        return Err(format!("restic upload failed with {}", upload.status));
    }

    drop(cleanup);
    Ok(())
}

pub fn is_repository_unavailable(error: &str) -> bool {
    error.starts_with("repository unavailable:")
}

fn repository_unavailable(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "connection refused",
        "connection timed out",
        "network is unreachable",
        "no route to host",
        "temporary failure in name resolution",
        "could not resolve host",
        "connection reset",
        "i/o timeout",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
}

/// Whether one manifest entry's declared custody and schema agree with what its kind is.
///
/// **Both directions, for both fields.** A missing declaration and an inapplicable one are
/// different defects and each is reachable: a database with no schema version cannot be checked
/// against the store that will read it, while a guidance file carrying one claims a contract it
/// does not have. Likewise root custody — `Some(0)` is the whole of the requirement for a
/// credential, and any owner at all on a file that is not one is a custody claim nothing enforces.
///
/// Its own function because [`verify`] reaches these two checks only after building a real
/// directory of real files at real modes, which is why neither had a test.
fn declaration_matches_kind(entry: &ManifestFile) -> Result<(), String> {
    let is_database =
        matches!(entry.kind, FileKind::Journal | FileKind::Proposals | FileKind::Lifecycle);
    match (is_database, entry.schema_version.is_some()) {
        (true, false) => {
            return Err(format!("snapshot database {} has no schema version", entry.path));
        }
        (false, true) => {
            return Err(format!("non-database {} carries a schema version", entry.path));
        }
        _ => {}
    }

    let needs_root = matches!(entry.kind, FileKind::Credential | FileKind::Configuration);
    match (needs_root, entry.required_owner_uid) {
        (true, owner) if owner != Some(0) => {
            Err(format!("snapshot {} does not require root custody", entry.path))
        }
        (false, Some(_)) => {
            Err(format!("snapshot {} carries an inapplicable custody owner", entry.path))
        }
        _ => Ok(()),
    }
}

pub fn verify(manifest_path: impl AsRef<Path>) -> Result<(), String> {
    let manifest_path = manifest_path.as_ref();
    let root = manifest_path.parent().ok_or("manifest has no parent directory")?;
    let manifest: SnapshotManifest = serde_json::from_slice(
        &fs::read(manifest_path).map_err(|e| format!("cannot read snapshot manifest: {e}"))?,
    )
    .map_err(|e| format!("snapshot manifest is not valid JSON: {e}"))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(format!("unsupported snapshot manifest version {}", manifest.version));
    }
    if manifest.files.is_empty() {
        return Err("snapshot manifest must contain files".to_owned());
    }
    let mut prior = None;
    let mut indexed = std::collections::BTreeMap::new();
    let mut restore_paths = std::collections::BTreeSet::new();
    let mut lifecycle_files = 0usize;
    for entry in &manifest.files {
        let relative = safe_relative(&entry.path)?;
        let restore_path = absolute_clean(Path::new(&entry.restore_path), "manifest restore path")?;
        if !restore_paths.insert(restore_path) {
            return Err("snapshot manifest contains duplicate restore paths".to_owned());
        }
        if prior.as_ref().is_some_and(|p: &String| p >= &entry.path) {
            return Err("snapshot manifest file paths are duplicated or unsorted".to_owned());
        }
        prior = Some(entry.path.clone());
        if indexed.insert(entry.path.as_str(), entry.kind).is_some() {
            return Err("snapshot manifest contains duplicate file paths".to_owned());
        }
        declaration_matches_kind(entry)?;
        if entry.kind == FileKind::Lifecycle {
            lifecycle_files += 1;
        }
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("required snapshot file {} is unavailable: {e}", entry.path))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!("snapshot file {} is not a regular file", entry.path));
        }
        let actual_mode = metadata.permissions().mode() & 0o777;
        if actual_mode != entry.mode || actual_mode & 0o077 != 0 {
            return Err(format!("snapshot file {} has unsafe or changed custody mode", entry.path));
        }
        if entry.required_owner_uid.is_some_and(|uid| metadata.uid() != uid) {
            return Err(format!("snapshot file {} has the wrong custody owner", entry.path));
        }
        if digest_file(&path)? != entry.sha256 {
            return Err(format!("snapshot file {} failed its digest", entry.path));
        }
        if let Some(expected) = entry.schema_version {
            verify_sqlite(&path, entry.kind, expected)?;
        }
    }
    if lifecycle_files != 1 || indexed.get("fleet/lifecycle.db") != Some(&FileKind::Lifecycle) {
        return Err(
            "snapshot must contain exactly fleet/lifecycle.db as its lifecycle store".into()
        );
    }
    let registries: Vec<_> =
        manifest.files.iter().filter(|entry| entry.kind == FileKind::Registry).collect();
    if registries.is_empty() {
        return Err("snapshot manifest contains no tenant registry".to_owned());
    }
    for registry_entry in &registries {
        let slug_text = registry_entry
            .path
            .strip_prefix("tenants/")
            .and_then(|path| path.strip_suffix("/registry.json"))
            .ok_or_else(|| {
                format!("tenant registry has unexpected path {}", registry_entry.path)
            })?;
        let slug = Slug::parse(slug_text).map_err(|e| e.to_string())?;
        let base = format!("tenants/{slug_text}");
        let guidance = format!("{base}/guidance.md");
        for (path, kind) in [
            (format!("{base}/serve.journal"), FileKind::Journal),
            (format!("{base}/proposals.db"), FileKind::Proposals),
            (registry_entry.path.clone(), FileKind::Registry),
            (guidance.clone(), FileKind::Guidance),
        ] {
            if indexed.get(path.as_str()) != Some(&kind) {
                return Err(format!(
                    "tenant {} is missing required {kind:?} file {path}",
                    slug.as_str()
                ));
            }
        }
        let registry_path = root.join(safe_relative(&registry_entry.path)?);
        let guidance_path = root.join(safe_relative(&guidance)?);
        let registry =
            TenantRegistry::parse(&fs::read_to_string(&registry_path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{}: {e}", registry_entry.path))?;
        if registry.len() != 1 {
            return Err(format!("{} is not a strict one-entry registry", registry_entry.path));
        }
        let tenant = registry
            .tenant(&slug)
            .ok_or_else(|| format!("{} does not bind {}", registry_entry.path, slug.as_str()))?;
        let canonical_guidance = manifest
            .files
            .iter()
            .find(|entry| entry.path == guidance)
            .map(|entry| entry.restore_path.as_str())
            .ok_or_else(|| format!("{} guidance is absent from its manifest", slug.as_str()))?;
        if tenant.guidance().as_str() != canonical_guidance {
            return Err(format!(
                "{} guidance destination does not match its manifest",
                slug.as_str()
            ));
        }
        Guidance::parse(
            &fs::read_to_string(guidance_path).map_err(|e| e.to_string())?,
            slug.as_str(),
            tenant.service().as_str(),
        )
        .map_err(|e| format!("{} guidance is invalid: {e}", slug.as_str()))?;
    }
    for kind in [FileKind::Journal, FileKind::Proposals, FileKind::Registry, FileKind::Guidance] {
        if manifest.files.iter().filter(|entry| entry.kind == kind).count() != registries.len() {
            return Err(format!("snapshot has an unexpected number of {kind:?} files"));
        }
    }
    Ok(())
}

fn validate_config(config: &BackupConfig) -> Result<(), String> {
    if config.version != CONFIG_VERSION {
        return Err(format!("unsupported backup configuration version {}", config.version));
    }
    if config.repository.trim().is_empty() {
        return Err("restic repository is required; there is no fallback".to_owned());
    }
    validate_literal_path(&config.password_file, "password_file")?;
    let lifecycle_root = permitted_root(&config.lifecycle_root, "lifecycle_root")?;
    require_within(&config.lifecycle_store, &lifecycle_root, "lifecycle_store")?;
    if config.tenants.is_empty() {
        return Err("backup configuration names no tenants".to_owned());
    }
    let mut slugs = std::collections::BTreeSet::new();
    for tenant in &config.tenants {
        Slug::parse(&tenant.slug).map_err(|e| format!("invalid tenant slug: {e}"))?;
        if !slugs.insert(&tenant.slug) {
            return Err(format!("tenant {} appears twice", tenant.slug));
        }
        let state = permitted_root(&tenant.state_root, "tenant state_root")?;
        let config_root = permitted_root(&tenant.config_root, "tenant config_root")?;
        require_within(&tenant.journal, &state, "journal")?;
        require_within(&tenant.proposals, &state, "proposals")?;
        require_within(&tenant.registry, &state, "registry")?;
        require_within(&tenant.guidance, &config_root, "guidance")?;
        for credential in &tenant.credentials {
            require_within(credential, &config_root, "credential")?;
        }
    }
    let mut fleet_names = std::collections::BTreeSet::new();
    for file in &config.fleet_files {
        if file.name.is_empty() || safe_relative(&file.name)?.components().count() != 1 {
            return Err("fleet file names must be one safe path component".to_owned());
        }
        if file.name == "backup-config.json" || !fleet_names.insert(&file.name) {
            return Err(format!("fleet file name {} is reserved or duplicated", file.name));
        }
        let root = permitted_root(&file.root, "fleet file root")?;
        require_within(&file.source, &root, "fleet file")?;
    }
    Ok(())
}

fn validate_literal_path(path: &Path, field: &str) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.contains(['*', '?', '$']) || text.starts_with('~') {
        return Err(format!("{field} must be a literal path without globs or variables"));
    }
    if !path.is_absolute() {
        return Err(format!("{field} must be absolute"));
    }
    Ok(())
}

fn permitted_root(path: &Path, field: &str) -> Result<PathBuf, String> {
    validate_literal_path(path, field)?;
    let canonical = canonical(path)?;
    let depth = canonical.components().filter(|c| matches!(c, Component::Normal(_))).count();
    // Roots are admission bounds, never recursively traversed: every archived source is still an
    // exact file. Two components admits application namespaces such as `/etc/ironworks` and
    // `/opt/ironclaw` while still rejecting `/`, `/etc`, `/var`, and other system-wide roots.
    if depth < 2 {
        return Err(format!("{field} is too broad to be a permitted backup root"));
    }
    Ok(canonical)
}

fn require_within(path: &Path, root: &Path, field: &str) -> Result<(), String> {
    validate_literal_path(path, field)?;
    let actual = canonical(path)?;
    if actual == root || !actual.starts_with(root) {
        return Err(format!("{field} must name a file below its exact permitted root"));
    }
    Ok(())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| format!("cannot resolve {}: {e}", path.display()))
}

fn absolute_clean(path: &Path, field: &str) -> Result<PathBuf, String> {
    validate_literal_path(path, field)?;
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{field} may not contain parent traversal"));
    }
    Ok(path.to_path_buf())
}

fn safe_relative(text: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(text);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("unsafe manifest path {text:?}"));
    }
    Ok(path)
}

fn snapshot_database(
    source: &Path,
    destination: &Path,
    kind: FileKind,
    schema_version: i64,
    files: &mut Vec<ManifestFile>,
) -> Result<(), String> {
    make_parent(destination)?;
    match kind {
        FileKind::Journal => ironworks_journal::Journal::open(source)
            .map_err(|e| e.to_string())?
            .snapshot_to(destination)
            .map_err(|e| e.to_string())?,
        FileKind::Proposals => ironworks_proposals::ProposalStore::open(source)
            .map_err(|e| e.to_string())?
            .snapshot_to(destination)
            .map_err(|e| e.to_string())?,
        FileKind::Lifecycle => ironworks_lifecycle::LifecycleStore::open(source)
            .map_err(|e| e.to_string())?
            .snapshot_to(destination)
            .map_err(|e| e.to_string())?,
        _ => return Err("internal snapshot kind is not a database".to_owned()),
    }
    add_manifest_file(destination, source, kind, None, Some(schema_version), files)
}

fn copy_file(
    source: &Path,
    destination: &Path,
    kind: FileKind,
    files: &mut Vec<ManifestFile>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|e| format!("cannot inspect {}: {e}", source.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a regular file", source.display()));
    }
    // A credential is secret, so group/other may not even read it. Configuration — the backup
    // configuration and the pin and artifact manifests — is not, and only writing is refused.
    let mode = metadata.permissions().mode();
    let custody = match kind {
        FileKind::Credential if metadata.uid() != 0 || mode & 0o077 != 0 => {
            Some("root-owned and inaccessible to group/other")
        }
        FileKind::Configuration if metadata.uid() != 0 || mode & 0o022 != 0 => {
            Some("root-owned and not writable by group/other")
        }
        _ => None,
    };
    if let Some(custody) = custody {
        return Err(format!("{} must be {custody} before it can be backed up", source.display()));
    }
    make_parent(destination)?;
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(metadata.permissions().mode() & 0o700)
        .open(destination)
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())?;
    let required_owner_uid =
        matches!(kind, FileKind::Credential | FileKind::Configuration).then_some(0);
    add_manifest_file(destination, source, kind, required_owner_uid, None, files)
}

fn add_manifest_file(
    path: &Path,
    restore_path: &Path,
    kind: FileKind,
    required_owner_uid: Option<u32>,
    schema_version: Option<i64>,
    files: &mut Vec<ManifestFile>,
) -> Result<(), String> {
    let staging = path
        .ancestors()
        .find(|p| {
            p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".snapshot-staging-"))
        })
        .ok_or("snapshot path escaped staging")?;
    files.push(ManifestFile {
        path: path_text(path.strip_prefix(staging).map_err(|_| "snapshot path escaped staging")?)?,
        restore_path: path_text(&canonical(restore_path)?)?,
        kind,
        sha256: digest_file(path)?,
        mode: fs::metadata(path).map_err(|e| e.to_string())?.permissions().mode() & 0o777,
        required_owner_uid,
        schema_version,
    });
    Ok(())
}

fn verify_sqlite(path: &Path, kind: FileKind, expected: i64) -> Result<(), String> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{} is not readable SQLite: {e}", path.display()))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err(format!("{} failed SQLite integrity: {integrity}", path.display()));
    }
    let table = match kind {
        FileKind::Journal => "meta",
        FileKind::Proposals => "proposal_meta",
        FileKind::Lifecycle => "lifecycle_meta",
        _ => return Err("manifest applies a schema version to a non-database".to_owned()),
    };
    let found: String = connection
        .query_row(&format!("SELECT value FROM {table} WHERE key='schema_version'"), [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if found != expected.to_string() {
        return Err(format!("{} has schema {found}, expected {expected}", path.display()));
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn make_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("snapshot path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| e.to_string())
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str().map(str::to_owned).ok_or_else(|| "path is not UTF-8".to_owned())
}

struct PlaintextCleanup(PathBuf);
impl Drop for PlaintextCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironworks_testkit::Scratch;

    fn entry(kind: FileKind, owner: Option<u32>, schema: Option<i64>) -> ManifestFile {
        ManifestFile {
            path: "a".to_owned(),
            restore_path: "/a".to_owned(),
            kind,
            sha256: "0".repeat(64),
            mode: 0o600,
            required_owner_uid: owner,
            schema_version: schema,
        }
    }

    /// A manifest entry has to declare what its kind actually requires — in both directions.
    ///
    /// Each of the four refusals is paired with the entry that must be accepted, because a check
    /// that can only invent a difference is the same defect as one that can only invent a match
    /// (§K-8). All eight were unreachable from a test before: `verify` gets here only after
    /// building a real directory of real files at real modes.
    #[test]
    fn a_manifest_entry_declares_the_custody_and_schema_its_kind_requires() {
        // Databases: a schema version is the whole point, and its absence is not a default.
        assert!(declaration_matches_kind(&entry(FileKind::Journal, None, Some(8))).is_ok());
        assert!(
            declaration_matches_kind(&entry(FileKind::Lifecycle, None, None))
                .is_err_and(|e| e.contains("has no schema version"))
        );

        // And a file that is not a database must not claim one — that is a contract nothing holds.
        assert!(declaration_matches_kind(&entry(FileKind::Guidance, None, None)).is_ok());
        assert!(
            declaration_matches_kind(&entry(FileKind::Guidance, None, Some(8)))
                .is_err_and(|e| e.contains("carries a schema version"))
        );

        // Credentials and configuration are root-owned, and `Some(0)` is the whole requirement:
        // any other owner, and no owner at all, are both a custody claim that is not made.
        assert!(declaration_matches_kind(&entry(FileKind::Credential, Some(0), None)).is_ok());
        assert!(declaration_matches_kind(&entry(FileKind::Configuration, Some(0), None)).is_ok());
        assert!(
            declaration_matches_kind(&entry(FileKind::Credential, None, None))
                .is_err_and(|e| e.contains("does not require root custody"))
        );
        assert!(
            declaration_matches_kind(&entry(FileKind::Credential, Some(1000), None))
                .is_err_and(|e| e.contains("does not require root custody"))
        );

        // Anything else carrying an owner is declaring custody nothing will enforce on restore.
        assert!(
            declaration_matches_kind(&entry(FileKind::Registry, Some(0), None))
                .is_err_and(|e| e.contains("inapplicable custody owner"))
        );
        assert!(
            declaration_matches_kind(&entry(FileKind::Journal, Some(0), Some(8)))
                .is_err_and(|e| e.contains("inapplicable custody owner"))
        );
    }

    /// A staging root inside `scratch`, named the way `snapshot` names one — `add_manifest_file`
    /// finds the root of a manifest path by that prefix.
    fn staging(scratch: &Scratch) -> PathBuf {
        let root = scratch.join(".snapshot-staging-test");
        fs::create_dir(&root).unwrap();
        root
    }

    /// Custody is refused per kind, and the refusal names the rule that kind actually has.
    ///
    /// Group/other-writable configuration and group-readable credentials are refused whoever owns
    /// them, so this holds under any uid. A credential may not be read by group/other; configuration
    /// only may not be written — the message used to claim "inaccessible" for both.
    #[test]
    fn a_custody_refusal_names_the_rule_its_kind_has() {
        let scratch = Scratch::new("state-custody");
        let root = staging(&scratch);
        for (kind, mode, rule) in [
            (FileKind::Configuration, 0o664, "not writable by group/other"),
            (FileKind::Credential, 0o640, "inaccessible to group/other"),
        ] {
            let source = scratch.join("source");
            fs::write(&source, b"x").unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(mode)).unwrap();
            let refused =
                copy_file(&source, &root.join("copy"), kind, &mut Vec::new()).unwrap_err();
            assert!(refused.contains(rule), "{kind:?}: {refused}");
            assert!(!root.join("copy").exists(), "{kind:?} was copied before it was refused");
        }
    }

    #[test]
    fn paths_refuse_broad_roots_globs_variables_and_traversal() {
        assert!(permitted_root(Path::new("/"), "root").is_err());
        assert!(validate_literal_path(Path::new("/var/lib/*"), "path").is_err());
        assert!(validate_literal_path(Path::new("$STATE/file"), "path").is_err());
        assert!(safe_relative("../secret").is_err());
        assert!(safe_relative("/absolute").is_err());
        assert_eq!(
            safe_relative("tenants/acme/serve.journal").unwrap(),
            PathBuf::from("tenants/acme/serve.journal")
        );
    }

    #[test]
    fn only_transport_failures_classify_the_repository_as_unavailable() {
        assert!(repository_unavailable("dial tcp: network is unreachable"));
        assert!(repository_unavailable("Could not resolve host backup.invalid"));
        assert!(!repository_unavailable("repository contains errors: pack is corrupt"));
        assert!(!repository_unavailable("wrong password or no key found"));
    }

    #[test]
    fn restored_content_verifies_with_sqlite_and_registry_guidance_binding() {
        let scratch = Scratch::new("state-verify");
        let root = staging(&scratch);
        let tenant = root.join("tenants/acme");
        fs::create_dir_all(&tenant).unwrap();
        let journal = tenant.join("serve.journal");
        let proposals = tenant.join("proposals.db");
        let lifecycle = root.join("fleet/lifecycle.db");
        fs::create_dir_all(lifecycle.parent().unwrap()).unwrap();
        ironworks_journal::Journal::open(&journal).unwrap();
        ironworks_proposals::ProposalStore::open(&proposals).unwrap();
        ironworks_lifecycle::LifecycleStore::open(&lifecycle).unwrap();
        let guidance_source = root.join("canonical-guidance.md");
        let guidance = format!(
            "<!-- tenant-guidance v1 slug: acme service: relationship-intelligence -->\n\n{}",
            "specific tenant operating guidance. ".repeat(20)
        );
        fs::write(&guidance_source, &guidance).unwrap();
        fs::set_permissions(&guidance_source, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(tenant.join("guidance.md"), &guidance).unwrap();
        fs::set_permissions(tenant.join("guidance.md"), fs::Permissions::from_mode(0o600)).unwrap();
        let registry = format!(
            "{{\"tenants\":[{{\"slug\":\"acme\",\"channel\":\"room\",\"org\":\"acme\",\"service\":\"relationship-intelligence\",\"guidance\":{:?},\"records_credential\":\"env:RECORDS\",\"runtime_credential\":\"env:RUNTIME\"}}]}}",
            guidance_source.display().to_string()
        );
        fs::write(tenant.join("registry.json"), registry).unwrap();
        fs::set_permissions(tenant.join("registry.json"), fs::Permissions::from_mode(0o600))
            .unwrap();

        let mut files = Vec::new();
        for (path, kind, schema) in [
            (&journal, FileKind::Journal, Some(ironworks_journal::SCHEMA_VERSION)),
            (&proposals, FileKind::Proposals, Some(ironworks_proposals::SCHEMA_VERSION)),
            (&lifecycle, FileKind::Lifecycle, Some(ironworks_lifecycle::SCHEMA_VERSION)),
            (&tenant.join("registry.json"), FileKind::Registry, None),
            (&tenant.join("guidance.md"), FileKind::Guidance, None),
        ] {
            add_manifest_file(path, path, kind, None, schema, &mut files).unwrap();
        }
        files.iter_mut().find(|entry| entry.kind == FileKind::Guidance).unwrap().restore_path =
            guidance_source.display().to_string();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = SnapshotManifest { version: MANIFEST_VERSION, files };
        let manifest_path = root.join("snapshot-manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(verify(&manifest_path).is_ok());

        let mut duplicate_restore = manifest.clone();
        duplicate_restore.files[1].restore_path = duplicate_restore.files[0].restore_path.clone();
        fs::write(&manifest_path, serde_json::to_vec(&duplicate_restore).unwrap()).unwrap();
        assert!(verify(&manifest_path).unwrap_err().contains("duplicate restore paths"));
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let mut omitted = manifest.clone();
        omitted.files.retain(|entry| entry.kind != FileKind::Proposals);
        fs::write(&manifest_path, serde_json::to_vec(&omitted).unwrap()).unwrap();
        assert!(verify(&manifest_path).unwrap_err().contains("missing required Proposals"));
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        fs::write(tenant.join("guidance.md"), "tampered").unwrap();
        assert!(verify(&manifest_path).unwrap_err().contains("failed its digest"));
    }
}
