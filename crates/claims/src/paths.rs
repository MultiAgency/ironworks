//! A document that names a file which no longer exists is a document nobody can follow.

use crate::{Finding, backticked, doc_files};
use std::path::Path;

/// Prefixes this repository unambiguously owns.
///
/// `site/` is deliberately absent: it is generated and gitignored, so asserting it exists would
/// fire on something correct.
///
/// **`deploy/` was absent for one specific reason, and excluded the whole directory to get it.**
/// The cause was the Python reference's own `deploy/lib`, which is one path, not a tree — and
/// while the whole of `deploy/` sat outside the check the gap was occupied: renaming the systemd
/// units left five citations in `docs/adr/0008` and four in experiments naming
/// `deploy/rustworks-ironclaw@.service`, and nothing could see any of them. So `deploy/` is ours
/// and [`FOREIGN`] carves out the part that is not, which is what the original reason actually
/// said.
const OWNED: &[&str] = &["dev/", "docs/", "fixtures/", "services/", "scrap/", "deploy/"];

/// Prefixes that sit under an [`OWNED`] one and belong to another tree.
///
/// `deploy/lib/` is the Python reference's, named in the reason `deploy/` was once excluded whole.
/// Asserting one of these exists would fire on a correct reference to a repository that is not
/// this one.
const FOREIGN: &[&str] = &["deploy/lib/"];

/// Workspace members, as `crates/<name>/` prefixes.
///
/// **Membership, not prefix.** The first version of this checker treated every `crates/…` path as
/// ours and immediately reported three failures: `crates/product/ironclaw_openai_compat/` and
/// `crates/kernel/ironclaw_host_runtime/` are inside the *pinned IronClaw source*, which
/// `DESIGN.md` and `docs/UPGRADE.md` cite precisely because reading the pinned source is the rule.
/// They are correct references to another tree — the exact misfire this module's header warns
/// about, produced within a minute of the check existing.
pub fn owned_crates(root: &Path) -> Vec<String> {
    let Ok(manifest) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return Vec::new();
    };
    manifest
        .lines()
        .filter_map(|line| {
            let t = line.trim().trim_end_matches(',').trim_matches('"');
            t.strip_prefix("crates/").map(|name| format!("crates/{name}/"))
        })
        .collect()
}

/// Whether a referenced path is one this repository is responsible for.
pub fn is_ours(reference: &str, crates: &[String]) -> bool {
    if FOREIGN.iter().any(|p| reference.starts_with(p)) {
        return false;
    }
    OWNED.iter().any(|p| reference.starts_with(p))
        || crates.iter().any(|p| reference.starts_with(p.as_str()))
}

/// The characters a repo-relative path is spelled with.
///
/// `@` is here because systemd template units carry one — `deploy/ironworks-ironclaw@.service` —
/// and [`crate::backticked`] requires *every* character between the backticks to pass, so without
/// it the whole span was skipped rather than truncated. Widening [`OWNED`] to include `deploy/`
/// caught nothing until this changed: the nine stale unit citations were invisible one layer
/// lower down, and the control asserting they are caught is the only reason that surfaced.
///
/// It does not widen what counts as ours. `ironclaw@<rev>/crates/…` now parses as a token and
/// [`is_ours`] still rejects it, which is the intended reading — an upstream anchor is a correct
/// reference to another tree.
fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '@')
}

/// Every file whose **citations** are held to the tree: the checked documents, the write-ups, and
/// the ADRs.
///
/// # Why the write-ups and ADRs are added back here and nowhere else
///
/// [`crate::doc_files`] leaves `docs/experiments/` out on purpose, and that exemption is about
/// *paths*: an experiment records what was true when it ran and may legitimately name a file that
/// has since been deleted. **A citation is the opposite case.** It is a pointer, not a claim, and a
/// pointer that no longer resolves is wrong in the present tense however old the record is —
/// repairing one revises nothing the experiment measured.
///
/// The exemption cost exactly what it predicted. `0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`
/// cited `crates/cli/src/main.rs:2411` for a `match` on continuation; the file grew by some eight
/// hundred lines and that number came to land in the middle of an argument list, with nothing to
/// say so. Six more citations wrote IronClaw's tree as a bare `crates/product/…`, which reads as
/// ours and is not — [`is_ours`] correctly skipped them, so the missing `ironclaw@<rev>/` prefix
/// was invisible to both the checker and, more to the point, to a reader.
///
/// `docs/adr/` is here for the same reason and was missed for a duller one: `crate::markdown_in`
/// does not recurse, so a nested directory falls out of *every* check rather than out of the path
/// check alone. An ADR is a dated record like a write-up — it takes a pointer to what superseded
/// it, never an edit — so its paths are exempt and its citations are not.
pub fn cited_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = crate::doc_files(root);
    out.extend(crate::markdown_in(&root.join("docs/experiments")));
    out.extend(crate::markdown_in(&root.join("docs/adr")));
    out.sort();
    out.dedup();
    out
}

