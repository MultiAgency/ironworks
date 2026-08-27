//! Whether this turn sends the book, when the conversation may already have it.
//!
//! A chained turn is answered by a runtime that replays the whole thread, so records supplied to
//! an earlier turn are still in front of the model. Sending them again is not wrong, it is just
//! paid for twice — measured on a real two-turn chain, the second turn carried another 14,514
//! bytes of the same forty records.
//!
//! # The two absences are different facts
//!
//! **Never supplied** and **supplied, and the book was empty** are not the same, and neither is
//! **supplied, and we cannot currently tell whether it moved**. Collapsing them gives a thread
//! that either re-sends everything forever or answers from a book it was never given. So the
//! decision is a table over two independent options — what this thread was last shown, and what
//! the store says now — and every cell is named.
//!
//! # What it never does
//!
//! It never decides to send *nothing* to a fresh turn. A turn with no thread behind it has no
//! conversation to have carried anything, so `Full` is the only answer there regardless of what
//! any stored mark says — a stale mark from a thread that has since been broken must not silence
//! the records of the thread replacing it.

/// Fingerprint the book exactly as the model would see it.
///
/// **Over the rendered records, not over a list of ids.** A catalog version would notice an
/// account appearing or disappearing and miss an activity body being edited — and an edited body
/// is precisely the change a coordination service must not answer around, since the whole product
/// is reading dated evidence. Rendering is what the model is shown, so digesting it means "moved"
/// has the same meaning here as it does to the reader.
pub fn fingerprint(
    records: &[crate::record::AccountRecord],
    fact_fields: Option<&[String]>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for record in records {
        hasher.update(crate::envelope::render_account_for_digest(record, fact_fields).as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// What a thread was last shown.
///
/// `catalog` is a fingerprint of the book as supplied — present even when the book was empty,
/// because "shown an empty book" is a fact worth holding. Its absence means never shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppliedMark {
    pub catalog: String,
    pub records: u32,
}

/// Why the book is being sent in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullBecause {
    /// No thread to have carried it. Every fresh turn sends the book.
    NotChained,
    /// A chained thread that has never been shown the book.
    NeverSupplied,
    /// The book moved since this thread was shown it.
    CatalogMoved { was: String, now: String },
    /// The store could not say what the book is now, so "unchanged" cannot be established.
    /// Sending it again is the direction that cannot answer from a stale book.
    CatalogUnknown,
}

impl std::fmt::Display for FullBecause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotChained => f.write_str("fresh turn"),
            Self::NeverSupplied => f.write_str("this thread has not been shown the book"),
            Self::CatalogMoved { was, now } => write!(f, "book moved {was} -> {now}"),
            Self::CatalogUnknown => {
                f.write_str("the store could not say whether the book has moved")
            }
        }
    }
}

/// What this turn does about the records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Supply {
    Full(FullBecause),
    /// The conversation already carries them, unchanged. `records` travels so the model can be
    /// told how many it is expected to still have.
    CarriedByThread {
        records: u32,
    },
}

impl Supply {
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full(_))
    }
}

