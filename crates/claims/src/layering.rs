//! `DESIGN.md`'s dependency tables, against the manifests they describe.
//!
//! # Why this exists
//!
//! The layering paragraph says **"Cargo enforces this. It is not a convention anyone has to
//! remember."** Cargo enforces the *rule*; nothing enforced the *table*, and the table is what a
//! reader reads. It said `site` depends on `graders`, and carried a paragraph justifying that edge
//! — the published page was to state a refusal rate computed by the same instrument that graded
//! the answers. `docs/FRONTEND.md` records that the statistic turned out not to be computable. The
//! edge went, the justification stayed, and the row still named a dependency the workspace does not
//! have while omitting `services`, which it does.
//!
//! That is the shape this crate exists to catch: **prose about something the tree can read for
//! itself.** A dependency column is exactly that.
//!
//! # The two columns, and why they are read differently
//!
//! - **Depends on** — the workspace crates a package names, written without the `ironworks-`
//!   prefix. `nothing` and `—` both mean the empty set.
//! - **Owns** — the IO a package declares: everything in `[dependencies]` that is neither a
//!   workspace crate nor one of [`NO_EFFECT`]. `DESIGN.md` states that definition beside the table,
//!   so this is reading a documented contract rather than inventing one. Parsing is not an effect,
//!   which is why `serde` sits in that set and `rusqlite` does not.
//!
//! # What it deliberately does not check
//!
//! **Nothing about the ASCII diagram above the tables**, and no ordering within a cell. A diagram
//! is a drawing and cannot be compared without deciding what a line means; a cell's order is
//! writing. Both would be checks that misfire on correct prose, and **a check that misfires is
//! worse than no check** — the first false alarm teaches its reader to skim it.

use crate::{Finding, paths};
use std::collections::BTreeSet;
use std::path::Path;

/// Dependencies that carry no effect, so a crate naming one still owns no IO.
///
/// `DESIGN.md` names this set beside the table. `domain` carries `serde` because record shapes
/// parse from the store's JSON and parsing is not an effect — the alternative, a separate crate
/// `domain` could not call, is what once kept the records envelope outside the verification gate.
pub const NO_EFFECT: &[&str] = &["serde", "serde_json", "sha2", "thiserror"];

/// The `[dependencies]` a manifest declares, split into workspace crates and everything else.
///
/// Hand-scanned rather than parsed. The section runs to the next `[`-headed line, and a key is
/// what stands left of the first `=` — enough for manifests written the way this workspace writes
/// them, and it reports [`Finding::blocked`] rather than guessing when it finds nothing.
pub fn declared(manifest: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut workspace = BTreeSet::new();
    let mut external = BTreeSet::new();
    let mut inside = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[dependencies]";
            continue;
        }
        if !inside || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((key, _)) = t.split_once('=') else { continue };
        let key = key.trim();
        match key.strip_prefix("ironworks-") {
            Some(name) => {
                workspace.insert(name.to_owned());
            }
            None if NO_EFFECT.contains(&key) => {}
            None => {
                external.insert(key.to_owned());
            }
        }
    }
    (workspace, external)
}

/// One table row: the crate it names, which column it was under, and the set the cell states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub crate_name: String,
    /// `true` when the row came from a table whose last column is `Owns`.
    pub owns: bool,
    pub stated: BTreeSet<String>,
    /// Entries in the cell that are neither a backticked name nor `nothing` or `—`.
    ///
    /// Kept rather than skipped. A dependency written as plain text used to be dropped before the
    /// comparison, so `` `y`, graders `` read as `{y}` and the manifest was held to a row that said
    /// less than it appeared to.
    pub unread: Vec<String>,
}

