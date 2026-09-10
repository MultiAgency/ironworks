//! Documented invocations, asked of the parser rather than of a list kept in step by hand.
//!
//! `docs/ACCOUNT-COORDINATION.md` documented `relay --channel-id <id>` for hours after the flag
//! was removed, and nothing noticed: the prose is not compiled and the binary is not read. Asking
//! the binary is free and touches nothing — argument parsing happens before any file is opened or
//! any socket dialled.

use crate::{Finding, doc_files};
use std::path::Path;

/// A documented invocation: the verb, and the long flags given to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub verb: String,
    pub flags: Vec<String>,
}

/// Every `ironworks <verb> … --flag` inside a fenced block.
///
/// **Continuation lines matter.** The invocations in these docs are wrapped with backslashes, and
/// a reader that took only the first line would miss every flag but the first — which is most of
/// them.
pub fn documented(text: &str) -> Vec<Invocation> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut fenced = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            i += 1;
            continue;
        }
        let Some(rest) = fenced.then(|| invocation(line)).flatten() else {
            i += 1;
            continue;
        };
        let (verb, mut tail) = rest;
        while tail.trim_end().ends_with('\\') && i + 1 < lines.len() {
            i += 1;
            tail = format!("{} {}", tail.trim_end().trim_end_matches('\\'), lines[i]);
        }
        let words: Vec<&str> = tail.split_whitespace().collect();
        // `lifecycle inspect` is one verb in two words; everything else is one.
        let verb = if verb == "lifecycle" && words.first().is_some_and(|w| !w.starts_with('-')) {
            format!("lifecycle {}", words[0])
        } else {
            verb
        };
        out.push(Invocation {
            verb,
            flags: words.iter().filter(|w| w.starts_with("--")).map(|w| (*w).to_owned()).collect(),
        });
        i += 1;
    }
    out
}

/// `[$ ]ironworks <verb><rest>` — the verb and everything after it.
fn invocation(line: &str) -> Option<(String, String)> {
    let t = line.trim_start().strip_prefix("$ ").unwrap_or(line.trim_start());
    let rest = t.strip_prefix("ironworks ")?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let verb = parts.next()?.to_owned();
    if verb.is_empty() {
        return None;
    }
    Some((verb, parts.next().unwrap_or("").to_owned()))
}

/// Ask the binary whether it knows a flag. Parse-only: nothing is read, opened or sent.
///
/// **`Err` rather than `false` when the binary will not run**, because the two mean opposite
/// things and the first version conflated them. `false` is "the parser accepted this flag" —
/// which [`check`] reports as a pass — so a binary that was present but unspawnable certified
/// every documented flag in the tree. `toolchain::cargo_ok` already treats an unrunnable
/// subprocess as BLOCKED; two subprocess checkers in one crate must not disagree about what
/// "could not measure" means. [`check`]'s `seen.is_empty()` guard covers *read* nothing; this
/// covers *ran* nothing.
fn rejects(binary: &Path, root: &Path, verb: &str, flag: &str) -> Result<bool, String> {
    let mut cmd = std::process::Command::new(binary);
    cmd.current_dir(root);
    for word in verb.split_whitespace() {
        cmd.arg(word);
    }
    let out = cmd
        .args([flag, "x"])
        .output()
        .map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(text.contains(&format!("unknown flag {flag}")))
}

/// Check that every documented flag is one the binary's parser accepts.
pub fn check(root: &Path) -> Vec<Finding> {
    let binary = root.join("target/debug/ironworks");
    if !binary.is_file() {
        return vec![Finding::blocked(
            "documented commands",
            format!("no binary at {}; cargo build first", binary.display()),
        )];
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    for (where_, text) in crate::scan_docs(root, doc_files(root)) {
        for inv in documented(&text) {
            for flag in &inv.flags {
                if !seen.insert((inv.verb.clone(), flag.clone())) {
                    continue;
                }
                match rejects(&binary, root, &inv.verb, flag) {
                    Ok(false) => {}
                    Ok(true) => findings.push(Finding::fail(
                        where_.clone(),
                        format!(
                            "documents `{} {flag}`, which the binary rejects as unknown",
                            inv.verb
                        ),
                    )),
                    // One unrunnable spawn blocks the whole check rather than this one flag: the
                    // binary is the same for every row, so the next spawn has no reason to differ,
                    // and reporting the rest as passing would be the vacuous half of a run nobody
                    // could act on.
                    Err(why) => return vec![Finding::blocked("documented commands", why)],
                }
            }
        }
    }
    if seen.is_empty() {
        return vec![Finding::blocked(
            "documented commands",
            "found no ironworks invocations to check",
        )];
    }
    crate::reported("documented commands", findings, || {
        format!("{} documented flag(s) all parse", seen.len())
    })
}
