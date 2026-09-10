//! What a recorded run is bound to, and what was said about it afterwards.
//!
//! Nothing here prints record text, guidance text or answers. **Digests, ids and counts only** —
//! the same restraint the turn journal keeps, for the same reason: a book fingerprint identifies
//! the records a turn was shown without carrying a word of them, and a tenant's book is a tenant's.
//!
//! Two of these modules read a run that has happened, and two carry state between runs:
//!
//! - [`fingerprint`] identifies the book a turn was shown, and [`regrades`] holds what a later
//!   reading said about a verdict already recorded.
//! - [`state`] is the evidence state (`E_t`): observations about a candidate, appended and never
//!   rewritten. Its ids are types that cannot hold prose, so the restraint above is enforced at
//!   the boundary rather than reviewed.
//! - [`plan`] is the increment document (`D_t`): one bounded scope, and a preservation set
//!   **derived** from [`state`] rather than written by whoever planned it.
//!
//! `docs/adr/0013-one-evidence-vocabulary-with-two-existing-projections.md` decides the vocabulary
//! those last two speak; `docs/adr/0014-the-evidence-state-is-append-only-and-derives-what-it-can.md`
//! decides how it is stored.

pub mod fingerprint;
pub mod plan;
pub mod regrades;
pub mod state;
pub mod transcript;

/// The repository root, from this crate's manifest location — a *development* default.
///
/// `None`, never a panic. `CARGO_MANIFEST_DIR` is the build machine's source path baked into the
/// binary, so a binary that outlives its tree, or one carried to a machine that never had it, has
/// no root to find here and must be told one. This used to `.expect(…)`, and `ironworks-site`
/// called it on an ordinary path — so a generator built once and run later aborted rather than
/// saying what it needed.
pub fn root() -> Option<std::path::PathBuf> {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().ok()
}

/// [`root`], for tests, which run from the tree they measure.
#[cfg(test)]
pub(crate) fn test_root() -> std::path::PathBuf {
    root().expect("the tests run from the source tree they measure")
}
