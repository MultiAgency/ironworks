//! Naming a book without carrying a word of it.
//!
//! A transcript records which records a turn was shown, and it may not hold the records — the
//! real book is a tenant's. So the binding is a **fingerprint**: per-file digests, which identify
//! a record without reproducing it.
//!
//! This must stay byte-identical to the value every recorded transcript already carries, because
//! that is the only thing that lets a later reader prove an answer was graded over the book on
//! disk today. The demo and the operator page refuse to render a transcript whose
//! fingerprint does not match, and a fingerprint filed after the fact is worth nothing.

use sha2::{Digest, Sha256};
use std::path::Path;

/// A book's identity: how many records, and a digest naming exactly which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub records: usize,
    pub fingerprint: String,
}

/// Fingerprint every `*.json` in `book_dir`, or `None` when the directory does not exist.
///
/// `None` is the honest answer for a suite whose book is not in this repository, and it is
/// distinct from a book of zero records.
///
/// **Sorted by file name**, so the value does not depend on directory order — the one detail that
/// would make this irreproducible on another machine.
pub fn book(book_dir: impl AsRef<Path>) -> Option<Fingerprint> {
    let dir = book_dir.as_ref();
    if !dir.is_dir() {
        return None;
    }
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort_by_key(|p| p.file_name().map(std::ffi::OsString::from));

    let mut manifest = String::new();
    for path in &files {
        let bytes = std::fs::read(path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        manifest.push_str(&format!("{name} {}\n", hex(&Sha256::digest(&bytes))));
    }
    Some(Fingerprint {
        records: files.len(),
        fingerprint: ironworks_domain::composition::digest16(manifest.as_bytes()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value every current transcript is bound to.
    ///
    /// This is the regression guard for the whole port: if the Rust fingerprint ever stops
    /// matching, every transcript in `docs/experiments` silently becomes unbindable and the two
    /// readers that check it would refuse to render anything. Recorded in
    /// `0020-account-coordination-ten-account-book.json` and `0021`.
    #[test]
    fn the_current_book_fingerprints_as_the_transcripts_record() {
        let root = crate::root();
        let got = book(root.join("fixtures/account-coordination/book"))
            .expect("the coordination book is in this repository");
        assert_eq!(got.records, 10);
        assert_eq!(
            got.fingerprint, "0f1291ddd6c78fcb",
            "the fingerprint moved; every recorded transcript binds to the old value"
        );
    }

    #[test]
    fn a_book_that_is_not_here_fingerprints_as_none() {
        // Distinct from a book of zero records, which is a book we looked at and found empty.
        assert_eq!(book("/nonexistent/book"), None);
    }

    #[test]
    fn the_manifest_is_ordered_by_name_not_by_directory_order() {
        // Two files written in one order and read in another must agree, or the value is a
        // property of the filesystem rather than of the book.
        let dir = std::env::temp_dir().join(format!("iw-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.json"), b"{\"b\":1}").unwrap();
        std::fs::write(dir.join("a.json"), b"{\"a\":1}").unwrap();
        let first = book(&dir).unwrap();

        let other = dir.with_extension("two");
        let _ = std::fs::remove_dir_all(&other);
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("a.json"), b"{\"a\":1}").unwrap();
        std::fs::write(other.join("b.json"), b"{\"b\":1}").unwrap();
        let second = book(&other).unwrap();

        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn non_json_files_are_not_records() {
        let dir = std::env::temp_dir().join(format!("iw-fp-mixed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{}").unwrap();
        let only_json = book(&dir).unwrap();
        std::fs::write(dir.join("README.md"), b"not a record").unwrap();
        assert_eq!(book(&dir).unwrap(), only_json, "a README is not a record");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
