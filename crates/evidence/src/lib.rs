//! What a recorded run is bound to, and what was said about it afterwards.
//!
//! Nothing here prints record text, guidance text or answers. **Digests, ids and counts only** —
//! the same restraint the turn journal keeps, for the same reason: a book fingerprint identifies
//! the records a turn was shown without carrying a word of them, and a tenant's book is a tenant's.

pub mod fingerprint;
pub mod regrades;

/// The repository root, from this crate's manifest location.
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}
