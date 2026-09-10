//! What a sentence stands on.
//!
//! # This is not the defect detector
//!
//! The overclaim that prompted this tool — *"Offboarding appears complete on the access side"* —
//! sits inside a block naming `LH-004` and classifies as [`Kind::Inherited`]. **No syntactic check
//! finds it**; a person reading it beside the two activities it draws on does. The classes order
//! the page; the evidence column does the work.
//!
//! # Why scope matters
//!
//! Without it, two thirds of every answer is flagged, and a marker that fires on everything says
//! nothing. These answers name the account in a heading and date the evidence in the sentence
//! beneath, so a sentence continuing a block is *in scope*, not context-free.

use super::book::{Book, dates_in, record_ids_in};

/// One line of an answer, with what its shape says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub heading: bool,
    pub bullet: bool,
    pub parts: Vec<String>,
}

/// Blocks in order; each block is its lines, each line its sentences.
///
/// The markdown structure is kept because a bullet that names nothing reads differently from a
/// heading that names nothing, and flattening the two would report a heading as an unanchored
/// claim.
pub fn sentences(answer: &str) -> Vec<Vec<Line>> {
    let mut out = Vec::new();
    for block in answer.split("\n\n") {
        let mut lines = Vec::new();
        for raw in block.lines() {
            // A horizontal rule is punctuation, not a claim. Counting one as a sentence that names
            // nothing is exactly the false positive that makes a marker worth ignoring.
            if raw.trim().is_empty() || is_rule(raw) {
                continue;
            }
            let trimmed = raw.trim();
            let heading = trimmed.starts_with('#') && trimmed.contains(' ')
                || (trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4);
            let bullet = is_bullet(raw);
            // The marker becomes the list item, so it must not also survive as text.
            let mut text = if bullet {
                trimmed.trim_start_matches(['-', '*', '+']).trim_start().to_owned()
            } else {
                trimmed.to_owned()
            };
            while text.starts_with('#') {
                text = text.trim_start_matches('#').trim_start().to_owned();
            }
            let parts = split_sentences(&text);
            lines.push(Line { heading, bullet, parts });
        }
        if !lines.is_empty() {
            out.push(lines);
        }
    }
    out
}

fn is_bullet(line: &str) -> bool {
    let t = line.trim_start();
    t.strip_prefix(['-', '*', '+']).is_some_and(|r| r.starts_with(char::is_whitespace))
}

/// `---`, `***`, `___` — three or more of one mark, alone on a line.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    let Some(first) = t.chars().next() else { return false };
    matches!(first, '-' | '*' | '_') && t.len() >= 3 && t.chars().all(|c| c == first)
}

/// Split on `(?<=[.!?])\s+(?=[A-Z\*\d])` — after terminal punctuation, before a new claim.
///
/// The lookahead is what keeps `Ltd. of` one sentence: a lowercase continuation is an abbreviation,
/// not a new assertion.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '.' | '!' | '?') {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let broke = j > i + 1
                && chars
                    .get(j)
                    .is_some_and(|c| c.is_uppercase() || *c == '*' || c.is_ascii_digit());
            if broke {
                let piece: String = chars[start..i + 1].iter().collect();
                if !piece.trim().is_empty() {
                    parts.push(piece.trim().to_owned());
                }
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.trim().is_empty() {
        parts.push(tail.trim().to_owned());
    }
    parts
}

