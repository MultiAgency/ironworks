//! The workspace member count, against the prose that states it.
//!
//! `DESIGN.md`'s layering paragraph spells out how many crates the workspace has. `README.md`
//! stated it too until the count was consolidated into one place; both copies had been
//! hand-corrected twice in one working day, in opposite directions and to different wrong numbers,
//! while three crates landed. That is the shape this crate exists to catch: a count of something
//! the tree can count itself. The check still reads every document, so a second copy is caught the
//! day it reappears.
//!
//! # Why a word list rather than a number
//!
//! Both claims spell the count out. The one digit-form `N crates` claim in the tree —
//! `DESIGN.md`'s **lockfile count**, under "Dependency budget" — counts a different set entirely:
//! transitive dependencies, not workspace members. The figure itself is not repeated here, and that
//! is the point of the example: it read `88` when this was written and reads a larger number now,
//! so a comment quoting it would have to be re-edited every time the thing it is *not* about moves.
//! Reading only the token before `crates`, and only recognising it when it is a
//! spelled-out number, keeps those apart without a special case for the sentence that would
//! otherwise be the first false alarm. **A check that misfires is worse than no check**, because
//! the first false alarm teaches its reader to skim it.
//!
//! An unrecognised token is not a failure and not a pass: it is simply not a claim. If no document
//! states a count at all, the check reports BLOCKED rather than passing over an absence.

use crate::{Finding, doc_files, paths};
use std::path::Path;

/// Spelled-out counts this recognises, hyphenated compounds listed rather than parsed.
///
/// A table is longer than a parser and cannot be wrong about `twenty-one`. The range is bounded on
/// purpose: a workspace that grows past it stops matching, the claim stops being checked, and the
/// BLOCKED branch says so — which is the failure mode to prefer over a parser that guesses.
const WORDS: &[(&str, usize)] = &[
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("twenty", 20),
    ("twenty-one", 21),
    ("twenty-two", 22),
    ("twenty-three", 23),
    ("twenty-four", 24),
    ("twenty-five", 25),
    ("twenty-six", 26),
    ("twenty-seven", 27),
    ("twenty-eight", 28),
    ("twenty-nine", 29),
    ("thirty", 30),
];

/// The count a sentence states, from the token immediately before `crates`.
///
/// Returns every stated count in the text, so a document naming one twice is checked twice rather
/// than at its first mention.
pub fn stated(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (idx, _) in text.match_indices("crates") {
        // `crates/domain` and friends are not counts; require a word boundary after.
        if text[idx..].chars().nth("crates".len()).is_some_and(|c| c == '/') {
            continue;
        }
        let before = text[..idx].trim_end();
        if before.len() == text[..idx].len() {
            continue; // no separating space: `subcrates`, and not a count
        }
        let token = before.rsplit(|c: char| c.is_whitespace()).next().unwrap_or_default();
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let lowered = token.to_ascii_lowercase();
        if let Some((_, n)) = WORDS.iter().find(|(w, _)| *w == lowered) {
            out.push(*n);
        }
    }
    out
}

/// Check every stated crate count against the workspace's actual membership.
pub fn check(root: &Path) -> Vec<Finding> {
    let actual = paths::owned_crates(root).len();
    if actual == 0 {
        return vec![Finding::blocked(
            "workspace members",
            "could not read members from Cargo.toml",
        )];
    }

    let mut claims = 0usize;
    let mut findings = Vec::new();
    for (where_, text) in crate::scan_docs(root, doc_files(root)) {
        for stated in stated(&text) {
            claims += 1;
            if stated != actual {
                findings.push(Finding::fail(
                    where_.clone(),
                    format!("says {stated} crates; the workspace has {actual}"),
                ));
            }
        }
    }

    if claims == 0 {
        return vec![Finding::blocked(
            "workspace members",
            format!("no document states a crate count; the workspace has {actual}"),
        )];
    }
    crate::reported("workspace members", findings, || {
        format!("{claims} stated count(s) match the {actual} members in Cargo.toml")
    })
}
