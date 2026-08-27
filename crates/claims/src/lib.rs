//! Fail when a written claim disagrees with what the tree measures.
//!
//! # Why this exists
//!
//! `DESIGN.md` opens its status block with *"Everything below is measured, not asserted."* Nothing
//! enforced that, and over one working day five separate claims drifted from their own evidence —
//! a test count, an experiment tally, a backup claim true of the config and false of the vault, a
//! commit inferred from symbol presence, and "nothing here is built" written after it was built.
//! Different files, one shape: **prose about a moving system, asserted rather than re-measured.**
//! Each was caught by a person looking, which is not a control. This is the control.
//!
//! # What it deliberately does not check
//!
//! `DESIGN.md` also claims a check count per proof script — "`dev/prove.sh` 9 checks". Counting
//! `verdict "<id>"` call sites gives 12, and the difference is not obviously drift: a shell script
//! names ids this cannot reliably map to logical checks. **A check that misfires is worse than no
//! check**, because the first false alarm teaches its reader to skim it. That claim is left
//! unverified rather than half-verified.
//!
//! # Verdicts
//!
//! PASS / FAIL / BLOCKED, and **BLOCKED is never a pass** — a measurement that could not be
//! performed has established nothing (`docs/INVARIANTS.md` §F-14).

pub mod commands;
pub mod members;
pub mod paths;
pub mod tallies;
pub mod toolchain;

/// The outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// The measurement could not be performed. Never a pass.
    Blocked,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Blocked => "BLOCKED",
        }
    }
}

/// One reported result: what was checked, how it went, and the sentence a reader acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub verdict: Verdict,
    pub subject: String,
    pub detail: String,
}

impl Finding {
    pub fn pass(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { verdict: Verdict::Pass, subject: subject.into(), detail: detail.into() }
    }
    pub fn fail(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { verdict: Verdict::Fail, subject: subject.into(), detail: detail.into() }
    }
    pub fn blocked(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { verdict: Verdict::Blocked, subject: subject.into(), detail: detail.into() }
    }
}

/// The repository root, from this crate's manifest location.
pub fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

/// The documents whose claims are checked.
///
/// `docs/experiments/*.md` is deliberately absent from the path check: an experiment is a record
/// of what was true when it ran, and may name a file that has since been deleted. Its tallies are
/// checked instead, by [`tallies`].
pub const DOC_FILES: &[&str] = &["CLAUDE.md", "README.md", "CONTEXT.md", "DESIGN.md"];
pub const DOC_GLOB_DIRS: &[&str] = &["docs", "services"];

/// Every markdown file whose prose is held to the tree.
pub fn doc_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<_> = DOC_FILES.iter().map(|f| root.join(f)).filter(|p| p.is_file()).collect();
    for dir in DOC_GLOB_DIRS {
        out.extend(markdown_in(&root.join(dir)));
    }
    // `fixtures/*/*.md` — one level down, matching the Python's glob.
    if let Ok(entries) = std::fs::read_dir(root.join("fixtures")) {
        for entry in entries.filter_map(Result::ok) {
            out.extend(markdown_in(&entry.path()));
        }
    }
    out.sort();
    out
}

pub(crate) fn markdown_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md"))
        .collect();
    out.sort();
    out
}

/// Text inside a pair of backticks, where the content is drawn from `allowed`.
///
/// Hand-scanned rather than matched. The one subtlety is that a span containing anything outside
/// `allowed` is **not** a match at all rather than a truncated one — which is what exempts
/// `` `fixtures/…/cases.py:86-127` `` (a line reference, carrying a colon) from the path check
/// while `` `fixtures/…/cases.py` `` is checked.
pub fn backticked(text: &str, allowed: impl Fn(char) -> bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && allowed(chars[j]) {
            j += 1;
        }
        if j > i + 1 && chars.get(j) == Some(&'`') {
            out.push(chars[i + 1..j].iter().collect());
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;
