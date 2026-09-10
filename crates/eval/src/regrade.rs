//! Offline regrading and the grader source snapshot carried by every new transcript.
//!
//! A content hash is an identifier, not provenance: when the Python grader was deleted, its hash
//! survived and its source did not. New evidence therefore carries the exact Rust sources that
//! produced its verdicts. Regrading preserves the recorded verdict and writes the current one
//! beside it; it never edits the transcript it reads.

use ironworks_graders::cases;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const SOURCES: &[(&str, &str)] = &[
    ("crates/graders/src/lib.rs", include_str!("../../graders/src/lib.rs")),
    ("crates/graders/src/cases.rs", include_str!("../../graders/src/cases.rs")),
    ("crates/graders/src/cases/tests.rs", include_str!("../../graders/src/cases/tests.rs")),
    ("crates/graders/src/discipline.rs", include_str!("../../graders/src/discipline.rs")),
    (
        "crates/graders/src/discipline/tests.rs",
        include_str!("../../graders/src/discipline/tests.rs"),
    ),
    ("crates/graders/src/text.rs", include_str!("../../graders/src/text.rs")),
];

/// The self-contained identity of the grader linked into this binary.
///
/// The digest makes comparisons cheap; `sources` makes the digest independently inspectable even
/// after the working tree moves or a file is deleted.
pub fn current_provenance() -> Value {
    provenance_of(SOURCES)
}

/// The manifest for a given set of sources.
///
/// Parameterised so the digest's one load-bearing property — that it *moves* when a source moves —
/// can be measured against two inputs. It used to be inlined over `SOURCES`, and the test named
/// for that property hashed the literal `b"different"` and asserted it differed from the manifest:
/// true for any two unequal strings, and never once a function of the sources.
fn provenance_of(sources: &[(&str, &str)]) -> Value {
    let mut hasher = Sha256::new();
    let sources: Vec<Value> = sources
        .iter()
        .map(|(path, source)| {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            hasher.update(source.as_bytes());
            hasher.update([0]);
            json!({
                "path": path,
                "sha256": sha256(source.as_bytes()),
                "source": source,
            })
        })
        .collect();
    json!({
        "implementation": "ironworks-eval",
        "sha256": hex(&hasher.finalize()),
        "sources": sources,
    })
}

