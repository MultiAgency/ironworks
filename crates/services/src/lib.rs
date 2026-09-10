//! Embedded services: persona fragments, skills, safety text, and service definitions.
//!
//! A *piece* of scrap is one markdown fragment. The melt fuses a service's pieces together with
//! that tenant's guidance and the safety tail to produce an alloy — the instructions the runtime
//! is sent every turn. A *service definition* names which pieces a service uses.
//!
//! # Why these are one crate
//!
//! They were two, and the split did not survive looking at it: both compile a repo-root tree into
//! the binary at build time, both exist so that what an artifact carries is decided once rather
//! than looked up, and the markdown crate already had a dev-dependency on the definitions crate
//! because a piece cannot be tested without a service that names it. Two names, one boundary.
//!
//! What this crate is *not* is a place for shared types or helpers. Its narrowness is the point:
//! what goes to a model is worth being able to enumerate exactly.
//!
//! Everything here is embedded at build time, so the service library travels with the artifact. There is
//! no filesystem dependency and no path resolution, which is why [`digest`] can honestly claim to
//! fingerprint *what this binary will actually send*.

pub mod service;

use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/scrap.rs"));

/// One piece's contents, by repo-relative path (e.g. `personas/COORDINATOR.md`).
pub fn read(rel: &str) -> Option<&'static str> {
    SCRAP.iter().find(|(p, _)| *p == rel).map(|(_, body)| *body)
}

/// Whether a piece exists. Service-definition validation uses this to fail closed at load
/// rather than mid-composition.
pub fn contains(rel: &str) -> bool {
    SCRAP.iter().any(|(p, _)| *p == rel)
}

/// Every path, sorted.
pub fn paths() -> impl Iterator<Item = &'static str> {
    SCRAP.iter().map(|(p, _)| *p)
}

pub fn count() -> usize {
    SCRAP.len()
}

/// A fingerprint of the whole heap: the first 16 hex characters of a SHA-256 over every path
/// and body, in sorted order.
///
/// Short and non-secret, so an operator can answer "is this binary carrying the scrap I think it
/// is?" without printing tens of kilobytes of prompt. Paths and bodies are both covered, so a
/// piece that grows a line changes the digest and so does one that is renamed.
pub fn digest() -> String {
    let mut hasher = Sha256::new();
    for (path, body) in SCRAP {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update(body.as_bytes());
        hasher.update([0u8]);
    }
    ironworks_domain::composition::digest16_of(hasher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heap_is_not_empty() {
        assert!(count() >= 3, "expected at least one persona, one skill and the tail");
    }

    #[test]
    fn every_expected_piece_is_present() {
        for expected in [
            "personas/COORDINATOR.md",
            "personas/_safety-tail.md",
            "skills/relationship-record/SKILL.md",
        ] {
            assert!(contains(expected), "missing piece: {expected}");
            assert!(!read(expected).unwrap().is_empty(), "empty piece: {expected}");
        }
    }

    #[test]
    fn an_absent_piece_reads_as_none() {
        assert!(read("personas/NOPE.md").is_none());
        assert!(!contains("personas/NOPE.md"));
    }

    #[test]
    fn paths_are_sorted_and_unique() {
        let all: Vec<_> = paths().collect();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        assert_eq!(all, sorted, "build.rs must emit sorted paths for a stable digest");
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(sorted, deduped);
    }

    #[test]
    fn the_digest_is_stable_and_short() {
        assert_eq!(digest().len(), 16);
        assert_eq!(digest(), digest());
    }

    /// Skills carry frontmatter and personas do not. The melt strips the former, so this records
    /// the shape rather than asserting a policy — if a persona grows frontmatter, composition
    /// behaviour changes and this test says so first.
    #[test]
    fn only_skills_carry_frontmatter() {
        for path in paths() {
            let has_frontmatter = read(path).unwrap().starts_with("---\n");
            let is_skill = path.starts_with("skills/");
            assert_eq!(
                has_frontmatter, is_skill,
                "{path}: frontmatter presence changed; check the crucible tests"
            );
        }
    }

    /// A safety tail that stopped mentioning safety would compose silently and change what every
    /// tenant's model is told.
    #[test]
    fn the_safety_tail_is_about_safety() {
        let tail = read("personas/_safety-tail.md").unwrap();
        assert!(tail.to_lowercase().contains("safety"), "the safety tail lost its subject");
    }
}

/// The embedded heap, as a [`PartSource`](ironworks_domain::PartSource).
///
/// This is the adapter that lets the pure domain compose against real markdown without ever
/// depending on it: `ironworks-domain` names the trait, this crate satisfies it, and a test can
/// substitute a `BTreeMap` for either.
#[derive(Debug, Clone, Copy, Default)]
pub struct Heap;

impl ironworks_domain::PartSource for Heap {
    fn read(&self, part: &ironworks_domain::PartRef) -> Option<&str> {
        read(part.as_str())
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use ironworks_domain::{PartRef, PartSource};

    #[test]
    fn the_heap_satisfies_the_domain_trait() {
        assert!(Heap.read(&PartRef::new("personas/COORDINATOR.md")).is_some());
        assert!(Heap.read(&PartRef::new("personas/NOPE.md")).is_none());
    }

    #[test]
    fn every_committed_service_composition_resolves_against_the_heap() {
        // The invariant that used to be a hand-written loop in the service loader.
        for name in crate::service::available() {
            let d = crate::service::load(name).unwrap();
            assert!(
                d.composition().missing_in(&Heap).is_empty(),
                "{name} names parts the heap does not hold"
            );
        }
    }
}
