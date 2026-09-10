//! Prove the fixture book on disk **is** the book these turns were shown, or refuse to render.
//!
//! # The failure this exists to stop was live
//!
//! Reading `0005-…json` — answers built over **five** records — against a fixture directory that
//! had since grown to six, and reporting the reading as being against six. Every sentence would
//! then have been resolvable against a record the model was never shown, and the page's whole
//! claim is that it shows what the turn saw.
//!
//! # Only a fingerprint binds
//!
//! The driver records a digest over the fixture file digests, taken at the time of the run.
//! Recomputing it here proves the same files, byte for byte. It carries no record content, so a
//! transcript can hold it without holding a tenant's book.
//!
//! **A matching record count is not a binding and is not called one.** It is the only book fact
//! the pre-fingerprint transcripts hold, and it proves nothing that matters: two books of six
//! records can share no bytes at all. An earlier revision rendered on a matching count and named
//! the weakness on the page — a caveat where a refusal belongs, since a reader who trusts the page
//! has already been shown the wrong evidence by the time they reach it.
//!
//! So a transcript without a recorded fingerprint **cannot be read here**, and that is the honest
//! outcome rather than a gap to be patched: nothing in this repository knows what the book held on
//! an earlier afternoon, and computing today's fingerprint and filing it under that run would
//! manufacture exactly the provenance this function exists to establish.

use super::book::Book;
use ironworks_page::esc;
use std::collections::BTreeSet;
use std::path::Path;

/// How the binding was established, for the terminal and for the page.
#[derive(Debug)]
pub struct Binding {
    pub terminal: String,
    pub html: String,
}

