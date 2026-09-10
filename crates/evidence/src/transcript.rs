//! The one shape question every transcript reader asks.
//!
//! # Written five times, answered four ways
//!
//! A transcript is either a bare array of case rows — the shape written before `compose` existed —
//! or an object carrying `cases` beside the book fingerprint and the ingredients. Both are read,
//! because the old ones are the evidence.
//!
//! That decode existed in `crates/site/src/read/mod.rs`, `crates/site/src/arc/data.rs`,
//! `crates/site/src/transcript.rs`, `crates/evidence/src/regrades.rs` and
//! `crates/claims/src/tallies.rs`, and the copies disagreed about an object with **no** `cases`
//! key: two returned zero cases, three refused. Zero cases is the empty-store shape §K-8 warns
//! about — a transcript that contributed nothing while reporting success.
//!
//! # `crates/claims` keeps its own, deliberately
//!
//! The control must not share a reader with what it checks. `crates/claims/src/tallies.rs` compares
//! an experiment's written verdict counts against the transcript it names; if it decoded through
//! this function and this function were wrong, both sides would agree and the check would go green
//! over the defect. That is the same reasoning as §K-8's positive control, applied to the harness
//! itself, and the duplication is the price of it — stated there, in that file.

use serde_json::Value;

/// A transcript that cannot be read as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    /// Neither an array of rows nor an object.
    NotATranscript,
    /// An object, but nothing under `cases`.
    NoCases,
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotATranscript => "is neither a list of cases nor an object carrying `cases`",
            Self::NoCases => "is an object with no `cases`",
        })
    }
}

/// The case rows a transcript records, whichever shape it is written in.
///
/// Borrowed rather than cloned: every caller of the five copies this replaces cloned the vector
/// and then immediately filtered it.
pub fn cases(doc: &Value) -> Result<&[Value], Malformed> {
    match doc {
        Value::Array(rows) => Ok(rows),
        Value::Object(map) => {
            map.get("cases").and_then(Value::as_array).map(Vec::as_slice).ok_or(Malformed::NoCases)
        }
        _ => Err(Malformed::NotATranscript),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_shapes_read_and_the_two_that_are_not_transcripts_are_refused() {
        let bare = serde_json::json!([{ "case": "a", "verdict": "PASS" }]);
        assert_eq!(cases(&bare).expect("a bare array is the old shape").len(), 1);

        let carried = serde_json::json!({
            "book": { "fingerprint": "x" },
            "cases": [{ "case": "a", "verdict": "PASS" }, { "case": "b", "verdict": "FAIL" }],
        });
        assert_eq!(cases(&carried).expect("an object carrying cases").len(), 2);

        // The disagreement this function exists to end: two of the five copies read this as zero
        // cases, which is a transcript reporting success while contributing nothing (§K-8).
        let ledger = serde_json::json!({ "regrades": [] });
        assert_eq!(cases(&ledger).unwrap_err(), Malformed::NoCases);

        assert_eq!(cases(&serde_json::json!("nope")).unwrap_err(), Malformed::NotATranscript);
        assert_eq!(cases(&serde_json::json!(7)).unwrap_err(), Malformed::NotATranscript);
    }

    #[test]
    fn an_empty_cases_list_is_a_transcript_with_no_cases_not_a_malformed_one() {
        // A suite that ran nothing is a real, readable transcript. Conflating it with a file that
        // is not a transcript would lose the distinction the verdict counts rest on.
        let empty = serde_json::json!({ "cases": [] });
        assert_eq!(cases(&empty).expect("an empty run is still a transcript").len(), 0);
    }
}