/// One activity a sentence reaches, and whether it was reached by name or by date alone.
#[derive(Debug, Clone)]
pub struct Hit {
    pub record: usize,
    pub activity: usize,
    /// False when the only thing tying this activity to the sentence is the date.
    pub pinned: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Anchors {
    pub records: Vec<usize>,
    pub dates: Vec<String>,
    pub activities: Vec<Hit>,
}

/// Every reference in one sentence that resolves to something in the book.
///
/// A date attaches to the records the sentence names; failing that, to the records its block or
/// its question named (`scope`); failing that, to any record holding an activity on that date.
/// **Only the last is guesswork**, and only it is reported as *matched on date alone* — because
/// these answers name the account in a heading and then date the evidence beneath, so scoring
/// every one of those as a loose match would call the normal shape of a good answer ambiguous.
pub fn anchors_in(text: &str, book: &Book, hint: Option<&str>, scope: &[usize]) -> Anchors {
    let mut named: Vec<usize> = Vec::new();
    for id in record_ids_in(text) {
        if let Some(i) = book.iter().position(|r| r.id == id)
            && !named.contains(&i)
        {
            named.push(i);
        }
    }
    for (i, rec) in book.iter().enumerate() {
        if !rec.name.is_empty() && text.contains(&rec.name) && !named.contains(&i) {
            named.push(i);
        }
    }

    let dates = dates_in(text, hint);
    let within: Vec<usize> = if !named.is_empty() { named.clone() } else { scope.to_vec() };
    let pinned = !within.is_empty();
    let search: Vec<usize> = if pinned { within } else { (0..book.len()).collect() };

    let mut activities = Vec::new();
    for date in &dates {
        for &r in &search {
            for &a in book[r].by_date.get(date).map(Vec::as_slice).unwrap_or(&[]) {
                activities.push(Hit { record: r, activity: a, pinned });
            }
        }
    }
    Anchors { records: named, dates, activities }
}

/// What a sentence is, for the reader's eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Heading,
    /// Resolves to a dated activity.
    Anchored,
    /// Points at a record, but no date the book holds.
    Named,
    /// Names nothing itself, but continues a block that named a record.
    Inherited,
    /// An assertion standing on nothing at all.
    Bare,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Anchored => "anchored",
            Self::Named => "named",
            Self::Inherited => "inherited",
            Self::Bare => "bare",
        }
    }
}

pub fn classify(anchors: &Anchors, heading: bool, in_scope: bool) -> Kind {
    if heading {
        Kind::Heading
    } else if !anchors.activities.is_empty() {
        Kind::Anchored
    } else if !anchors.records.is_empty() || !anchors.dates.is_empty() {
        Kind::Named
    } else if in_scope {
        Kind::Inherited
    } else {
        Kind::Bare
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Book {
        super::super::book::load(&crate::test_root().join("fixtures/account-coordination/book"))
            .unwrap()
    }

    #[test]
    fn a_horizontal_rule_is_punctuation_not_a_claim() {
        // Counting one as a sentence that names nothing is the false positive that makes the
        // marker worth ignoring.
        assert!(sentences("---").is_empty());
        assert!(sentences("***").is_empty());
        assert_eq!(sentences("real text").len(), 1);
    }

    #[test]
    fn a_bullet_marker_becomes_the_list_item_and_does_not_survive_as_text() {
        let got = sentences("- one thing");
        assert!(got[0][0].bullet);
        assert_eq!(got[0][0].parts, ["one thing"]);
    }

    #[test]
    fn a_bold_line_and_a_hash_line_are_both_headings() {
        assert!(sentences("**Northline**")[0][0].heading);
        assert!(sentences("## Northline")[0][0].heading);
        assert!(!sentences("Northline renewed.")[0][0].heading);
    }

    #[test]
    fn a_lowercase_continuation_does_not_start_a_new_sentence() {
        // The lookahead in the original: `Ltd. of` is one sentence, not two.
        assert_eq!(split_sentences("Acme Ltd. of Ohio filed."), ["Acme Ltd. of Ohio filed."]);
        assert_eq!(split_sentences("One. Two."), ["One.", "Two."]);
        assert_eq!(split_sentences("One. **Two**."), ["One.", "**Two**."]);
    }

    #[test]
    fn a_sentence_naming_a_record_and_a_date_anchors_to_that_activity() {
        let book = book();
        let a = anchors_in("NC-003 on 2026-08-21 said so", &book, Some("2026"), &[]);
        assert!(!a.records.is_empty(), "the record is named");
        assert!(a.activities.iter().all(|h| h.pinned), "named, so not a loose date match");
        assert_eq!(classify(&a, false, true), Kind::Anchored);
    }

    #[test]
    fn a_date_alone_is_matched_loosely_and_says_so() {
        let book = book();
        let a = anchors_in("on 2026-08-21 something happened", &book, Some("2026"), &[]);
        if !a.activities.is_empty() {
            assert!(a.activities.iter().all(|h| !h.pinned), "no record named, so it is guesswork");
        }
    }

    #[test]
    fn scope_carries_a_block_that_named_a_record() {
        let book = book();
        let scope = vec![book.iter().position(|r| r.id == "NC-003").unwrap()];
        let a = anchors_in("It remains unresolved.", &book, Some("2026"), &scope);
        // Names nothing itself, but continues a block that named one.
        assert_eq!(classify(&a, false, true), Kind::Inherited);
        // Without scope it stands on nothing.
        assert_eq!(classify(&a, false, false), Kind::Bare);
    }

    #[test]
    fn a_heading_is_never_reported_as_an_unanchored_claim() {
        let a = Anchors::default();
        assert_eq!(classify(&a, true, false), Kind::Heading);
    }
}
