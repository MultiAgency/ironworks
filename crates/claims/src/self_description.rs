//! The list of checks in `CLAUDE.md`, against the checks that actually run.
//!
//! # Why this exists
//!
//! `CLAUDE.md`'s "What is machine-checked, and what is not" enumerated ten things and omitted
//! [`crate::paths::check_experiment_index`] — the check that keeps the experiment index complete.
//! A whole planning pass then treated that completeness as an open routing failure to be fixed by
//! hand, because nothing said it was already guarded. **An unlisted check is a check somebody
//! re-discovers**, and re-discovering it costs more than the check saved.
//!
//! The same section also said the citation checks read `docs/experiments/` and *"only those two"*,
//! three lines above a parenthesis naming three families of check that read it. A section that
//! disagrees with itself within one screen is not a description anybody can act on.
//!
//! # Why the subject, and not a count
//!
//! It compares the **subject** each check reports under. A count would move whenever a check
//! reports per-document findings rather than one summary, which several do — churn rather than
//! signal, the reason the old test-total check was deleted. A subject is a name: it changes only
//! when somebody adds, removes or renames a check, and that is exactly when the prose should move.
//!
//! Subjects that name a document rather than a check — `tallies` reports under the file that
//! carries the claim — are not held to this, because they are not names anybody could enumerate.
//!
//! # And the registry, because subjects alone had two holes
//!
//! A check whose findings that run were all failures filed under documents reported no subject of
//! its own, so the prose was not asked to name it at exactly the moment it mattered. And a check
//! that could not run reported under a name of its own — `toolchain claims` — that no prose had
//! reason to contain, so its BLOCKED arrived as this check's FAIL and the binary exited 2,
//! *"measured, and wrong"*, over a measurement that never happened. Every name in
//! [`crate::CHECKS`] is therefore required too, whether or not it reported under it.

use crate::Finding;
use std::path::Path;

/// The section whose prose must name every check, and the heading that ends it.
const SECTION: (&str, &str) = ("## What is machine-checked, and what is not", "## House rules");

/// Subjects that are a document's name rather than a check's, and so are not enumerable prose.
///
/// A finding filed under `README.md` or `docs/PRD.md` is one document's disagreement, reported
/// there so a reader knows which file to open. Requiring `CLAUDE.md` to name it would require it to
/// list every document, which the section already does in its first sentence.
pub(crate) fn is_document(subject: &str) -> bool {
    subject.ends_with(".md") || subject.contains(".md:")
}

/// Check that `CLAUDE.md` names every registered check, and every check subject the binary reports.
pub fn check(root: &Path, checks: &[&str], findings: &[Finding]) -> Vec<Finding> {
    let Ok(text) = std::fs::read_to_string(root.join("CLAUDE.md")) else {
        return vec![Finding::blocked("self-description", "CLAUDE.md could not be read")];
    };
    let Some(start) = text.find(SECTION.0) else {
        return vec![Finding::blocked(
            "self-description",
            format!("CLAUDE.md has no \"{}\" section", SECTION.0),
        )];
    };
    let rest = &text[start..];
    let section = &rest[..rest.find(SECTION.1).unwrap_or(rest.len())];

    // **Its own subject, because it cannot be in `findings` yet.** This check runs last and reports
    // afterwards, so the one name guaranteed to be missing from its input is the one it files
    // under — and a check about unlisted checks that exempts itself is the joke writing itself.
    let mut subjects: Vec<&str> = checks
        .iter()
        .copied()
        .chain(findings.iter().map(|f| f.subject.as_str()).filter(|s| !is_document(s)))
        .chain(std::iter::once("self-description"))
        .collect();
    subjects.sort_unstable();
    subjects.dedup();

    let missing: Vec<&str> = subjects.iter().copied().filter(|s| !section.contains(s)).collect();
    if missing.is_empty() {
        return vec![Finding::pass(
            "self-description",
            format!("CLAUDE.md names all {} check(s)", subjects.len()),
        )];
    }
    vec![Finding::fail(
        "self-description",
        format!(
            "CLAUDE.md's machine-checked section names no {} — a check nobody knows about is one \
             somebody redoes by hand",
            missing.join(", no ")
        ),
    )]
}
