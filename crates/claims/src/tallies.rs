//! An experiment's written tally, checked against the transcript it names.
//!
//! `0003` said 4 PASS where its transcript held 3. The writeup is prose about a recorded run, and
//! prose drifts; the transcript does not, because **a transcript is evidence and is never
//! rewritten**. So the transcript is the authority and the sentence is what gets corrected.

use crate::Finding;
use std::collections::BTreeMap;
use std::path::Path;

pub const VERDICTS: &[&str] = &["PASS", "FAIL", "REVIEW", "BLOCKED"];

/// The verdict tally a transcript actually holds.
///
/// **Two shapes exist and both are current**: the early runs are a bare list of case rows, the
/// later ones a map carrying `suite` and `ingredients` alongside `cases`. Accept either rather
/// than normalising the archive.
pub fn transcript_counts(path: &Path) -> Result<BTreeMap<String, usize>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let rows = match &doc {
        serde_json::Value::Array(rows) => rows.clone(),
        serde_json::Value::Object(map) => map
            .get("cases")
            .and_then(|c| c.as_array())
            .cloned()
            .ok_or_else(|| "no `cases` in the transcript".to_owned())?,
        _ => return Err("the transcript is neither a list nor an object".to_owned()),
    };
    let mut counts = BTreeMap::new();
    for row in rows {
        if let Some(v) = row.get("verdict").and_then(|v| v.as_str()) {
            *counts.entry(v.to_owned()).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

/// A `- Recorded:` line, the transcript its section names, and the tally it claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub line: usize,
    pub transcript: Option<String>,
    pub tally: BTreeMap<String, usize>,
}

/// Every `- Recorded: …` line in a writeup, paired with the transcript named above it.
///
/// **Only the first tally on the line is read.** A line may also carry a regrade — *"**Regraded 6
/// PASS, 1 FAIL**"* — which is a later judgement under corrected graders and is deliberately not
/// what the transcript holds. Comparing it would report a disagreement that is the whole point of
/// the ledger.
pub fn recorded_claims(markdown: &str) -> Vec<Claim> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for (n, line) in markdown.lines().enumerate() {
        if let Some(name) = transcript_name(line) {
            current = Some(name);
        }
        if line.trim_start().starts_with("- Recorded:") {
            let head = line.split("**Regraded").next().unwrap_or(line);
            out.push(Claim { line: n + 1, transcript: current.clone(), tally: tally(head) });
        }
    }
    out
}

/// A backticked `NNNN-name.json`, if the line names one.
fn transcript_name(line: &str) -> Option<String> {
    crate::backticked(line, |c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        .into_iter()
        .find(|s| {
            s.ends_with(".json")
                && s.len() > 5
                && s.as_bytes()[..4].iter().all(u8::is_ascii_digit)
                && s.as_bytes().get(4) == Some(&b'-')
        })
}

/// Every `<n> <VERDICT>` pair in a line.
fn tally(text: &str) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    for pair in words.windows(2) {
        let count = pair[0].trim_matches(|c: char| !c.is_ascii_digit());
        let verdict = pair[1].trim_matches(|c: char| !c.is_ascii_alphabetic());
        if count.is_empty() {
            continue;
        }
        if let (Ok(n), Some(v)) = (count.parse::<usize>(), VERDICTS.iter().find(|v| **v == verdict))
        {
            out.insert((*v).to_owned(), n);
        }
    }
    out
}

/// Check every experiment writeup's tally against its transcript.
pub fn check(root: &Path) -> Vec<Finding> {
    let dir = root.join("docs/experiments");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![Finding::blocked("experiments", "no docs/experiments directory")];
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();

    let mut findings = Vec::new();
    for md in files {
        let Ok(text) = std::fs::read_to_string(&md) else { continue };
        let name = md.file_name().unwrap_or_default().to_string_lossy().into_owned();
        for claim in recorded_claims(&text) {
            let where_ = format!("{name}:{}", claim.line);
            let Some(transcript) = claim.transcript else {
                findings
                    .push(Finding::blocked(where_, "a Recorded line names no transcript above it"));
                continue;
            };
            let path = dir.join(&transcript);
            if !path.is_file() {
                findings.push(Finding::blocked(
                    where_,
                    format!("names {transcript}, which is not in the tree"),
                ));
                continue;
            }
            match transcript_counts(&path) {
                Err(why) => findings.push(Finding::blocked(where_, format!("{transcript}: {why}"))),
                Ok(actual) => {
                    let disagree: Vec<String> = VERDICTS
                        .iter()
                        .filter(|v| {
                            claim.tally.get(**v).copied().unwrap_or(0)
                                != actual.get(**v).copied().unwrap_or(0)
                        })
                        .map(|v| {
                            format!(
                                "{v}: says {}, transcript has {}",
                                claim.tally.get(*v).copied().unwrap_or(0),
                                actual.get(*v).copied().unwrap_or(0)
                            )
                        })
                        .collect();
                    if disagree.is_empty() {
                        findings.push(Finding::pass(where_, format!("{transcript} tallies")));
                    } else {
                        findings.push(Finding::fail(
                            where_,
                            format!("{transcript} — {}", disagree.join("; ")),
                        ));
                    }
                }
            }
        }
    }
    // A reader that reads nothing passes everything. The directory can exist, and every writeup in
    // it can be pruned or renamed, and every loop above still completes without a finding — which
    // reports as a clean run over zero claims. Every sibling check guards this; this one did not,
    // and the guard became load-bearing when the tallies were consolidated out of twenty-one
    // writeups into one section of one file.
    if findings.is_empty() {
        return vec![Finding::blocked(
            "experiments",
            "no `- Recorded:` line in any writeup; nothing was checked",
        )];
    }
    findings
}