/// Grade stored answers with the linked Rust grader, preserving what the earlier grader recorded.
pub fn run(
    root: &Path,
    transcripts: &[PathBuf],
    legacy_grader: Option<&str>,
    output: &Path,
) -> Result<std::process::ExitCode, String> {
    let mut rows = Vec::new();
    let mut inputs = Vec::new();
    let mut changed = 0usize;
    let mut recorded_grader: Option<Value> = None;
    let mut complete_provenance = true;

    for requested in transcripts {
        let path = if requested.is_absolute() { requested.clone() } else { root.join(requested) };
        let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let doc: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("parsing {}: {e}", path.display()))?;
        let cases = doc
            .get("cases")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{} has no cases array", path.display()))?;
        let label = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
        let provenance = if let Some(grader) = doc.get("grader") {
            grader.clone()
        } else {
            complete_provenance = false;
            let id = legacy_grader.ok_or_else(|| {
                format!(
                    "{label}: no embedded grader; legacy transcripts require --recorded-grader <id>"
                )
            })?;
            json!({
                "id": id,
                "source": null,
                "provenance": "incomplete: the uncommitted Python source was deleted before it was copied; the id was recorded before and after all forty turns",
            })
        };
        if recorded_grader.as_ref().is_some_and(|seen| seen != &provenance) {
            return Err(
                "the input transcripts were graded by different instruments; regrade them separately"
                    .to_owned(),
            );
        }
        recorded_grader = Some(provenance);
        inputs.push(json!({
            "transcript": label,
            "sha256": sha256(&bytes),
            "answers": cases.len(),
        }));

        for row in cases {
            let field = |name: &str| {
                row.get(name)
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{label}: a row has no string {name}"))
            };
            let case_id = field("case")?;
            let answer = field("answer")?;
            let was = field("verdict")?;
            let was_why = field("why")?;
            let run = row.get("run").and_then(Value::as_u64).unwrap_or(1);
            let case = cases::case(case_id)
                .ok_or_else(|| format!("{label}: no current grader for case {case_id:?}"))?;
            let now = case.grade(answer);
            let verdict = now.verdict.as_str();
            if verdict != was {
                changed += 1;
            }
            rows.push(json!({
                "transcript": label,
                "run": run,
                "case": case_id,
                "recorded": {"verdict": was, "why": was_why},
                "regraded": {"verdict": verdict, "why": now.why},
                "changed": verdict != was,
            }));
        }
    }

    let current = current_provenance();
    let report = json!({
        "kind": "grader-regrade",
        "recorded_grader": recorded_grader,
        "current_grader": current,
        "inputs": inputs,
        "summary": {
            "answers": rows.len(),
            "changed": changed,
            "unchanged": rows.len() - changed,
        },
        "cases": rows,
    });
    std::fs::write(output, serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?)
        .map_err(|e| format!("writing {}: {e}", output.display()))?;

    println!("regrade: {} stored answer(s), {changed} verdict change(s)", rows.len());
    println!(
        "regrade: recorded grader {}",
        report["recorded_grader"]["sha256"]
            .as_str()
            .or_else(|| report["recorded_grader"]["id"].as_str())
            .unwrap_or("unreported")
    );
    println!(
        "regrade: current grader {} with exact source embedded",
        current["sha256"].as_str().unwrap_or("unreported")
    );
    println!("regrade: wrote {}", output.display());
    if complete_provenance {
        Ok(std::process::ExitCode::SUCCESS)
    } else {
        eprintln!(
            "regrade: BLOCKED provenance for the recorded grader — its uncommitted source was deleted; only its pre-recorded id survives"
        );
        Ok(std::process::ExitCode::from(3))
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_carries_the_source_not_only_its_hash() {
        let p = current_provenance();
        assert_eq!(p["sha256"].as_str().unwrap().len(), 64);
        let sources = p["sources"].as_array().unwrap();
        assert!(sources.iter().all(|s| !s["source"].as_str().unwrap().is_empty()));
        // `sources.len() == SOURCES.len()` was the whole check here, and it is true by
        // construction: the array is built by mapping over that constant.
        assert_eq!(sources.len(), SOURCES.len());
    }

    /// `SOURCES` must name every file the grader is built from.
    ///
    /// This is the property the manifest exists for, and nothing asserted it: a new grader module
    /// would be linked into the binary, absent from the manifest, and the digest would go on
    /// reporting the same identity for a grader that had changed. Read from the tree rather than
    /// from the constant, because a constant cannot notice its own omission.
    #[test]
    fn every_grader_source_is_listed_in_the_manifest() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../graders/src");
        let mut found: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("graders/src is readable") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path.strip_prefix(&root).expect("under root");
                    found.push(format!("crates/graders/src/{}", rel.display()));
                }
            }
        }
        found.sort();
        let mut listed: Vec<String> = SOURCES.iter().map(|(p, _)| (*p).to_owned()).collect();
        listed.sort();
        assert_eq!(found, listed, "the grader manifest and crates/graders/src disagree");
    }

    #[test]
    fn changing_a_source_changes_the_manifest_digest() {
        // Two source sets differing in exactly one byte of one body, and then in exactly one
        // character of one path. The old test hashed the literal `b"different"` and asserted it
        // differed from the real manifest — true for any two unequal strings, and never a
        // statement about the sources at all.
        let a: &[(&str, &str)] = &[("a.rs", "fn one() {}"), ("b.rs", "fn two() {}")];
        let body: &[(&str, &str)] = &[("a.rs", "fn one() {}"), ("b.rs", "fn TWO() {}")];
        let path: &[(&str, &str)] = &[("a.rs", "fn one() {}"), ("c.rs", "fn two() {}")];

        let d = |s: &[(&str, &str)]| provenance_of(s)["sha256"].as_str().unwrap().to_owned();
        assert_ne!(d(a), d(body), "a changed source body left the digest alone");
        assert_ne!(d(a), d(path), "a changed source path left the digest alone");
        // Positive control: the digest is a function of the sources, so the same input twice is
        // the same digest. Without this the two assertions above would pass for a random value.
        assert_eq!(d(a), d(a), "the digest is not stable for one input");
    }

    #[test]
    fn a_regrade_preserves_the_recorded_verdict_beside_the_current_one() {
        let dir = std::env::temp_dir().join(format!("iw-regrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("input.json"),
            r#"{"cases":[{"run":1,"case":"attention","verdict":"FAIL","why":"old rule","answer":"Juniper needs its sample export and technical owner. Northline has contradictory renewal records."}]}"#,
        )
        .unwrap();
        let output = dir.join("output.json");

        let code = run(&dir, &[PathBuf::from("input.json")], Some("sha256:old"), &output).unwrap();
        assert_eq!(code, std::process::ExitCode::from(3));
        let report: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(report["cases"][0]["recorded"]["verdict"], "FAIL");
        assert_eq!(report["cases"][0]["regraded"]["verdict"], "PASS");
        assert_eq!(report["summary"]["changed"], 1);
        assert!(report["current_grader"]["sources"].as_array().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_embedded_recorded_grader_needs_no_legacy_override() {
        let dir =
            std::env::temp_dir().join(format!("iw-regrade-provenance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("input.json"),
            r#"{"grader":{"sha256":"exact","sources":[{"source":"grader"}]},"cases":[{"run":1,"case":"attention","verdict":"PASS","why":"old rule","answer":"Juniper needs its sample export and technical owner. Northline has contradictory renewal records."}]}"#,
        )
        .unwrap();
        let output = dir.join("output.json");

        let code = run(&dir, &[PathBuf::from("input.json")], None, &output).unwrap();
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        let report: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(report["recorded_grader"]["sha256"], "exact");
        assert_eq!(report["recorded_grader"]["sources"][0]["source"], "grader");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