/// Every row of every crate table in `DESIGN.md`.
///
/// A table qualifies when its header row's first cell is `Crate` and its last is `Depends on` or
/// `Owns`. Rows are read until the first line that is not a table row, so a table ending at a
/// paragraph and a table ending at another heading are both handled without a special case.
pub fn rows(text: &str) -> Vec<Row> {
    let mut out = Vec::new();
    let mut owns = None;
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            owns = None;
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        let (Some(first), Some(last)) = (cells.first(), cells.last()) else { continue };
        if *first == "Crate" {
            owns = match *last {
                "Owns" => Some(true),
                "Depends on" => Some(false),
                _ => None,
            };
            continue;
        }
        let Some(owns) = owns else { continue };
        // The `|---|---|` separator, and nothing else, is all dashes.
        if cells.iter().all(|c| c.chars().all(|ch| ch == '-')) {
            continue;
        }
        let Some(name) = first.strip_prefix('`').and_then(|n| n.strip_suffix('`')) else {
            continue;
        };
        let mut stated = BTreeSet::new();
        let mut unread = Vec::new();
        for part in last.split(',') {
            let part = part.trim().trim_matches('*').trim();
            if part == "nothing" || part == "—" || part.is_empty() {
                continue;
            }
            match part.strip_prefix('`').and_then(|d| d.strip_suffix('`')) {
                Some(dep) => {
                    stated.insert(dep.to_owned());
                }
                None => unread.push(part.to_owned()),
            }
        }
        out.push(Row { crate_name: name.to_owned(), owns, stated, unread });
    }
    out
}

/// Check `DESIGN.md`'s dependency tables against `crates/*/Cargo.toml`.
pub fn check(root: &Path) -> Vec<Finding> {
    let design = root.join("DESIGN.md");
    let Ok(text) = std::fs::read_to_string(&design) else {
        return vec![Finding::blocked("layering", "DESIGN.md could not be read")];
    };
    let rows = rows(&text);
    if rows.is_empty() {
        return vec![Finding::blocked(
            "layering",
            "found no `| Crate |` table with a `Depends on` or `Owns` column in DESIGN.md",
        )];
    }
    let members = paths::owned_crates(root);
    let mut findings = Vec::new();
    for row in &rows {
        let dir = format!("crates/{}/", row.crate_name);
        if !members.contains(&dir) {
            findings.push(Finding::fail(
                "layering",
                format!("DESIGN.md names `{}`, which is not a workspace member", row.crate_name),
            ));
            continue;
        }
        let Ok(manifest) = std::fs::read_to_string(root.join(&dir).join("Cargo.toml")) else {
            findings
                .push(Finding::blocked("layering", format!("{dir}Cargo.toml could not be read")));
            continue;
        };
        let (workspace, external) = declared(&manifest);
        let (actual, column) =
            if row.owns { (external, "owns") } else { (workspace, "depends on") };
        if !row.unread.is_empty() {
            findings.push(Finding::fail(
                "layering",
                format!(
                    "DESIGN.md's `{}` row {column} writes {} without backticks, so it is compared \
                     against nothing",
                    row.crate_name,
                    row.unread.join(", ")
                ),
            ));
        }
        if actual == row.stated {
            continue;
        }
        let missing: Vec<&str> = actual.difference(&row.stated).map(String::as_str).collect();
        let phantom: Vec<&str> = row.stated.difference(&actual).map(String::as_str).collect();
        let mut why = Vec::new();
        if !phantom.is_empty() {
            why.push(format!("names {}, which it does not", phantom.join(", ")));
        }
        if !missing.is_empty() {
            why.push(format!("omits {}, which it does", missing.join(", ")));
        }
        findings.push(Finding::fail(
            "layering",
            format!("DESIGN.md's `{}` row {column}: {}", row.crate_name, why.join("; ")),
        ));
    }
    // **The completeness direction, and it is the half a row-by-row check cannot see.** Everything
    // above reads rows that exist and asks whether each is right. A crate added to the workspace
    // and to no table is invisible to that: there is no row to be wrong, so the check stays green
    // over a diagram that has stopped describing the workspace. Both halves are the same defect
    // (§K-8) — a harness can invent a match as easily as a difference — and this is the one the
    // reader is least likely to notice, because nothing looks missing until you go counting.
    let undocumented: Vec<String> = members
        .iter()
        .filter_map(|dir| dir.strip_prefix("crates/").and_then(|d| d.strip_suffix('/')))
        .filter(|name| !rows.iter().any(|r| r.crate_name == *name))
        .map(str::to_owned)
        .collect();
    if !undocumented.is_empty() {
        findings.push(Finding::fail(
            "layering",
            format!(
                "DESIGN.md's tables have no row for {} — a crate the layering does not describe is \
                 one nobody can place in it",
                undocumented.join(", ")
            ),
        ));
    }

    crate::reported("layering", findings, || {
        format!(
            "{} dependency row(s) in DESIGN.md match their manifests, and every workspace \
             member has one",
            rows.len()
        )
    })
}
