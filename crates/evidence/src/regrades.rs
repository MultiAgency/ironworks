//! Later verdicts for answers that were already recorded.
//!
//! **A transcript is never rewritten** — it holds what the grader produced at the time — so a
//! finding made afterwards has nowhere to live but prose, where nothing can read it. The ledger is
//! that prose as data.
//!
//! # Why it is validated on every read
//!
//! A ledger that can name a transcript which does not exist, a case nobody ran, or a `was` that
//! never stood, is a ledger that can **rewrite history by typo**. So every entry is checked
//! against the transcript it points at, and a caller with any problem must block: an unvalidated
//! ledger silently restates the past, which is worse than having none.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One later judgement about one case in one recorded run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regrade {
    pub transcript: String,
    pub case: String,
    /// The verdict the transcript stored.
    pub was: String,
    /// The verdict that now stands.
    pub verdict: String,
    pub why: String,
}

/// A validated ledger, keyed by `(transcript, case)`, and everything wrong with it.
///
/// Both are returned rather than one or the other, because a problem is not a reason to discard
/// the entries that are sound — it is a reason for the caller to refuse to publish.
#[derive(Debug, Default)]
pub struct Ledger {
    pub entries: BTreeMap<(String, String), Regrade>,
    pub problems: Vec<String>,
}

/// Load and validate `regrades.json` from an experiments directory.
///
/// A missing ledger is empty and sound — not every repository state has one.
impl Regrade {
    /// One entry, or the names of the fields it does not carry.
    ///
    /// # Why this is not five `unwrap`s after a presence check
    ///
    /// `load` read each field **twice** — once to build a `missing` list, then again with
    /// `.unwrap()` to build the value — so five unwraps were correct only because of a check ten
    /// lines above them. That coupling is exactly what a fallible constructor removes: the fields
    /// are read once, and a missing one is a value rather than a proof obligation.
    fn read(entry: &serde_json::Value) -> Result<Self, Vec<&'static str>> {
        let field = |k: &str| {
            entry.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
        };
        let (transcript, case, was, verdict, why) =
            (field("transcript"), field("case"), field("was"), field("verdict"), field("why"));
        let missing: Vec<&'static str> = [
            ("transcript", transcript.is_none()),
            ("case", case.is_none()),
            ("was", was.is_none()),
            ("verdict", verdict.is_none()),
            ("why", why.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, absent)| absent.then_some(name))
        .collect();
        if !missing.is_empty() {
            return Err(missing);
        }
        Ok(Self {
            transcript: transcript.expect("checked above"),
            case: case.expect("checked above"),
            was: was.expect("checked above"),
            verdict: verdict.expect("checked above"),
            why: why.expect("checked above"),
        })
    }
}

pub fn load(dir: &Path) -> Ledger {
    let path = dir.join("regrades.json");
    if !path.is_file() {
        return Ledger::default();
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ledger {
            entries: BTreeMap::new(),
            problems: vec!["regrades.json is unreadable".to_owned()],
        };
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ledger {
            entries: BTreeMap::new(),
            problems: vec!["regrades.json is not valid JSON".to_owned()],
        };
    };

    let mut ledger = Ledger::default();
    let mut stored: BTreeMap<String, Option<BTreeMap<String, BTreeSet<String>>>> = BTreeMap::new();
    let rows = doc.get("regrades").and_then(|r| r.as_array()).cloned().unwrap_or_default();

    for (n, entry) in rows.iter().enumerate() {
        let regrade = match Regrade::read(entry) {
            Ok(regrade) => regrade,
            Err(missing) => {
                ledger.problems.push(format!("entry {n} is missing {}", missing.join(", ")));
                continue;
            }
        };
        let key = (regrade.transcript.clone(), regrade.case.clone());
        if ledger.entries.contains_key(&key) {
            ledger
                .problems
                .push(format!("{} {}: regraded twice", regrade.transcript, regrade.case));
            continue;
        }
        let by_case = stored
            .entry(regrade.transcript.clone())
            .or_insert_with(|| verdicts_by_case(&dir.join(&regrade.transcript)));
        let Some(by_case) = by_case else {
            ledger.problems.push(format!("{}: no such transcript", regrade.transcript));
            continue;
        };
        let Some(recorded) = by_case.get(&regrade.case) else {
            ledger
                .problems
                .push(format!("{}: no case {:?} in it", regrade.transcript, regrade.case));
            continue;
        };
        if recorded.len() > 1 {
            // A stability probe records one case many times. An entry keyed on the case alone
            // would restate every one of those runs from a reading of one of them.
            let seen: Vec<&str> = recorded.iter().map(String::as_str).collect();
            ledger.problems.push(format!(
                "{} {}: recorded {:?} across runs, so one entry cannot say which verdict it regrades",
                regrade.transcript, regrade.case, seen
            ));
            continue;
        }
        if !recorded.contains(&regrade.was) {
            let seen: Vec<&str> = recorded.iter().map(String::as_str).collect();
            ledger.problems.push(format!(
                "{} {}: ledger says it was {}, the transcript stored {}",
                regrade.transcript,
                regrade.case,
                regrade.was,
                seen.join("/")
            ));
            continue;
        }
        ledger.entries.insert(key, regrade);
    }
    ledger
}

