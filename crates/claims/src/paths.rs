//! A document that names a file which no longer exists is a document nobody can follow.

use crate::{Finding, backticked, doc_files};
use std::path::Path;

/// Prefixes this repository unambiguously owns.
///
/// `deploy/` and `site/` are deliberately absent: the Python reference has its own `deploy/lib`,
/// and `site/` is generated and gitignored, so asserting either exists would fire on something
/// correct.
const OWNED: &[&str] = &["dev/", "docs/", "fixtures/", "services/", "scrap/"];

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
    OWNED.iter().any(|p| reference.starts_with(p))
        || crates.iter().any(|p| reference.starts_with(p.as_str()))
}

fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
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
    for md in doc_files(root) {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let where_ = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned();
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
    matches!(ext, "md" | "json" | "log")
        && stem.len() > 5
        && stem.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && stem.as_bytes()[4] == b'-'
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
    for md in files {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let where_ = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned();
        for name in crate::backticked(&text, path_char) {
            if is_experiment_file(&name) {
                out.push((where_.clone(), name));
            }
        }
        // `](0001-alloy-drift.md)` — only meaningful beside the writeups it links to.
        if md.parent().is_some_and(|d| d.ends_with("experiments")) {
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
pub fn check_experiment_numbers(root: &Path) -> Vec<Finding> {
    let dir = root.join("docs/experiments");
    let mut by_number: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for path in crate::markdown_in(&dir) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        // `README.md` and anything else unnumbered is not an experiment.
        if name.len() < 5 || !name.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
            continue;
        }
        by_number.entry(name[..4].to_owned()).or_default().push(name);
    }
    if by_number.is_empty() {
        return vec![Finding::blocked(
            "experiment numbers",
            "found no numbered write-ups in docs/experiments",
        )];
    }
    let mut findings: Vec<Finding> = by_number
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
    if findings.is_empty() {
        findings.push(Finding::pass(
            "experiment numbers",
            format!("{} numbered write-up(s), no number claimed twice", by_number.len()),
        ));
    }
    findings
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
    let mut findings: Vec<Finding> = cited
        .iter()
        .filter(|(_, name)| !dir.join(name).is_file())
        .map(|(where_, name)| {
            Finding::fail(where_.clone(), format!("cites {name}, which is not in docs/experiments"))
        })
        .collect();
    if findings.is_empty() {
        findings.push(Finding::pass(
            "cited experiments",
            format!("{} experiment(s) cited by name all exist", cited.len()),
        ));
    }
    findings
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
    for md in doc_files(root) {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let where_ = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned();
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
    fn walk(dir: &Path, out: &mut std::collections::BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // Build output and generated pages are not the repository's own material.
                if !matches!(name.as_str(), "target" | ".git" | "site" | ".wrangler") {
                    walk(&path, out);
                }
            } else if name.ends_with(".md") {
                out.insert(name);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(root, &mut out);
    out
}

/// Check that every markdown file cited by bare name is still somewhere in the tree.
pub fn check_bare_docs(root: &Path) -> Vec<Finding> {
    let cited = bare_docs(root);
    if cited.is_empty() {
        return vec![Finding::blocked("named documents", "found no markdown cited by bare name")];
    }
    let present = markdown_basenames(root);
    let mut findings: Vec<Finding> = cited
        .iter()
        .filter(|(_, name)| !present.contains(name.as_str()))
        .map(|(where_, name)| {
            Finding::fail(where_.clone(), format!("names {name}, which is not in the tree"))
        })
        .collect();
    if findings.is_empty() {
        findings.push(Finding::pass(
            "named documents",
            format!("{} document(s) cited by bare name all exist", cited.len()),
        ));
    }
    findings
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
pub fn anchored(root: &Path) -> Vec<(String, String, usize)> {
    let crates = owned_crates(root);
    let mut out = Vec::new();
    for md in doc_files(root) {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let where_ = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned();
        for span in crate::backticked(&text, |c| path_char(c) || matches!(c, ':' | ',' | '-')) {
            let Some((reference, spec)) = span.split_once(':') else { continue };
            if !is_ours(reference, &crates) {
                continue;
            }
            // `203,221` and `29-34` both name lines; the highest is the one that has to exist.
            let highest = spec
                .split([',', '-'])
                .filter(|p| !p.is_empty())
                .map(str::parse::<usize>)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .and_then(|v| v.into_iter().max());
            if let Some(line) = highest {
                out.push((where_.clone(), reference.to_owned(), line));
            }
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
        .map(|(where_, reference, line)| {
            Finding::fail(
                where_.clone(),
                format!(
                    "cites {reference}:{line} — a line anchor drifts silently; \
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
    for md in doc_files(root) {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let where_ = md.strip_prefix(root).unwrap_or(&md).to_string_lossy().into_owned();
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
    text.lines().any(|line| {
        let mut token = line.trim();
        for modifier in ["pub(crate) ", "pub(super) ", "pub ", "async ", "unsafe ", "default "] {
            token = token.strip_prefix(modifier).unwrap_or(token);
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
    let mut findings: Vec<Finding> = cites
        .iter()
        .filter_map(|(where_, reference, symbol)| {
            let Ok(text) = std::fs::read_to_string(root.join(reference)) else {
                return Some(Finding::fail(
                    where_.clone(),
                    format!("cites {reference}::{symbol}, and {reference} is not in the tree"),
                ));
            };
            let last = symbol.rsplit("::").next().unwrap_or(symbol);
            (!declares(&text, last)).then(|| {
                Finding::fail(
                    where_.clone(),
                    format!("cites {reference}::{symbol}, which {reference} does not declare"),
                )
            })
        })
        .collect();
    if findings.is_empty() {
        findings.push(Finding::pass(
            "symbol citations",
            format!("{} `path::symbol` citation(s) name a live symbol", cites.len()),
        ));
    }
    findings
}

/// Check that every referenced path exists.
pub fn check(root: &Path) -> Vec<Finding> {
    let references = referenced(root);
    if references.is_empty() {
        return vec![Finding::blocked("referenced paths", "found no repo-relative paths to check")];
    }
    let mut findings: Vec<Finding> = references
        .iter()
        .filter(|(_, r)| !root.join(r.trim_end_matches('/')).exists())
        .map(|(where_, r)| {
            Finding::fail(where_.clone(), format!("names {r}, which is not in the tree"))
        })
        .collect();
    if findings.is_empty() {
        findings.push(Finding::pass(
            "referenced paths",
            format!("{} repo-relative reference(s) all exist", references.len()),
        ));
    }
    findings
}
