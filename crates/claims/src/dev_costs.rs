//! What a `dev/` script costs, against the two documents that list it.
//!
//! # Why this exists
//!
//! `CLAUDE.md` opened its cost section with *"Most of `dev/` is free and repeatable. These are
//! not"* and then named seven scripts. `docs/AUDIT-BRIEF.md` named those seven and three more —
//! among them `dev/probe-up.sh`, which **bills one model turn per bring-up** and is the
//! prerequisite of most of the seven. So the always-loaded brief told an agent that the one script
//! it would certainly run was free, and the correction lived in a file written for somebody else.
//!
//! Four copies of one fact existed and two of them disagreed. **The script's own header is the
//! only copy that cannot drift from what it describes**, so that is where the fact lives now, on a
//! `# COST:` line, and the documents are derived lists this check holds to it.
//!
//! # Both directions, in both lists, because the negative half is the claim
//!
//! *"Free is stated, never inferred from absence"* is only worth anything if something checks it.
//! A check that verified the costly scripts alone would pass a tree where a new costly script was
//! named nowhere — which is precisely the failure it was written for. So each document carries two
//! lists, and a script must be in the one its header names and not in the other: costly in the
//! costly list, free in the free one. The free half was missing at first — a script marked free
//! and named in neither document passed, while both documents said that was a defect — which is
//! the whole of this crate's premise in one sentence: that was prose, not a control. §F-14: a
//! negative check needs a positive control, and here each direction is the other's.
//!
//! # Scope, stated exactly
//!
//! **Executable `dev/*.sh`.** `dev/lib.sh` is not executable and says *"Sourced, never executed"*
//! — it is a library, has no cost of its own, and demanding a marker on it would be a check
//! misfiring on correct writing. The boundary is the file mode, which the tree carries, rather
//! than a name list here that somebody has to maintain.

use crate::Finding;
use std::path::Path;

/// Each document's two lists, as the strings that bound them: the costly list runs from the first
/// marker to the second, and the free list from the second to the third.
///
/// A region is bounded rather than searched whole, because both files name the free scripts a few
/// lines below the costly ones — searching the section would find every script in it and check
/// nothing. The markers are prose the documents state deliberately; when one is missing the check
/// reports BLOCKED rather than passing over an absence.
const LISTS: &[(&str, &str, &str, &str)] = &[
    ("CLAUDE.md", "## What costs something to run", "These cost nothing", "**Free is stated"),
    (
        "docs/AUDIT-BRIEF.md",
        "### Commands you must not run",
        "You **may** run these",
        "**Free is stated",
    ),
];

/// Every executable `dev/*.sh`, paired with the cost its header declares.
///
/// `None` where the script carries no `# COST:` line at all — an absence the caller reports rather
/// than guessing a cost for.
pub fn declared(root: &Path) -> Vec<(String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(root.join("dev")) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "sh") || !is_executable(&path) {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let cost =
            text.lines().find_map(|l| l.strip_prefix("# COST:")).map(|c| c.trim().to_owned());
        out.push((format!("dev/{name}"), cost));
    }
    out.sort();
    out
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Whether a declared cost says the script costs nothing.
///
/// One spelling, checked at the front of the line, so `free — no runtime, no model key` is free and
/// *"the probe — its first act is to tear the stack down"* is not. A cost sentence that merely
/// contains the word somewhere is not thereby free.
pub fn is_free(cost: &str) -> bool {
    cost.starts_with("free")
}

/// The slice of a document in which a costly script must be named.
pub fn region<'a>(text: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let start = text.find(from)?;
    let rest = &text[start..];
    let end = rest.find(to)?;
    Some(&rest[..end])
}

/// Check every executable `dev/*.sh` against the cost lists that name it.
pub fn check(root: &Path) -> Vec<Finding> {
    let scripts = declared(root);
    if scripts.is_empty() {
        return vec![Finding::blocked("dev costs", "found no executable dev/*.sh to check")];
    }
    let mut lists = Vec::new();
    for (doc, costly, free, end) in LISTS {
        let Ok(text) = std::fs::read_to_string(root.join(doc)) else {
            return vec![Finding::blocked("dev costs", format!("{doc} could not be read"))];
        };
        let (Some(costly_list), Some(free_list)) =
            (region(&text, costly, free), region(&text, free, end))
        else {
            return vec![Finding::blocked(
                "dev costs",
                format!("{doc} has no \"{costly}\" … \"{free}\" … \"{end}\" lists to read"),
            )];
        };
        lists.push((*doc, costly_list.to_owned(), free_list.to_owned()));
    }

    let mut findings = Vec::new();
    for (script, cost) in &scripts {
        let Some(cost) = cost else {
            findings.push(Finding::fail(
                "dev costs",
                format!("{script} declares no `# COST:` line, so nothing states what it spends"),
            ));
            continue;
        };
        // Backticked, as both lists write every script, so a name is matched rather than a prefix.
        let named = format!("`{script}`");
        for (doc, costly_list, free_list) in &lists {
            let (as_costly, as_free) = (costly_list.contains(&named), free_list.contains(&named));
            let why = match (is_free(cost), as_costly, as_free) {
                (false, true, false) | (true, false, true) => continue,
                (false, false, _) => format!("{script} costs something and {doc} does not name it"),
                (false, true, true) => {
                    format!("{script} costs something and {doc} also lists it as free")
                }
                (true, true, _) => {
                    format!("{script} declares itself free and {doc} lists it as costing something")
                }
                (true, false, false) => format!(
                    "{script} declares itself free and {doc} does not say so — free is stated, \
                     never inferred from absence"
                ),
            };
            findings.push(Finding::fail("dev costs", why));
        }
    }
    crate::reported("dev costs", findings, || {
        format!("{} dev script(s) declare a cost the documents agree with", scripts.len())
    })
}