/// The set of verdicts a transcript stored per case, or `None` when it does not exist.
///
/// **A set, not a verdict.** One transcript may record the same case many times.
fn verdicts_by_case(path: &Path) -> Option<BTreeMap<String, BTreeSet<String>>> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    let rows = crate::transcript::cases(&doc).ok()?;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        let (Some(case), Some(verdict)) =
            (row.get("case").and_then(|c| c.as_str()), row.get("verdict").and_then(|v| v.as_str()))
        else {
            continue;
        };
        out.entry(case.to_owned()).or_default().insert(verdict.to_owned());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("iw-regrades-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    const ONE_RUN: &str =
        r#"{"cases":[{"case":"a","verdict":"FAIL"},{"case":"b","verdict":"PASS"}]}"#;

    #[test]
    fn a_sound_entry_loads() {
        let dir = scratch("sound");
        write(&dir, "0001-x.json", ONE_RUN);
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[{"transcript":"0001-x.json","case":"a",
            "was":"FAIL","verdict":"PASS","why":"grader false negative"}]}"#,
        );
        let l = load(&dir);
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(l.entries.len(), 1);
    }

    #[test]
    fn an_entry_naming_a_verdict_that_never_stood_is_a_problem() {
        // This is the typo that would rewrite history: claiming a case was FAIL when it passed.
        let dir = scratch("wrongwas");
        write(&dir, "0001-x.json", ONE_RUN);
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[{"transcript":"0001-x.json","case":"b",
            "was":"FAIL","verdict":"PASS","why":"..."}]}"#,
        );
        let l = load(&dir);
        assert!(l.entries.is_empty());
        assert!(l.problems[0].contains("ledger says it was FAIL"), "{:?}", l.problems);
    }

    #[test]
    fn an_entry_naming_a_case_nobody_ran_is_a_problem() {
        let dir = scratch("nocase");
        write(&dir, "0001-x.json", ONE_RUN);
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[{"transcript":"0001-x.json","case":"zzz",
            "was":"FAIL","verdict":"PASS","why":"..."}]}"#,
        );
        assert!(load(&dir).problems[0].contains("no case"));
    }

    #[test]
    fn an_entry_naming_a_transcript_that_does_not_exist_is_a_problem() {
        let dir = scratch("notranscript");
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[{"transcript":"9999-nope.json","case":"a",
            "was":"FAIL","verdict":"PASS","why":"..."}]}"#,
        );
        assert!(load(&dir).problems[0].contains("no such transcript"));
    }

    #[test]
    fn a_case_recorded_many_times_cannot_be_regraded_by_one_entry() {
        // A stability probe runs one case repeatedly. One entry cannot say which run it means,
        // and accepting it would restate all of them from a reading of one.
        let dir = scratch("stability");
        write(
            &dir,
            "0010-s.json",
            r#"{"cases":[{"case":"a","verdict":"PASS"},
            {"case":"a","verdict":"FAIL"}]}"#,
        );
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[{"transcript":"0010-s.json","case":"a",
            "was":"FAIL","verdict":"PASS","why":"..."}]}"#,
        );
        assert!(load(&dir).problems[0].contains("across runs"), "{:?}", load(&dir).problems);
    }

    #[test]
    fn the_same_case_cannot_be_regraded_twice() {
        let dir = scratch("twice");
        write(&dir, "0001-x.json", ONE_RUN);
        write(
            &dir,
            "regrades.json",
            r#"{"regrades":[
            {"transcript":"0001-x.json","case":"a","was":"FAIL","verdict":"PASS","why":"one"},
            {"transcript":"0001-x.json","case":"a","was":"FAIL","verdict":"REVIEW","why":"two"}]}"#,
        );
        assert!(load(&dir).problems[0].contains("regraded twice"));
    }

    #[test]
    fn an_incomplete_entry_is_a_problem_rather_than_a_silent_skip() {
        let dir = scratch("incomplete");
        write(&dir, "0001-x.json", ONE_RUN);
        write(&dir, "regrades.json", r#"{"regrades":[{"transcript":"0001-x.json","case":"a"}]}"#);
        assert!(
            load(&dir).problems[0].contains("missing was, verdict, why"),
            "{:?}",
            load(&dir).problems
        );
    }

    #[test]
    fn a_missing_ledger_is_empty_and_sound() {
        let dir = scratch("none");
        let l = load(&dir);
        assert!(l.entries.is_empty() && l.problems.is_empty());
    }

    #[test]
    fn the_repositorys_own_ledger_validates() {
        // The ledger in the tree must be sound, or nothing may publish from it.
        let l = load(&crate::test_root().join("docs/experiments"));
        assert!(
            l.problems.is_empty(),
            "the regrade ledger has problems:\n  {}",
            l.problems.join("\n  ")
        );
        assert!(!l.entries.is_empty(), "found no entries — the reader is not reading");
    }
}
