//! The composition arc: what moved under a digest, and which runs graded it.
//!
//! An answer-quality verdict is bound to **one** alloy digest. When the digest moves, the next
//! question is always what moved under it — and an alloy fuses scrap *and* a tenant's guidance, so
//! it can move with no repository diff at all. This reads the ingredients out of the working tree
//! and out of history, so the arc can say which part changed rather than only that something did.

use ironworks_domain::composition::digest16;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// A suite's transcript patterns and how its alloy is composed.
pub struct Suite {
    pub name: &'static str,
    /// **Wider than the suite name on purpose.** Focused stability probes predate the suite name
    /// in their filenames, so the set also selects the case names they were recorded under. They
    /// are separated by *shape* afterwards, never by filename — see [`transcripts`].
    pub patterns: &'static [&'static str],
    pub slug: &'static str,
    pub service: Option<&'static str>,
    pub guidance: &'static str,
}

pub const SUITES: &[Suite] = &[
    Suite {
        name: "account-coordination",
        patterns: &["account-coordination", "attention", "injection"],
        slug: "coordination-eval",
        service: Some("relationship-intelligence"),
        guidance: "fixtures/account-coordination/guidance.md",
    },
    Suite {
        name: "account-analysis",
        patterns: &["eval-parity"],
        slug: "eval",
        service: None,
        guidance: "fixtures/eval.guidance.md",
    },
];

pub fn suite(name: &str) -> Option<&'static Suite> {
    SUITES.iter().find(|s| s.name == name)
}

/// Mirrors `core::composition::strip_frontmatter`, **including what it refuses to strip**.
///
/// Conservative by construction: an unterminated block is returned untouched, because a malformed
/// header is worth seeing in the prompt where it is obvious.
pub fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else { return text };
    let Some(end) = rest.find("\n---\n") else { return text };
    rest[end + "\n---\n".len()..].trim_start_matches('\n')
}

/// Reads a repo-relative file, from the working tree or from a git revision.
pub enum Reader {
    Worktree(std::path::PathBuf),
    Revision { root: std::path::PathBuf, rev: String },
}

impl Reader {
    pub fn read(&self, rel: &str) -> Option<String> {
        match self {
            Self::Worktree(root) => std::fs::read_to_string(root.join(rel)).ok(),
            Self::Revision { root, rev } => {
                let out = Command::new("git")
                    .args(["show", &format!("{rev}:{rel}")])
                    .current_dir(root)
                    .output()
                    .ok()?;
                out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
            }
        }
    }
}

/// Part digests in composition order, safety tail last — mirroring `Composition::part_digests`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingredients {
    pub parts: Vec<(String, String)>,
    pub missing: Vec<String>,
}

/// Read a service's ingredients through `read`.
///
/// `None` when the tree does not hold the service definition — the honest answer for a revision
/// predating it, rather than an empty result that reads like "nothing changed".
pub fn ingredients(read: &Reader, service: &str) -> Option<Ingredients> {
    let raw = read.read(&format!("services/{service}.json"))?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let composition = doc.get("composition")?;
    let mut parts = Vec::new();
    let mut missing = Vec::new();
    for rel in composition.get("parts")?.as_array()? {
        let rel = rel.as_str()?;
        match read.read(&format!("scrap/{rel}")) {
            Some(body) => {
                parts.push((rel.to_owned(), digest16(strip_frontmatter(&body).as_bytes())))
            }
            None => missing.push(rel.to_owned()),
        }
    }
    let tail = composition.get("safety_tail")?.as_str()?;
    match read.read(&format!("scrap/{tail}")) {
        // `melt` does not strip the tail's frontmatter, so neither does this.
        Some(body) => parts.push((tail.to_owned(), digest16(body.as_bytes()))),
        None => missing.push(tail.to_owned()),
    }
    Some(Ingredients { parts, missing })
}

/// What the binary would compose right now — a dry melt, no turn, no model.
#[derive(Debug, Default, Clone)]
pub struct Composed {
    pub service: String,
    pub alloy: String,
    pub alloy_bytes: u64,
    pub guidance: String,
    pub guidance_bytes: u64,
    pub parts: Vec<(String, String)>,
}

pub fn compose(root: &Path, suite: &Suite) -> Result<Composed, String> {
    let binary = root.join("target/debug/ironworks");
    if !binary.is_file() {
        return Err(format!("no binary at {}; cargo build first", binary.display()));
    }
    let mut cmd = Command::new(&binary);
    cmd.current_dir(root).args(["compose", "--slug", suite.slug, "--guidance", suite.guidance]);
    if let Some(service) = suite.service {
        cmd.args(["--service", service]);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "ironworks compose failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut c = Composed::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["service", name, ..] => c.service = (*name).to_owned(),
            ["alloy", bytes, _, digest, ..] => {
                c.alloy_bytes = bytes.parse().unwrap_or(0);
                c.alloy = digest.trim_matches(['(', ')']).to_owned();
            }
            ["guidance", bytes, _, digest, ..] => {
                c.guidance_bytes = bytes.parse().unwrap_or(0);
                c.guidance = digest.trim_matches(['(', ')']).to_owned();
            }
            ["part" | "tail", name, digest, ..] => {
                c.parts.push(((*name).to_owned(), (*digest).to_owned()));
            }
            _ => {}
        }
    }
    (!c.alloy.is_empty()).then_some(c).ok_or_else(|| "compose reported no alloy digest".to_owned())
}

