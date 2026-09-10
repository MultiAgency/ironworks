//! A fingerprint of a request body. Never the body.
//!
//! # There is one SHA-256 in this workspace, and it is `sha2`
//!
//! This module carried its own, ~40 lines of it, "so this crate needs no hashing dependency
//! beside SQLite". `sha2` is already a workspace dependency, already in the lockfile, and already
//! used in four `ironworks-domain` modules — and this crate depends on `ironworks-domain`. So the
//! manifest line that avoided added **no transitive dependency at all**, which is the thing
//! `Cargo.toml`'s dependency budget is written about. What it did buy was a second implementation
//! of a primitive whose whole value is that everyone computes the same answer.
//!
//! The known-answer tests below are kept exactly as they were.

use std::fmt;

/// SHA-256 of the exact bytes that were, or would be, sent.
///
/// Stored instead of the body because the journal holds identifiers only — and it is enough:
/// crash recovery does not need to *read* the old request, only to prove a rebuilt one is
/// byte-identical to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestDigest(String);

impl RequestDigest {
    /// Fingerprint a request body.
    pub fn of(body: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(body);
        Self(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
    /// Rehydrate from storage. Not a computation — the value came from `of` originally.
    pub fn from_hex(v: impl Into<String>) -> Self {
        Self(v.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known-answer tests, because a hand-written hash that is subtly wrong would make every
    /// replay decision wrong in the same direction — and silently.
    #[test]
    fn matches_the_published_vectors() {
        assert_eq!(
            RequestDigest::of(b"").as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            RequestDigest::of(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            RequestDigest::of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").as_str(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_single_byte_changes_the_digest() {
        let a = RequestDigest::of(br#"{"model":"m","input":"x"}"#);
        let b = RequestDigest::of(br#"{"model":"m","input":"y"}"#);
        assert_ne!(a, b, "a replay must not accept a different body");
    }

    #[test]
    fn it_spans_a_multi_block_body() {
        // 14KB is the real size of an alloy; a hash that only handled one block would pass every
        // small test and fail on every real request.
        let big = "x".repeat(14_337);
        assert_eq!(RequestDigest::of(big.as_bytes()).as_str().len(), 64);
        assert_ne!(
            RequestDigest::of(big.as_bytes()),
            RequestDigest::of(format!("{big}y").as_bytes())
        );
    }
}