pub fn verify(
    doc: &serde_json::Value,
    rows: &[serde_json::Value],
    book: &Book,
    book_dir: &Path,
    root: &Path,
    name: &str,
) -> Result<Binding, String> {
    let rel = book_dir.strip_prefix(root).unwrap_or(book_dir).to_string_lossy().into_owned();

    // Diagnostics first, so a refusal can say what the turns reported even when nothing binds.
    let mut counts: BTreeSet<String> = BTreeSet::new();
    let mut states: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        // Recorded as a string by some drivers and as a number by others. `Value::to_string`
        // would re-encode a string *with its quotes*, so `"10"` would never parse back to 10 and
        // a transcript that agrees with its book would be reported as contradicting itself.
        if let Some(c) = row.pointer("/composition/records") {
            let text = c.as_str().map(str::to_owned).or_else(|| c.as_u64().map(|n| n.to_string()));
            if let Some(text) = text {
                counts.insert(text);
            }
        }
        if let Some(s) = row.pointer("/composition/state").and_then(|s| s.as_str()) {
            states.insert(s.to_owned());
        }
    }
    let told = format!(
        "{} record(s) [{}]",
        if counts.is_empty() {
            "?".to_owned()
        } else {
            counts.iter().cloned().collect::<Vec<_>>().join("/")
        },
        if states.is_empty() {
            "unstated".to_owned()
        } else {
            states.iter().cloned().collect::<Vec<_>>().join("/")
        }
    );

    let recorded = doc.get("book").filter(|b| !b.is_null());
    let fingerprint = recorded
        .and_then(|b| b.get("fingerprint"))
        .and_then(|f| f.as_str())
        .filter(|f| !f.is_empty());
    let Some(fingerprint) = fingerprint else {
        return Err(format!(
            "{name} records no book fingerprint, so nothing here can prove {rel} still holds the \
             records these turns were shown. Its telemetry reports {told}, which is a diagnostic \
             and not a binding: two books of the same size need share no bytes. Nothing may be \
             rendered against it, and no fingerprint may be invented for it after the fact — \
             record a new run, which carries one."
        ));
    };
    if let Some(path) = recorded.and_then(|b| b.get("path")).and_then(|p| p.as_str())
        && path != rel
    {
        return Err(format!("{name} was recorded against {path}, not {rel}"));
    }
    let now = ironworks_evidence::fingerprint::book(book_dir)
        .map_err(|e| format!("cannot read the book at {}: {e}", book_dir.display()))?
        .ok_or_else(|| format!("no book at {}", book_dir.display()))?;
    if now.fingerprint != fingerprint {
        return Err(format!(
            "{rel} is not the book {name} was recorded against — it fingerprints {}, the \
             transcript recorded {fingerprint}. The answers were built over records this directory \
             no longer holds, and showing them against these would attribute evidence the turns \
             never received.",
            now.fingerprint
        ));
    }

    // The fingerprint binds; the telemetry must then agree with it, or the transcript contradicts
    // itself about its own book and neither half can be trusted.
    if states.len() != 1 || !states.contains("complete") {
        return Err(format!(
            "{name} fingerprints this book but reports it as {told}; only a complete book can be \
             read against"
        ));
    }
    if counts.len() != 1
        || counts.iter().next().and_then(|c| c.parse::<usize>().ok()) != Some(book.len())
    {
        return Err(format!(
            "{name} fingerprints this book and reports {told}, but {rel} holds {} records — the \
             transcript contradicts itself",
            book.len()
        ));
    }

    Ok(Binding {
        terminal: format!("fingerprint {} ({told} reported)", now.fingerprint),
        html: format!(
            "verified by fingerprint <span class=\"mono\">{}</span> recorded with the run; the \
             turns reported {}",
            esc(&now.fingerprint),
            esc(&told)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book_dir() -> std::path::PathBuf {
        crate::test_root().join("fixtures/account-coordination/book")
    }

    fn rows(records: usize, state: &str) -> Vec<serde_json::Value> {
        vec![serde_json::json!({"composition": {"records": records, "state": state}})]
    }

    #[test]
    fn a_record_count_recorded_as_a_string_still_parses() {
        // Both shapes are in the archive. Re-encoding a string value would keep its quotes, and a
        // transcript agreeing with its book would read as contradicting itself.
        let book = super::super::book::load(&book_dir()).unwrap();
        let fp = ironworks_evidence::fingerprint::book(book_dir()).unwrap().unwrap().fingerprint;
        let doc = serde_json::json!({"book": {"fingerprint": fp}});
        let as_string =
            vec![serde_json::json!({"composition": {"records": "10", "state": "complete"}})];
        assert!(
            verify(&doc, &as_string, &book, &book_dir(), &crate::test_root(), "x.json").is_ok(),
            "a string count must bind exactly as a numeric one does"
        );
    }

    #[test]
    fn a_transcript_with_no_fingerprint_cannot_be_read_at_all() {
        // And no fingerprint may be invented for it after the fact.
        let book = super::super::book::load(&book_dir()).unwrap();
        let doc = serde_json::json!({"cases": []});
        let err =
            verify(&doc, &rows(10, "complete"), &book, &book_dir(), &crate::test_root(), "x.json")
                .unwrap_err();
        assert!(err.contains("records no book fingerprint"), "{err}");
        assert!(
            err.contains("diagnostic and not a binding"),
            "the count must not read as a binding"
        );
    }

    #[test]
    fn a_fingerprint_from_another_book_refuses() {
        let book = super::super::book::load(&book_dir()).unwrap();
        let doc = serde_json::json!({"book": {"fingerprint": "deadbeefdeadbeef"}});
        let err =
            verify(&doc, &rows(10, "complete"), &book, &book_dir(), &crate::test_root(), "x.json")
                .unwrap_err();
        assert!(err.contains("is not the book"), "{err}");
    }

    #[test]
    fn a_bound_book_whose_telemetry_disagrees_is_a_self_contradiction() {
        let book = super::super::book::load(&book_dir()).unwrap();
        let fp = ironworks_evidence::fingerprint::book(book_dir()).unwrap().unwrap().fingerprint;
        let doc = serde_json::json!({"book": {"fingerprint": fp}});
        // Right fingerprint, wrong count.
        let err =
            verify(&doc, &rows(6, "complete"), &book, &book_dir(), &crate::test_root(), "x.json")
                .unwrap_err();
        assert!(err.contains("contradicts itself"), "{err}");
    }

    #[test]
    fn a_degraded_supply_cannot_be_read_against() {
        let book = super::super::book::load(&book_dir()).unwrap();
        let fp = ironworks_evidence::fingerprint::book(book_dir()).unwrap().unwrap().fingerprint;
        let doc = serde_json::json!({"book": {"fingerprint": fp}});
        let err =
            verify(&doc, &rows(10, "partial"), &book, &book_dir(), &crate::test_root(), "x.json")
                .unwrap_err();
        assert!(err.contains("only a complete book"), "{err}");
    }

    #[test]
    fn a_bound_and_consistent_transcript_verifies() {
        let book = super::super::book::load(&book_dir()).unwrap();
        let fp = ironworks_evidence::fingerprint::book(book_dir()).unwrap().unwrap().fingerprint;
        let doc = serde_json::json!({"book": {"fingerprint": fp.clone()}});
        let got =
            verify(&doc, &rows(10, "complete"), &book, &book_dir(), &crate::test_root(), "x.json")
                .expect("binds");
        assert!(got.terminal.contains(&fp));
    }
}