/// One recorded run of a suite: a column of the matrix.
#[derive(Debug, Clone)]
pub struct Run {
    pub label: String,
    /// **A set, not one digest.** A run that graded two compositions describes neither — the
    /// driver's own rule — and carrying the set forward keeps that visible instead of picking one.
    pub alloy: Vec<String>,
    /// What stands now, after the regrade ledger.
    ///
    /// **Ordered as the transcript recorded them, not sorted.** The matrix reads down the cases,
    /// and a suite has a running order that means something — sorting them alphabetically would
    /// reorder the page every time a case is renamed.
    pub verdicts: Vec<(String, String)>,
    /// The ledger entries that moved a stored verdict, keyed by case.
    pub regraded: BTreeMap<String, ironworks_evidence::regrades::Regrade>,
}

impl Run {
    /// The standing verdict for a case, if this run graded it.
    pub fn verdict(&self, case: &str) -> Option<&str> {
        self.verdicts.iter().find(|(c, _)| c == case).map(|(_, v)| v.as_str())
    }
}

/// One case graded many times at a fixed composition: a measurement of variance, not a run.
#[derive(Debug, Clone)]
pub struct Probe {
    pub label: String,
    pub case: String,
    pub alloy: Vec<String>,
    pub verdicts: Vec<String>,
}

/// Every recorded run for a suite, oldest first, with any regrade applied.
///
/// # Why probes are separated by shape rather than by name
///
/// The matrix is cases down and runs across, so a column is only meaningful if it graded the
/// suite. A one-case transcript would add a column with one filled cell and six blanks, and its
/// tally would read as `0 P` for a suite of seven — a worse omission than leaving it out. So a
/// transcript with **one distinct case over more than one row** is a probe, and nothing is
/// excluded by filename.
pub fn transcripts(experiments: &Path, suite: &Suite) -> Result<(Vec<Run>, Vec<Probe>), String> {
    let ledger = ironworks_evidence::regrades::load(experiments);
    if !ledger.problems.is_empty() {
        return Err(format!(
            "the regrade ledger does not match the transcripts it names:\n         {}",
            ledger.problems.join("\n         ")
        ));
    }

    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(experiments)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            name.ends_with(".json") && suite.patterns.iter().any(|pat| name.contains(pat))
        })
        .collect();
    paths.sort();

    let (mut runs, mut probes) = (Vec::new(), Vec::new());
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();

        // Transcripts written before `compose` existed are a bare list; newer ones carry the
        // ingredients beside them. Both are read, because the old ones are the evidence.
        let rows = match &doc {
            serde_json::Value::Array(rows) => rows.clone(),
            serde_json::Value::Object(map) => {
                map.get("cases").and_then(|c| c.as_array()).cloned().unwrap_or_default()
            }
            _ => continue,
        };
        let cases: BTreeSet<String> = rows.iter().filter_map(case_of).collect();

        if cases.len() == 1 && rows.len() > 1 {
            let mut ordered = rows.clone();
            ordered.sort_by_key(|r| r.get("run").and_then(serde_json::Value::as_u64).unwrap_or(0));
            probes.push(Probe {
                label: name,
                case: cases.iter().next().cloned().unwrap_or_default(),
                alloy: digests(&ordered),
                verdicts: ordered.iter().filter_map(verdict_of).collect(),
            });
            continue;
        }

        let mut by_run: BTreeMap<u64, Vec<serde_json::Value>> = BTreeMap::new();
        for row in rows {
            let n = row.get("run").and_then(serde_json::Value::as_u64).unwrap_or(1);
            by_run.entry(n).or_default().push(row);
        }
        let many = by_run.len() > 1;
        for (n, group) in by_run {
            let stored: Vec<(String, String)> =
                group.iter().filter_map(|r| Some((case_of(r)?, verdict_of(r)?))).collect();
            let regraded: BTreeMap<String, ironworks_evidence::regrades::Regrade> = stored
                .iter()
                .filter_map(|(case, _)| {
                    ledger
                        .entries
                        .get(&(name.clone(), case.clone()))
                        .map(|g| (case.clone(), g.clone()))
                })
                .collect();
            let verdicts: Vec<(String, String)> = stored
                .iter()
                .map(|(case, v)| {
                    let standing = regraded.get(case).map_or(v.clone(), |g| g.verdict.clone());
                    (case.clone(), standing)
                })
                .collect();
            runs.push(Run {
                label: if many { format!("{name} run {n}") } else { name.clone() },
                alloy: digests(&group),
                verdicts,
                regraded,
            });
        }
    }
    Ok((runs, probes))
}

