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
pub mod dev_costs;
pub mod layering;
pub mod members;
pub mod paths;
pub mod self_description;
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
    /// **What was checked, stably.** The identity a later run uses to say that this same claim's
    /// verdict *moved* — so it must not change when the tree does.
    ///
    /// For most checks that is the check's own name and identical to `subject`. It is separate
    /// because for some it cannot be: the tally check reports one finding per transcript and its
    /// subject is a line anchor like `README.md:164`, which moves whenever the lines above it do.
    /// Recording that as an identity made the same claim arrive at the next run as a new one
    /// (measured: 27 of 43 subjects were line anchors), which is the one thing an evidence state
    /// exists to show. `subject` stays the anchor because it is genuinely where a reader should
    /// look, and because `self_description::is_document` filters on that shape.
    pub claim: String,
    /// **Where to look.** A check name, a file, or a `path:line` anchor — chosen for a human
    /// reading the report, not for equality across runs.
    pub subject: String,
    pub detail: String,
}

impl Finding {
    pub fn pass(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Verdict::Pass, subject, detail)
    }
    pub fn fail(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Verdict::Fail, subject, detail)
    }
    pub fn blocked(subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(Verdict::Blocked, subject, detail)
    }

    /// `claim` defaults to `subject`, which is correct for every check whose subject is its own
    /// name. A check whose subject is a location says otherwise with [`Self::claiming`].
    fn new(verdict: Verdict, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        let subject = subject.into();
        Self { verdict, claim: subject.clone(), subject, detail: detail.into() }
    }

    /// Give this finding a stable identity distinct from where it points.
    pub fn claiming(mut self, claim: impl Into<String>) -> Self {
        self.claim = claim.into();
        self
    }

    /// Claim a finding filed under a document by the check that produced it.
    ///
    /// A check that disagrees once per document reports under the document's name, because that is
    /// the file a reader opens — and until this, that name was also its identity. So one check read
    /// as `symbol citations` when it held and as `README.md` when it did not, two checks failing in
    /// one file merged into one claim, and the evidence recorder saw a claim vanish and another
    /// appear where one had simply moved. An identity given with [`Self::claiming`] is kept: a
    /// tally's transcript is a finer identity than its check's name, not a default to overwrite.
    fn within(mut self, check: &str) -> Self {
        if self.claim == self.subject && self_description::is_document(&self.subject) {
            self.claim = check.to_owned();
        }
        self
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
///
/// **That exemption covers paths, not citations.** A write-up's `path:line` and `path::symbol`
/// spans *are* checked — see [`paths::cited_files`] for why a pointer and a claim are held to
/// different standards, and what the exemption cost while it covered both.
///
/// `deploy/` is here because it was the largest document in the repository that no check read at
/// all. It is not a dated record — it is the operator procedure for a host that is currently
/// serving, so a path in it that no longer resolves is an instruction that fails at the point
/// someone follows it. It is one level, like the others: `deploy/user/` holds units, not prose.
pub const DOC_FILES: &[&str] = &["CLAUDE.md", "README.md", "CONTEXT.md", "DESIGN.md"];
pub const DOC_GLOB_DIRS: &[&str] = &["docs", "services", "deploy"];

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
/// `` `docs/PRD.md:12-14` `` (a line reference, carrying a colon) from the path check while
/// `` `docs/PRD.md` `` is checked. The colon-carrying form is not thereby unchecked: it is what
/// [`paths::anchored`] reads, with its own wider character set.
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

/// The four-digit number a name begins with, if it names a numbered experiment file.
///
/// # One rule, because there were four
///
/// `NNNN-` and then something. This test was written out four times with **three different length
/// bounds** — `> 5`, `< 5` inverted, `>= 5` — and two of the four never checked the hyphen at all,
/// so `00012-foo.md` read as experiment `0001` to the index checks and as nothing to the citation
/// checks. Nothing in the tree is named that way today, which is exactly why four copies could
/// disagree without anyone noticing.
pub fn experiment_number(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    (bytes.len() > 5 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-')
        .then(|| &name[..4])
}

/// Every readable document in `files`, as (the name a finding is filed under, its contents).
///
/// # The preamble this replaces appeared seven times
///
/// `for md in doc_files(root)`, then `let Ok(text) = std::fs::read_to_string(&md) else { continue }`,
/// then a `strip_prefix(root)` to get the repo-relative name. That `read_to_string` line alone is
/// written fourteen times in this crate.
///
/// **Unreadable is skipped rather than reported**, which is what every copy did: a document this
/// process cannot open is a filesystem problem, and the check that would notice a document that is
/// not there is [`paths::check`] naming it.
pub fn scan_docs(
    root: &std::path::Path,
    files: impl IntoIterator<Item = std::path::PathBuf>,
) -> Vec<(String, String)> {
    files
        .into_iter()
        .filter_map(|md| {
            let text = std::fs::read_to_string(&md).ok()?;
            Some((md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned(), text))
        })
        .collect()
}

/// A check's findings, or the one pass that says what it looked at.
///
/// Written out eleven times. `held` is a closure so the sentence — which usually counts what was
/// examined — is not built on the path where it is thrown away.
///
/// **An empty findings list means nothing disagreed, not that nothing was checked.** The
/// difference matters enough that several checks return a BLOCKED finding when they find nothing
/// to read, and `tests::every_check_that_can_run_in_the_suite_holds` asserts no check is silent.
pub fn reported(
    subject: &'static str,
    findings: Vec<Finding>,
    held: impl FnOnce() -> String,
) -> Vec<Finding> {
    if findings.is_empty() { vec![Finding::pass(subject, held())] } else { findings }
}

/// One check this binary runs.
///
/// # The registry existed three times
///
/// `main` listed thirteen `findings.extend(…)` calls by hand; `tests.rs` re-listed six of them as
/// "does the tree hold?" tests; and [`self_description`] compares what actually ran against the
/// prose in `CLAUDE.md`. A check added to `main` and not to `tests.rs` therefore ran in the binary
/// and not in the suite — so `cargo test --workspace` could report green over a claim the binary
/// would have refused, which is precisely what [`toolchain`] then asserts to `DESIGN.md` as
/// `passing`.
///
/// One table, iterated by all three, makes that unrepresentable.
pub struct Check {
    /// What this check is, for a reader of `main` and for a failing test's message.
    pub name: &'static str,
    /// Run it against a repository root.
    pub run: fn(&std::path::Path) -> Vec<Finding>,
    /// Whether `cargo test` may run it.
    ///
    /// **Two may not, and both for the same reason: they need a subprocess.** `commands` asks the
    /// built binary about its own flags, and `toolchain` shells out to cargo — a suite that ran
    /// either would be running itself. They are not exempt from being checked, only from being
    /// checked here; the binary runs them, and that is what `dev/prove-workspace.sh` invokes.
    pub in_suite: bool,
}

impl Check {
    /// Run it, with each finding it filed under a document claimed by this check's name.
    ///
    /// Here rather than in each check, because the registry is the one place that knows which check
    /// produced a finding — and a rule every check had to remember is a rule the next one forgets.
    /// `Finding::within` says what the identity it fixes was costing.
    pub fn findings(&self, root: &std::path::Path) -> Vec<Finding> {
        (self.run)(root).into_iter().map(|f| f.within(self.name)).collect()
    }
}

/// Every check, in the order `main` runs them.
///
/// [`self_description::check`] is deliberately absent: its input is this binary's own output
/// rather than the tree, so it cannot run until every entry below has reported.
pub const CHECKS: &[Check] = &[
    Check { name: "tallies", run: tallies::check, in_suite: true },
    Check { name: "documented commands", run: commands::check, in_suite: false },
    Check { name: "referenced paths", run: paths::check, in_suite: true },
    Check { name: "line anchors", run: paths::check_anchors, in_suite: true },
    Check { name: "symbol citations", run: paths::check_symbols, in_suite: true },
    Check { name: "cited experiments", run: paths::check_experiments, in_suite: true },
    Check { name: "experiment numbers", run: paths::check_experiment_numbers, in_suite: true },
    Check { name: "experiment index", run: paths::check_experiment_index, in_suite: true },
    Check { name: "named documents", run: paths::check_bare_docs, in_suite: true },
    Check { name: "workspace members", run: members::check, in_suite: true },
    Check { name: "layering", run: layering::check, in_suite: true },
    Check { name: "dev costs", run: dev_costs::check, in_suite: true },
    Check { name: "toolchain", run: toolchain::check, in_suite: false },
];

#[cfg(test)]
mod tests;
