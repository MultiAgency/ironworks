//! A recorded run, refused unless it binds to the synthetic book.
//!
//! # Why this refuses rather than warns
//!
//! **The published page renders answer text.** Every answer on it was produced over
//! `fixtures/account-coordination/book` — invented accounts, every domain RFC 2606 reserved, every
//! record marked synthetic. Nothing enforced that: the generator read whatever JSON sat in
//! `docs/experiments/` and published its answers. The protection was the convention that a private
//! dogfood transcript is never retained — which is a practice, not a control, and a practice is one
//! careless `cp` away from publishing a real counterparty's records to the internet.
//!
//! So the binding is **proved**: the transcript records the fingerprint of the book it ran against,
//! that fingerprint is recomputed from the fixtures on disk, and the two must match. A transcript
//! naming no book, or a different one, is refused — and refused *closed*, because the failure mode
//! of guessing is unrecoverable.

use std::path::Path;

/// The five runs recorded before the driver wrote a book fingerprint.
///
/// Each is committed to this repository and is provably a run of the synthetic suite; none can be
/// bound mechanically, because **no fingerprint is invented after the fact**.
///
/// **This list is closed.** It names history, not a category. A transcript written from now on
/// records its book or it is not published — so the way to add to this set is to not need to.
const PRE_FINGERPRINT: &[&str] = &[
    "0003-account-coordination-poc.json",
    "0004-account-coordination-poc.json",
    "0005-account-coordination-poc.json",
    "0006-account-coordination-poc.json",
    "0007-account-coordination-poc.json",
];

/// A loaded run: its case rows, and what it was composed from.
#[derive(Debug)]
pub struct Transcript {
    pub cases: Vec<serde_json::Value>,
    pub ingredients: Option<serde_json::Value>,
}

/// Load a transcript, proving it was run over the book on disk.
pub fn load(experiments: &Path, book: &Path, name: &str) -> Result<Transcript, String> {
    let path = experiments.join(name);
    if !path.is_file() {
        return Err(format!("no transcript at {}", path.display()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;

    let (cases, recorded, ingredients) = match &doc {
        serde_json::Value::Array(rows) => (rows.clone(), None, None),
        serde_json::Value::Object(map) => (
            map.get("cases")
                .and_then(|c| c.as_array())
                .cloned()
                .ok_or_else(|| format!("{name} has no `cases`"))?,
            map.get("book").cloned(),
            map.get("ingredients").cloned(),
        ),
        _ => return Err(format!("{name} is neither a list nor an object")),
    };

    let recorded_fingerprint = recorded
        .as_ref()
        .and_then(|b| b.get("fingerprint"))
        .and_then(|f| f.as_str())
        .filter(|f| !f.is_empty());

    let Some(recorded_fingerprint) = recorded_fingerprint else {
        if PRE_FINGERPRINT.contains(&name) {
            return Ok(Transcript { cases, ingredients });
        }
        return Err(format!(
            "{name} records no book fingerprint and is not one of the five historical runs; \
             refusing to publish answers that cannot be tied to the synthetic fixtures"
        ));
    };

    let ours = ironworks_evidence::fingerprint::book(book)
        .ok_or_else(|| format!("no book at {} to bind {name} against", book.display()))?;

    // Compare the fingerprint alone. A transcript also records the book's `path`, and a recorded
    // object is a superset of a recomputed one — comparing them whole refuses every transcript,
    // which is a broken control rather than a strict one.
    if recorded_fingerprint != ours.fingerprint {
        return Err(format!(
            "{name} was run against a different book than {} — refusing to publish. If that book \
             is a tenant's, it must never reach this page.",
            book.display()
        ));
    }
    Ok(Transcript { cases, ingredients })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("iw-site-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (exp, book) = (base.join("experiments"), base.join("book"));
        std::fs::create_dir_all(&exp).unwrap();
        std::fs::create_dir_all(&book).unwrap();
        std::fs::write(book.join("a.json"), b"{\"account\":{}}").unwrap();
        (exp, book)
    }

    #[test]
    fn a_transcript_bound_to_the_book_loads() {
        let (exp, book) = scratch("bound");
        let fp = ironworks_evidence::fingerprint::book(&book).unwrap().fingerprint;
        std::fs::write(
            exp.join("0099-x.json"),
            format!(
                r#"{{"book":{{"fingerprint":"{fp}"}},"cases":[{{"case":"a","verdict":"PASS"}}]}}"#
            ),
        )
        .unwrap();
        assert!(load(&exp, &book, "0099-x.json").is_ok());
    }

    #[test]
    fn a_transcript_run_over_another_book_is_refused() {
        // The control that matters: if that other book is a tenant's, it must never be published.
        let (exp, book) = scratch("otherbook");
        std::fs::write(
            exp.join("0099-x.json"),
            r#"{"book":{"fingerprint":"deadbeefdeadbeef"},"cases":[{"case":"a","verdict":"PASS"}]}"#,
        )
        .unwrap();
        let err = load(&exp, &book, "0099-x.json").unwrap_err();
        assert!(err.contains("different book"), "{err}");
    }

    #[test]
    fn a_transcript_naming_no_book_is_refused_unless_it_is_historical() {
        let (exp, book) = scratch("nobook");
        std::fs::write(exp.join("0099-x.json"), r#"{"cases":[{"case":"a","verdict":"PASS"}]}"#)
            .unwrap();
        assert!(
            load(&exp, &book, "0099-x.json").unwrap_err().contains("records no book fingerprint")
        );

        // The five pre-fingerprint runs are the closed exception.
        std::fs::write(
            exp.join("0004-account-coordination-poc.json"),
            r#"[{"case":"contradiction","verdict":"FAIL"}]"#,
        )
        .unwrap();
        assert!(load(&exp, &book, "0004-account-coordination-poc.json").is_ok());
    }

    #[test]
    fn the_historical_list_is_closed() {
        // It names history, not a category. A new transcript records its book or is not published.
        assert_eq!(PRE_FINGERPRINT.len(), 5);
        assert!(PRE_FINGERPRINT.iter().all(|n| n.starts_with("000")));
    }
}