fn case_of(row: &serde_json::Value) -> Option<String> {
    row.get("case").and_then(|c| c.as_str()).map(str::to_owned)
}

fn verdict_of(row: &serde_json::Value) -> Option<String> {
    row.get("verdict").and_then(|v| v.as_str()).map(str::to_owned)
}

fn digests(rows: &[serde_json::Value]) -> Vec<String> {
    let set: BTreeSet<String> = rows
        .iter()
        .filter_map(|r| r.pointer("/composition/digest").and_then(|d| d.as_str()))
        .map(str::to_owned)
        .collect();
    set.into_iter().collect()
}

/// The latest recorded run that graded **exactly** this composition, or `None`.
///
/// The "now" column is not a summary of the newest run — it is *this composition's* row, and the
/// only runs entitled to fill it are the ones that composed it. Taking the last run instead was
/// wrong in the one case the column exists for: a composition that moves away and comes back
/// reads as graded, then shows the verdicts of whatever ran in between — verdicts describing bytes
/// this binary would no longer send.
///
/// **Latest**, because a later run of the same composition supersedes an earlier one.
/// **Exactly**, because a run whose digest moved mid-run describes neither composition.
pub fn matching_run<'a>(runs: &'a [Run], alloy: &str) -> Option<&'a Run> {
    runs.iter().rev().find(|r| r.alloy.len() == 1 && r.alloy[0] == alloy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_is_stripped_only_when_it_is_well_formed() {
        assert_eq!(strip_frontmatter("---\ntitle: x\n---\nbody"), "body");
        // An unterminated block is left alone: a malformed header is worth seeing in the prompt.
        assert_eq!(strip_frontmatter("---\ntitle: x\nbody"), "---\ntitle: x\nbody");
        assert_eq!(strip_frontmatter("no frontmatter"), "no frontmatter");
    }

    fn run(label: &str, alloy: &[&str]) -> Run {
        Run {
            label: label.to_owned(),
            alloy: alloy.iter().map(|s| (*s).to_owned()).collect(),
            verdicts: Vec::new(),
            regraded: BTreeMap::new(),
        }
    }

    #[test]
    fn the_now_column_takes_the_latest_run_of_this_composition() {
        let runs = [run("a", &["aaa"]), run("b", &["bbb"]), run("c", &["aaa"])];
        assert_eq!(matching_run(&runs, "aaa").unwrap().label, "c", "latest, not first");
        assert!(matching_run(&runs, "zzz").is_none());
    }

    #[test]
    fn a_composition_that_moved_away_and_came_back_is_not_filled_by_what_ran_between() {
        // The defect this rule exists for: `bbb`'s verdicts describe bytes the binary would no
        // longer send, and must never fill `aaa`'s column.
        let runs = [run("old", &["aaa"]), run("between", &["bbb"])];
        assert_eq!(matching_run(&runs, "aaa").unwrap().label, "old");
    }

    #[test]
    fn a_run_that_graded_two_compositions_speaks_for_neither() {
        let runs = [run("mixed", &["aaa", "bbb"])];
        assert!(matching_run(&runs, "aaa").is_none(), "a mid-run digest move describes neither");
        assert!(matching_run(&runs, "bbb").is_none());
    }

    #[test]
    fn the_real_suite_separates_probes_from_runs_by_shape() {
        let root = ironworks_evidence::root();
        let (runs, probes) =
            transcripts(&root.join("docs/experiments"), suite("account-coordination").unwrap())
                .expect("the ledger validates");
        assert!(!runs.is_empty(), "found no runs");
        assert!(!probes.is_empty(), "found no stability probes");
        // A probe grades one case many times; a run grades the suite.
        assert!(probes.iter().all(|p| p.verdicts.len() > 1));
        assert!(runs.iter().all(|r| !r.verdicts.is_empty()));
    }

    #[test]
    fn ingredients_are_none_for_a_tree_without_the_definition() {
        let reader = Reader::Worktree(std::env::temp_dir().join("iw-no-such-tree"));
        assert!(ingredients(&reader, "relationship-intelligence").is_none());
    }

    #[test]
    fn the_working_tree_composes_the_service_it_ships() {
        let reader = Reader::Worktree(ironworks_evidence::root());
        let got =
            ingredients(&reader, "relationship-intelligence").expect("the definition is here");
        assert!(got.missing.is_empty(), "missing parts: {:?}", got.missing);
        assert!(got.parts.len() >= 2, "a composition is parts plus a safety tail");
        // The tail is last, always.
        assert!(got.parts.last().unwrap().0.contains("safety"), "{:?}", got.parts.last());
    }
}
