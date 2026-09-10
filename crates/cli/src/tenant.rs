//! Human-operated tenant export.

use ironworks_domain::{
    CredentialPurpose, CredentialRef, OrgId, ScopeResolver, Slug, TenantRegistry,
};
use ironworks_records::{Book, BookExport, EnvCredentials, RecordAdministration};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ExportResult {
    pub bytes: usize,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportManifest {
    version: u32,
    slug: String,
    org: String,
    bytes: usize,
    sha256: String,
}

pub fn export(slug: &str, registry_path: &str, output: &str) -> Result<ExportResult, String> {
    let slug = Slug::parse(slug).map_err(|e| e.to_string())?;
    let output = Path::new(output);
    if !output.is_absolute() {
        return Err("--output must be an absolute path".to_owned());
    }
    let registry = TenantRegistry::parse(
        &fs::read_to_string(registry_path).map_err(|e| format!("cannot read registry: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    if registry.len() != 1 {
        return Err("tenant export requires a strict one-entry registry".to_owned());
    }
    let entry = registry.tenant(&slug).ok_or("registry does not contain the requested tenant")?;
    let base = crate::Var::RecordStore.required()?;
    let book = Book::new(&base, EnvCredentials);
    let scope = ScopeResolver::resolve(&book, entry.records())
        .map_err(|e| format!("the read credential could not establish scope: {e}"))?;
    if !entry.org().matches(scope.org()) {
        return Err("the verified record scope differs from the registry expectation".to_owned());
    }
    if let Some(existing) = verify_existing(&slug, scope.org(), output)? {
        return Ok(existing);
    }
    let export_credential =
        CredentialRef::new(CredentialPurpose::RecordExport, "env:IRONWORKS_RECORDS_EXPORT_TOKEN");
    let exported = RecordAdministration::new(base, EnvCredentials)
        .export(&export_credential, scope.org())
        .map_err(|e| e.to_string())?;
    let manifest = ExportManifest {
        version: 1,
        slug: slug.as_str().to_owned(),
        org: exported.org().as_str().to_owned(),
        bytes: exported.bytes().len(),
        sha256: exported.digest().to_owned(),
    };
    publish_pair(output, exported.bytes(), &manifest)?;
    Ok(ExportResult { bytes: manifest.bytes, digest: manifest.sha256 })
}

fn verify_existing(
    slug: &Slug,
    expected_org: &OrgId,
    output: &Path,
) -> Result<Option<ExportResult>, String> {
    let manifest_path = manifest_path(output)?;
    match (output.exists(), manifest_path.exists()) {
        (false, false) => Ok(None),
        (true, true) => {
            let manifest: ExportManifest =
                serde_json::from_slice(&fs::read(&manifest_path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if manifest.version != 1
                || manifest.slug != slug.as_str()
                || manifest.org != expected_org.as_str()
            {
                return Err("existing export manifest does not match this command".to_owned());
            }
            let bytes = fs::read(output).map_err(|e| e.to_string())?;
            let exported = BookExport::validate(bytes, expected_org).map_err(|e| e.to_string())?;
            if exported.bytes().len() != manifest.bytes || exported.digest() != manifest.sha256 {
                return Err("existing export does not match its manifest".to_owned());
            }
            Ok(Some(ExportResult { bytes: manifest.bytes, digest: manifest.sha256 }))
        }
        _ => {
            Err("export and manifest are incomplete; preserve both and reconcile manually"
                .to_owned())
        }
    }
}

/// The export and its manifest, published together and durably.
///
/// # It is a pair, and it used to be a weaker protocol than its sibling
///
/// `verify_existing` refuses a half — an export without its manifest, or the reverse — so what is
/// on disk after a crash has to be both or neither. This wrote both temporaries, renamed both, and
/// fsynced the directory once at the end: no sync *before* the renames, so a power cut could leave
/// a rename durable over a file whose bytes were not; and no sync between them, so the pair could
/// land apart. `crates/cli/src/registry_publish.rs::publish_durably` in the same crate has always
/// done all three.
///
/// It also staged through its own `create_new` writer with no drop guard, so a failure in the
/// second write left `.book.json.pending` behind — and `create_new` then made **every later run**
/// fail with "cannot create". The staging primitive it now shares removes the file on drop and
/// names it uniquely, so neither can happen.
fn publish_pair(output: &Path, bytes: &[u8], manifest: &ExportManifest) -> Result<(), String> {
    use crate::registry_publish::{sync_directory, write_temp};

    let parent = output.parent().ok_or("output has no parent directory")?;
    let manifest_path = manifest_path(output)?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;

    let mut export_temp = write_temp("export", output, bytes)?;
    let mut manifest_temp = write_temp("export manifest", &manifest_path, &manifest_bytes)?;
    sync_directory("export", parent)?;

    // The manifest first: `verify_existing` reads it to decide whether an export is complete, so
    // a crash between the two renames leaves a manifest with no export — which that function
    // already refuses — rather than an export nothing vouches for.
    fs::rename(manifest_temp.path(), &manifest_path).map_err(|e| e.to_string())?;
    manifest_temp.renamed();
    sync_directory("export", parent)?;
    fs::rename(export_temp.path(), output).map_err(|e| e.to_string())?;
    export_temp.renamed();
    sync_directory("export", parent)
}

fn manifest_path(output: &Path) -> Result<PathBuf, String> {
    let name = output.file_name().ok_or("output has no file name")?.to_string_lossy();
    Ok(output.with_file_name(format!("{name}.manifest.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    /// A temporary directory that goes away when it goes out of scope.
    ///
    /// This was the seventh copy and the worst: it returned a bare `PathBuf` with no `Drop`, and
    /// the test below ended with `let _ = fs::remove_dir_all(root);` — a line that never runs on a
    /// panic, so every failing run leaked its directory.
    fn scratch() -> ironworks_testkit::Scratch {
        ironworks_testkit::Scratch::new("tenant-export")
    }

    fn export_bytes(org: &str) -> (Vec<u8>, String) {
        let mut value = serde_json::json!({
            "format": "ironworks-book-v1",
            "org": org,
            "complete": true,
            "exported_at": "2026-09-06T12:00:00+00:00",
            "accounts": [],
            "contacts": [],
            "activities": []
        });
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&value).unwrap()));
        value
            .as_object_mut()
            .unwrap()
            .insert("full_content_digest".to_owned(), Value::String(digest.clone()));
        (serde_json::to_vec(&value).unwrap(), digest)
    }

    #[test]
    fn a_valid_existing_export_is_idempotent_but_never_crosses_org_scope() {
        let root = scratch();
        let output = root.join("book.json");
        let slug = Slug::parse("acme").unwrap();
        let acme = OrgId::parse("acme").unwrap();
        let globex = OrgId::parse("globex").unwrap();
        let (bytes, digest) = export_bytes("acme");
        let manifest = ExportManifest {
            version: 1,
            slug: "acme".to_owned(),
            org: "acme".to_owned(),
            bytes: bytes.len(),
            sha256: digest.clone(),
        };
        publish_pair(&output, &bytes, &manifest).unwrap();

        let existing = verify_existing(&slug, &acme, &output).unwrap().unwrap();
        assert_eq!(existing.digest, digest);
        assert!(verify_existing(&slug, &globex, &output).is_err());
    }
}