/// The table. Two options in, one decision out, every cell reachable.
///
/// `chained` is separate from `supplied` on purpose: a mark can outlive the thread that earned
/// it — a composition change breaks the chain but the row is still there — and reading the mark
/// without asking whether this turn is actually continuing that thread is how a fresh turn ends
/// up silently starved of records.
pub fn decide(chained: bool, supplied: Option<&SuppliedMark>, catalog: Option<&str>) -> Supply {
    if !chained {
        return Supply::Full(FullBecause::NotChained);
    }
    match (supplied, catalog) {
        (None, _) => Supply::Full(FullBecause::NeverSupplied),
        (Some(_), None) => Supply::Full(FullBecause::CatalogUnknown),
        (Some(mark), Some(now)) if mark.catalog == now => {
            Supply::CarriedByThread { records: mark.records }
        }
        (Some(mark), Some(now)) => Supply::Full(FullBecause::CatalogMoved {
            was: mark.catalog.clone(),
            now: now.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(catalog: &str, records: u32) -> SuppliedMark {
        SuppliedMark { catalog: catalog.to_owned(), records }
    }

    /// The exhaustive `(supplied, catalog)` table gap 3 asks for, both `None` cases included.
    #[test]
    fn the_supply_table() {
        let shown = mark("cat-1", 40);

        // Not chained: nothing carries anything, whatever any stored mark says.
        for supplied in [None, Some(&shown)] {
            for catalog in [None, Some("cat-1")] {
                assert_eq!(
                    decide(false, supplied, catalog),
                    Supply::Full(FullBecause::NotChained),
                    "a fresh turn always sends the book (supplied={supplied:?} catalog={catalog:?})"
                );
            }
        }

        // Chained, the four cells.
        assert_eq!(
            decide(true, None, None),
            Supply::Full(FullBecause::NeverSupplied),
            "never shown, and the store cannot say what the book is: still send what we have"
        );
        assert_eq!(
            decide(true, None, Some("cat-1")),
            Supply::Full(FullBecause::NeverSupplied),
            "never shown: the thread has nothing to carry"
        );
        assert_eq!(
            decide(true, Some(&shown), None),
            Supply::Full(FullBecause::CatalogUnknown),
            "shown before, but unchanged cannot be established — never assume it held"
        );
        assert_eq!(
            decide(true, Some(&shown), Some("cat-1")),
            Supply::CarriedByThread { records: 40 },
            "shown, and the book has not moved"
        );
        assert_eq!(
            decide(true, Some(&shown), Some("cat-2")),
            Supply::Full(FullBecause::CatalogMoved { was: "cat-1".into(), now: "cat-2".into() }),
            "the book moved under the thread"
        );
    }

    #[test]
    fn a_thread_shown_an_empty_book_is_not_a_thread_never_shown_one() {
        // The distinction `ever_supplied` exists for. An empty book still has a fingerprint, so
        // "shown, and there was nothing" is a mark like any other — and re-sending nothing to
        // prove nothing is exactly the waste this avoids.
        let empty = mark("cat-empty", 0);
        assert_eq!(
            decide(true, Some(&empty), Some("cat-empty")),
            Supply::CarriedByThread { records: 0 }
        );
        assert_eq!(decide(true, None, Some("cat-empty")), Supply::Full(FullBecause::NeverSupplied));
    }

    #[test]
    fn a_stale_mark_cannot_starve_the_thread_that_replaced_it() {
        // A composition change breaks the chain and the mark outlives it. Reading the mark
        // without asking whether this turn is continuing that thread would hand the new thread an
        // envelope with no records and a note saying it already has them.
        let shown = mark("cat-1", 40);
        assert!(decide(false, Some(&shown), Some("cat-1")).is_full());
    }

    fn record(id: &str, body: &str) -> crate::record::AccountRecord {
        crate::record::AccountRecord::from_json(&format!(
            r#"{{"record_id":"{id}","account":{{"name":"Acme"}},
                "activities":[{{"activity_id":"{id}-A1","occurred_at":"2026-08-01",
                                "kind":"call","body":"{body}"}}]}}"#
        ))
        .expect("fixture parses")
    }

    #[test]
    fn the_fingerprint_moves_when_an_activity_body_is_edited() {
        // The reason this digests the rendering rather than a list of ids: an edited body changes
        // nothing about the catalog and everything about the answer, and a coordination service
        // that answered around it would be reading evidence that no longer exists.
        let before = [record("AC-001", "agreed to send the export")];
        let after = [record("AC-001", "agreed to send the export by Friday")];
        assert_ne!(fingerprint(&before, None), fingerprint(&after, None));
    }

    #[test]
    fn the_fingerprint_is_stable_and_notices_a_record_arriving() {
        let one = [record("AC-001", "a")];
        let two = [record("AC-001", "a"), record("AC-002", "b")];
        assert_eq!(fingerprint(&one, None), fingerprint(&one, None), "stable across calls");
        assert_ne!(fingerprint(&one, None), fingerprint(&two, None));
        // An empty book has a fingerprint of its own — which is what lets "shown an empty book"
        // be a mark rather than an absence.
        assert_ne!(fingerprint(&[], None), fingerprint(&one, None));
        assert_eq!(fingerprint(&[], None).len(), 16);
    }

    /// The fingerprint digests the rendering, and the declared policy is part of the rendering —
    /// so a book that has not moved under a policy that has must not read as unchanged. This is
    /// the same `render_account` the envelope uses, which is what keeps "the book has not moved"
    /// meaning what a reader of the answer would mean by it.
    #[test]
    fn the_fingerprint_follows_the_declared_fact_policy() {
        let book = [crate::record::AccountRecord::from_json(
            r#"{"record_id":"AC-001","account":{"name":"Acme",
                 "facts":{"alpha":"a","zebra":"z"}},"contacts":[],"activities":[]}"#,
        )
        .expect("fixture parses")];
        let one = vec!["zebra".to_owned(), "alpha".to_owned()];
        let other = vec!["alpha".to_owned(), "zebra".to_owned()];
        let gapped = vec!["work_order".to_owned()];

        assert_ne!(
            fingerprint(&book, Some(&one)),
            fingerprint(&book, Some(&other)),
            "reordering the declaration reorders the rendering"
        );
        assert_ne!(
            fingerprint(&book, None),
            fingerprint(&book, Some(&gapped)),
            "declaring an absent key adds a gap line the model is shown"
        );
        // The two spellings of "assert no gaps" are one instruction, so they are one fingerprint.
        assert_eq!(fingerprint(&book, None), fingerprint(&book, Some(&[])));
        assert_eq!(fingerprint(&book, Some(&one)), fingerprint(&book, Some(&one)));
    }

    #[test]
    fn every_reason_reads_as_a_sentence() {
        for r in [
            FullBecause::NotChained,
            FullBecause::NeverSupplied,
            FullBecause::CatalogMoved { was: "a".into(), now: "b".into() },
            FullBecause::CatalogUnknown,
        ] {
            assert!(r.to_string().len() > 8, "{r:?}");
        }
    }
}