/// Every repo-relative path named in the checked documents, with the document that names it.
///
/// # Two places, because a path is written two ways
///
/// Inline prose spells a path in backticks, and that is what this originally read. But the
/// documents' **commands** are in fenced blocks, where a path is bare — and a command is the more
/// dangerous of the two, because a reader does not merely follow it, they paste it. The gap was
/// live: `fixtures/account-coordination/README.md` told a reader to run `dev/eval.py` for a whole
/// changeset after that script was deleted, in the same file whose neighbouring block had been
/// updated. Nothing noticed, because the only checked spelling was the one it did not use.
pub fn referenced(root: &Path) -> Vec<(String, String)> {
    let crates = owned_crates(root);
    let mut out = Vec::new();
    for (where_, text) in crate::scan_docs(root, doc_files(root)) {
        let inline = backticked(&text, path_char);
        for reference in inline.into_iter().chain(fenced_paths(&text)) {
            if is_ours(&reference, &crates) && !reference.ends_with('*') {
                out.push((where_.clone(), reference));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Bare paths inside fenced code blocks.
///
/// A block's contents are shell, so a token is split on whitespace and stripped of the punctuation
/// a command wraps a path in. Two things are deliberately **not** treated as paths, because both
/// are correct writing that this must not fail on:
///
/// - **Anything carrying a colon.** `crates/…/x.rs:12` is a line reference, and `docs/x.md:` ends
///   a sentence. [`backticked`] exempts these by refusing a span with a character outside its
///   set; here the token is trimmed and then checked, so the rule has to be stated.
/// - **A token that is only a prefix**, like `docs/` alone. It names a directory that exists, so
///   it would pass — but it is prose, and admitting it invites the check to start reporting on
///   sentence fragments.
pub(crate) fn fenced_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            continue;
        }
        if !inside {
            continue;
        }
        for token in line.split_whitespace() {
            // The punctuation a command puts around a path, and nothing that could be part of one.
            let token = token.trim_matches(|c: char| "\"'`(),;<>[]{}|$".contains(c));
            if token.len() < 2 || !token.contains('/') || token.ends_with('/') {
                continue;
            }
            // A line reference or a sentence's punctuation is not a path, and neither is a URL.
            if token.contains(':') || !token.chars().all(path_char) {
                continue;
            }
            out.push(token.to_owned());
        }
    }
    out
}

/// Whether a bare name is an experiment file: `NNNN-slug.md`, `.json` or `.log`.
///
/// Four leading digits and a hyphen, which is the numbering `docs/experiments/` uses and nothing
/// else in the tree does. Narrow on purpose — a looser rule would start matching ordinary prose.
fn is_experiment_file(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    matches!(ext, "md" | "json" | "log") && crate::experiment_number(stem).is_some()
}

/// Experiment files cited by bare name, which [`referenced`] cannot see.
///
/// Writeups and transcripts are cited as `` `0021-three-runs-three-answers.md` `` rather than by
/// repo-relative path, and a bare name has no owned prefix — so [`is_ours`] rejects it and the path
/// check skips it in silence. **Pruning a writeup therefore leaves every citation of it dangling
/// with nothing to notice**, which is exactly what happened when twenty-one writeups were
/// consolidated into one README: one dead citation survived in a document nothing checks it from.
///
/// Both spellings resolve against `docs/experiments/`, so both are checked: a backticked bare name
/// anywhere in the documents, and a Markdown link target inside `docs/experiments/` itself, where a
/// relative link can only mean a sibling.
pub fn cited_experiments(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // `doc_files` is deliberately one level deep, so it does not reach the writeups themselves —
    // and the index that links them all is one of them. Add that directory for this check only.
    let files =
        doc_files(root).into_iter().chain(crate::markdown_in(&root.join("docs/experiments")));
    for (where_, text) in crate::scan_docs(root, files) {
        for name in crate::backticked(&text, path_char) {
            if is_experiment_file(&name) {
                out.push((where_.clone(), name));
            }
        }
        // `](0001-alloy-drift.md)` — only meaningful beside the writeups it links to. `where_` is
        // repo-relative, so the directory is a prefix of it.
        if where_.starts_with("docs/experiments/") {
            for tail in text.split("](").skip(1) {
                let Some(name) = tail.split(')').next() else { continue };
                if is_experiment_file(name) {
                    out.push((where_.clone(), name.to_owned()));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Check that every experiment cited by bare name is still in `docs/experiments/`.
/// No two experiment files may claim one number.
///
/// # Why this is worth a check
///
/// An experiment number is an identity. Every write-up, every table row and every `0025`-style
/// citation in this repository resolves through it, and `cited_experiments` above matches on the
/// **name**, so two files numbered alike both exist and both pass — the ambiguity is invisible to
/// every check there was.
///
/// It is not hypothetical. Two `0037`s were written into this directory six minutes apart while
/// the tree was being edited from two places at once, and nothing but `git status` noticed. The
/// numbering has no lock and nothing hands one out, so the collision is a normal consequence of
/// two people working, not a mistake either of them made.
///
/// Transcripts and console logs are deliberately included. `0022` legitimately has two transcripts
/// and a log, so a shared number is only a collision among **write-ups** — the `.md` that reads a
/// run is the thing a citation resolves to, and there may be exactly one per number.
/// The numbered write-ups in `docs/experiments/`, by their number.
///
/// One enumeration for the two checks that need it, so a rule about which files are experiments
/// cannot come to mean two things.
fn numbered_write_ups(root: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut by_number: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for path in crate::markdown_in(&root.join("docs/experiments")) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        // `README.md` and anything else unnumbered is not an experiment.
        let Some(number) = crate::experiment_number(&name) else { continue };
        by_number.entry(number.to_owned()).or_default().push(name.clone());
    }
    by_number
}

/// Every number that exists in `docs/experiments/`, whatever kind of file carries it.
///
/// # Why this is wider than [`numbered_write_ups`]
///
/// The index check used to build its expectation from write-ups alone, so a run with a transcript
/// and no write-up was outside its domain entirely. Five were: `0004` through `0007` and `0009` sat
/// on disk with `- Recorded:` lines and appeared in neither of the index's tables. Their only
/// coverage was oblique — inside the `0003` row's *"the POC arc, runs 1–8"*, which named none of
/// them — so a citation of `0007` resolved to nothing in the documented entry point.
///
/// A pruned run is still a run. What the index owes it is a **name**, not a row of its own; the
/// tables already group numbers on one line, which is why the check asks only that the number
/// appear.
fn numbered_files(root: &Path) -> std::collections::BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(root.join("docs/experiments")) else {
        return std::collections::BTreeSet::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter_map(|n| crate::experiment_number(&n).map(str::to_owned))
        .collect()
}

/// Check that `docs/experiments/README.md` names every numbered write-up in its own directory.
///
/// # Why this is worth a check
///
/// The index is how a reader finds the run that measured a claim, which `CLAUDE.md` requires
/// before the claim is repeated. A write-up the index omits is one nobody will read first, so the
/// claim gets repeated from memory — the failure this repository's house rules exist to prevent.
///
/// It is not hypothetical, and it is not slow. Four write-ups had fallen out of the index; within
/// minutes of their being added back, two more landed from a second session editing the same tree
/// and the index was incomplete again. Nothing noticed either time. A gap that reopens in minutes
/// is one that has to be checked rather than swept, which is why this is a check and not a tidy.
///
/// It asks only that the number appear. What the row *says* is a judgement no checker can make,
/// and demanding a shape here would turn one honest omission into a format nobody maintains.
pub fn check_experiment_index(root: &Path) -> Vec<Finding> {
    let by_number = numbered_files(root);
    if by_number.is_empty() {
        return vec![Finding::blocked(
            "experiment index",
            "found no numbered files in docs/experiments",
        )];
    }
    let index = root.join("docs/experiments/README.md");
    let Ok(text) = std::fs::read_to_string(&index) else {
        return vec![Finding::blocked(
            "experiment index",
            "docs/experiments/README.md could not be read",
        )];
    };
    let missing: Vec<&str> = by_number
        .iter()
        .filter(|n| !text.contains(&format!("`{n}`")))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        return vec![Finding::pass(
            "experiment index",
            format!("docs/experiments/README.md names all {} numbered run(s)", by_number.len()),
        )];
    }
    vec![Finding::fail(
        "experiment index",
        format!(
            "docs/experiments/README.md names no {} — a run the index omits is one nobody \
             reads before repeating what it measured",
            missing.join(", no ")
        ),
    )]
}

pub fn check_experiment_numbers(root: &Path) -> Vec<Finding> {
    let by_number = numbered_write_ups(root);
    if by_number.is_empty() {
        return vec![Finding::blocked(
            "experiment numbers",
            "found no numbered write-ups in docs/experiments",
        )];
    }
    let findings: Vec<Finding> = by_number
        .iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(number, files)| {
            Finding::fail(
                "experiment numbers",
                format!(
                    "{} is claimed by {} write-ups ({}) — a number is an identity and a \
                     citation of it resolves to whichever one a reader opens first",
                    number,
                    files.len(),
                    files.join(", ")
                ),
            )
        })
        .collect();
    crate::reported("experiment numbers", findings, || {
        format!("{} numbered write-up(s), no number claimed twice", by_number.len())
    })
}

pub fn check_experiments(root: &Path) -> Vec<Finding> {
    let cited = cited_experiments(root);
    if cited.is_empty() {
        return vec![Finding::blocked(
            "cited experiments",
            "found no experiment files cited by name",
        )];
    }
    let dir = root.join("docs/experiments");
    let findings: Vec<Finding> = cited
        .iter()
        .filter(|(_, name)| !dir.join(name).is_file())
        .map(|(where_, name)| {
            Finding::fail(where_.clone(), format!("cites {name}, which is not in docs/experiments"))
        })
        .collect();
    crate::reported("cited experiments", findings, || {
        format!("{} experiment(s) cited by name all exist", cited.len())
    })
}

/// Markdown cited by bare filename, which every other check is blind to.
///
/// **Demonstrated, not anticipated.** `README.md` gained a section naming `RECON.md`,
/// `RECON-FOLLOWUPS.md`, `DEMO.md` and `CONTEXT-AUDIT.md`; all four were later deleted; and this
/// binary went on reporting every claim holding. A bare name has no owned prefix, so [`is_ours`]
/// rejects it and [`referenced`] skips it in silence — the same hole [`cited_experiments`] exists
/// to close, one directory up.
///
/// **Somewhere in the tree, not at the root.** `COORDINATOR.md` is cited bare and lives at
/// `scrap/personas/COORDINATOR.md`; a root-only rule would fail a citation that is perfectly good.
/// What a bare name claims is that the file exists *here*, and that is what is checked.
///
/// Experiment-shaped names are excluded because [`check_experiments`] already owns them, and owns
/// them more strictly: it requires `docs/experiments/` specifically, which this cannot.
pub fn bare_docs(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (where_, text) in crate::scan_docs(root, doc_files(root)) {
        for name in crate::backticked(&text, path_char) {
            if name.ends_with(".md") && !name.contains('/') && !is_experiment_file(&name) {
                out.push((where_.clone(), name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every markdown basename in the tree, so a bare citation can be resolved without guessing where.
fn markdown_basenames(root: &Path) -> std::collections::BTreeSet<String> {
    basenames(root).into_iter().filter(|(n, _)| n.ends_with(".md")).map(|(n, _)| n).collect()
}

/// Every file basename in the repository, with how many files carry it.
///
/// The count is the point. A bare name is only usable as an identifier when exactly one file
/// answers to it — `mod.rs` names twenty things and resolves to none of them.
fn basenames(root: &Path) -> std::collections::BTreeMap<String, usize> {
    fn walk(dir: &Path, out: &mut std::collections::BTreeMap<String, usize>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // Build output and generated pages are not the repository's own material.
                if !matches!(name.as_str(), "target" | ".git" | "site" | ".wrangler") {
                    walk(&path, out);
                }
            } else {
                *out.entry(name).or_default() += 1;
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, &mut out);
    out
}

/// Whether a bare basename names exactly one file in this repository.
///
/// [`is_ours`] asks whether a *path* is ours and answers on its prefix, so a citation written as
/// a bare name — `COORDINATOR.md:101`, `discipline.rs:124-129`, `model.rs:228-233` — was not ours
/// by that test and slipped [`check_anchors`] entirely. Those are line anchors into files that are
/// unambiguously the repository's own, which is the form the house rule exists to refuse; the
/// prefix test simply could not see them. Uniqueness is required so that an ambiguous name is
/// left alone rather than guessed at.
fn names_one_file_of_ours(
    reference: &str,
    index: &std::collections::BTreeMap<String, usize>,
) -> bool {
    !reference.contains('/') && index.get(reference) == Some(&1)
}

/// Check that every markdown file cited by bare name is still somewhere in the tree.
pub fn check_bare_docs(root: &Path) -> Vec<Finding> {
    let cited = bare_docs(root);
    if cited.is_empty() {
        return vec![Finding::blocked("named documents", "found no markdown cited by bare name")];
    }
    let present = markdown_basenames(root);
    let findings: Vec<Finding> = cited
        .iter()
        .filter(|(_, name)| !present.contains(name.as_str()))
        .map(|(where_, name)| {
            Finding::fail(where_.clone(), format!("names {name}, which is not in the tree"))
        })
        .collect();
    crate::reported("named documents", findings, || {
        format!("{} document(s) cited by bare name all exist", cited.len())
    })
}

/// A `path:line` or `path:start-end` citation into our own tree, and the highest line it names.
///
/// **These are refused now, and the reason is a measurement.** This function used to feed a check
/// that a cited line was within the file. It ran green over ten citations while **six of them
/// pointed at unrelated code** — drifts of 17, 21, 51, 76 and 168 lines, plus one naming a `Reading`
/// enum for a claim about `MemberConfinement::apply`. In-bounds is the one property a line number
/// keeps across an edit and the one property that carries no information, so a green here bought
/// nothing and spent a reader's trust: someone who follows a citation and lands on an argument
/// parser stops following citations.
///
/// Extending it was the alternative and it is worse. Deciding whether a line still *says* what the
/// citing sentence implies means either a second copy of the prose — the duplication the citation
/// exists to avoid — or a fuzzy match that reports differences that are not there, which is §K-8
/// of `docs/INVARIANTS.md` in the checker written to uphold it.
///
/// So the form is refused instead and [`symbol_citations`] replaces it: a symbol survives every
/// edit that does not rename or delete the thing it names, and fails loudly when one does.
/// **Anchors into the pinned IronClaw source are untouched** — that tree is pinned by SHA and
/// cannot drift under a citation, so a line number there is correct. [`is_ours`] is what separates
/// them, by workspace membership rather than by prefix.
///
/// Colons are why these are invisible to [`referenced`]: [`crate::backticked`] only matches a span
/// whose every character is in `path_char`, so a line-anchored citation is not a match at all.
pub fn anchored(root: &Path) -> Vec<(String, String, String)> {
    let crates = owned_crates(root);
    let index = basenames(root);
    let mut out = Vec::new();
    for (where_, text) in crate::scan_docs(root, cited_files(root)) {
        for span in crate::backticked(&text, |c| path_char(c) || matches!(c, ':' | ',' | '-')) {
            let Some((reference, spec)) = span.split_once(':') else { continue };
            // A second colon here is the `path::symbol` form, which is the *replacement* for this
            // one — [`symbol_citations`] owns it, and splitting on the first colon is what makes it
            // look like an anchor whose line is `:Stage::MemberConfined`.
            if spec.starts_with(':') {
                continue;
            }
            if !is_ours(reference, &crates) && !names_one_file_of_ours(reference, &index) {
                continue;
            }
            // **The form is refused, not the number.** This used to gate on the spec parsing as
            // digits, which quietly exempted every anchor that named anything else — so
            // `crates/cli/src/main.rs:12` was refused while `crates/cli/src/main.rs::USAGE` with
            // one colon was not, though it drifts exactly as silently and [`check_anchors`]
            // describes itself as looking for a form that must not come back.
            out.push((where_.clone(), reference.to_owned(), spec.to_owned()));
        }
    }
    out
}

/// Refuse a line anchor into our own tree.
///
/// **Empty is the goal, and an empty result is a pass rather than a BLOCKED.** That is the
/// opposite of [`check_experiments`] and it is deliberate: a reader that reads nothing passes
/// everything only where the check is looking *for* something. This one is looking for a form
/// that must not come back, so finding none of it is the result.
pub fn check_anchors(root: &Path) -> Vec<Finding> {
    let findings: Vec<Finding> = anchored(root)
        .iter()
        .map(|(where_, reference, spec)| {
            Finding::fail(
                where_.clone(),
                format!(
                    "cites {reference}:{spec} — a line anchor drifts silently; \
                     cite `path::symbol`, or quote a phrase where there is no symbol"
                ),
            )
        })
        .collect();
    if findings.is_empty() {
        return vec![Finding::pass("line anchors", "no `path:line` citation into our own tree")];
    }
    findings
}

/// A `path::symbol` citation into our own tree, and the symbol it names.
///
/// The replacement for the line anchors [`check_anchors`] refuses. `Stage::MemberConfined` keeps
/// both segments; the last is the one that has to be declared, because that is the name the
/// sentence is about.
pub fn symbol_citations(root: &Path) -> Vec<(String, String, String)> {
    let crates = owned_crates(root);
    let mut out = Vec::new();
    for (where_, text) in crate::scan_docs(root, cited_files(root)) {
        for span in crate::backticked(&text, |c| path_char(c) || c == ':') {
            let Some((reference, symbol)) = span.split_once("::") else { continue };
            // A path carries no colon of its own, so anything left of the first `::` that still
            // has one is not a citation of this shape.
            if reference.contains(':') || symbol.is_empty() || !is_ours(reference, &crates) {
                continue;
            }
            out.push((where_.clone(), reference.to_owned(), symbol.to_owned()));
        }
    }
    out
}

/// Whether `text` *declares* `name` — an item, an enum variant, or a struct field.
///
/// Presence is not enough. A symbol that moved to another module would keep passing wherever it
/// was still merely mentioned, which is the line-anchor failure in a different costume.
///
/// **The boundary, stated.** A tuple variant (`Turn(IdempotencyKey)`) is not found by the bare-name
/// arm, because `name(` is also what a call looks like and admitting it would let a citation pass
/// on a use site. Cite the enum in that case. This is a checker that would rather refuse a fair
/// citation than accept an unfair one.
fn declares(text: &str, name: &str) -> bool {
    const ITEM: &[&str] =
        &["fn", "struct", "enum", "trait", "const", "static", "type", "mod", "union", "impl"];
    let defined = defined_macros(text);
    text.lines().any(|line| {
        let mut token = line.trim();
        for modifier in ["pub(crate) ", "pub(super) ", "pub ", "async ", "unsafe ", "default "] {
            token = token.strip_prefix(modifier).unwrap_or(token);
        }
        if declared_by_macro(token, name, &defined) {
            return true;
        }
        match ITEM.iter().find_map(|k| token.strip_prefix(k)?.strip_prefix(' ')) {
            // `fn apply(`, `enum Reading {`, `impl Reading {`.
            Some(rest) => rest.strip_prefix(name).is_some_and(|t| !continues_a_name(t)),
            // An enum variant or a struct field carries no keyword: `MemberConfined,` and
            // `egress: WireAccess,`. `::` is excluded so a path at line start is not a match.
            None => token.strip_prefix(name).is_some_and(|t| {
                t.starts_with(',') || (t.starts_with(':') && !t.starts_with("::"))
            }),
        }
    })
}

/// Whether a shell script declares `name` as a function: `name()`.
///
/// Separate from [`declares`] because the ambiguity that one refuses does not exist here. In Rust
/// `name(` is also what a call looks like, so admitting it would let a citation pass on a use
/// site; in shell a call is `creates "$path"` with no parentheses at all, so `name()` can only be
/// a declaration. The same evidence means opposite things in the two languages.
///
/// It exists because the checker was reporting a **correct** citation as broken — the message
/// "`dev/lib.sh` does not declare `creates`" was false, and `dev/lib.sh` declares it as `creates()`
/// one line above `fetches()`. That is the failure mode [`declared_by_macro`] was written to avoid,
/// in a second costume: a check that misfires on correct prose teaches its reader to skim it.
/// Only the `name()` form, because that is the only one `dev/` writes. The `function name`
/// spelling is deliberately not recognised: a branch no script exercises is a branch nothing
/// keeps honest, and adding it would have to come with a script that uses it.
fn declares_shell(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        line.trim().strip_prefix(name).is_some_and(|t| t.trim_start().starts_with("()"))
    })
}

/// A declarative macro that names what it declares first: `pin_newtype!(ModelPin, "…")`.
///
/// Without this the checker reads a **correct** citation as broken, which is the failure mode its
/// own siblings are written to avoid — `DESIGN.md` cites `crates/domain/src/pins.rs::ModelPin`,
/// and that file declares `ModelPin` through a macro rather than a keyword. A check that misfires
/// on correct prose is worse than no check: the first false alarm teaches its reader to skim it.
///
/// Deliberately narrow, twice. The name must be the macro's **first** argument and must be followed
/// by a comma or the closing paren, so this recognises the naming convention rather than guessing
/// at macro bodies — and a macro that does not name what it declares is still not seen, which is
/// honest rather than clever.
///
/// **And the macro must be one the same file defines with `macro_rules!`.** Without that,
/// `assert!(ready);` names `ready` first and so declared it: a citation of `::ready` passed on a use
/// site, the exact thing [`declares`] refuses for `name(`. Every declaring macro in the tree is
/// defined in the file that invokes it, so this costs no fair citation.
fn declared_by_macro(token: &str, name: &str, defined: &[&str]) -> bool {
    let Some((invoked, args)) = token.split_once("!(") else { return false };
    defined.contains(&invoked)
        && args.strip_prefix(name).is_some_and(|t| t.starts_with(',') || t.starts_with(')'))
}

/// The macros `text` defines: `macro_rules! pin_newtype {` gives `pin_newtype`.
fn defined_macros(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("macro_rules!"))
        .filter_map(|rest| {
            rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).find(|s| !s.is_empty())
        })
        .collect()
}

fn continues_a_name(rest: &str) -> bool {
    rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
}

/// Check that every `path::symbol` citation names something its file still declares.
pub fn check_symbols(root: &Path) -> Vec<Finding> {
    let cites = symbol_citations(root);
    if cites.is_empty() {
        return vec![Finding::blocked(
            "symbol citations",
            "found no `path::symbol` citations to check",
        )];
    }
    let findings: Vec<Finding> = cites
        .iter()
        .filter_map(|(where_, reference, symbol)| {
            let Ok(text) = std::fs::read_to_string(root.join(reference)) else {
                return Some(Finding::fail(
                    where_.clone(),
                    format!("cites {reference}::{symbol}, and {reference} is not in the tree"),
                ));
            };
            let last = symbol.rsplit("::").next().unwrap_or(symbol);
            let declared = if reference.ends_with(".sh") {
                declares_shell(&text, last)
            } else {
                declares(&text, last)
            };
            (!declared).then(|| {
                Finding::fail(
                    where_.clone(),
                    format!("cites {reference}::{symbol}, which {reference} does not declare"),
                )
            })
        })
        .collect();
    crate::reported("symbol citations", findings, || {
        format!("{} `path::symbol` citation(s) name a live symbol", cites.len())
    })
}

/// Check that every referenced path exists.
pub fn check(root: &Path) -> Vec<Finding> {
    let references = referenced(root);
    if references.is_empty() {
        return vec![Finding::blocked("referenced paths", "found no repo-relative paths to check")];
    }
    let findings: Vec<Finding> = references
        .iter()
        .filter(|(_, r)| !root.join(r.trim_end_matches('/')).exists())
        .map(|(where_, r)| {
            Finding::fail(where_.clone(), format!("names {r}, which is not in the tree"))
        })
        .collect();
    crate::reported("referenced paths", findings, || {
        format!("{} repo-relative reference(s) all exist", references.len())
    })
}
