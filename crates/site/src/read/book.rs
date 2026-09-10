//! The book a claim can be anchored *to*, and the date references that reach it.
//!
//! Loaded from the fixtures rather than from the transcript, deliberately: a transcript records
//! how many records a turn received, never their content, **and it is not going to start** — the
//! real book is a tenant's. Which is why nothing may render until [`super::verify`] has bound the
//! two: this reads the book as it is *now*, and the turn was shown the book as it was *then*.

use std::collections::BTreeMap;
use std::path::Path;

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub id: String,
    pub date: String,
    pub kind: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub activities: Vec<Activity>,
    pub by_date: BTreeMap<String, Vec<usize>>,
}

/// Every record in a book, keyed by record id, in file order.
pub type Book = Vec<Record>;

pub fn load(dir: &Path) -> Result<Book, String> {
    if !dir.is_dir() {
        return Err(format!("no book at {}", dir.display()));
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();

    let mut book = Book::new();
    for path in files {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let id = doc
            .get("record_id")
            .or_else(|| doc.get("account_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        let name = doc.pointer("/account/name").and_then(|v| v.as_str()).unwrap_or(&id).to_owned();
        let activities: Vec<Activity> = doc
            .get("activities")
            .and_then(|a| a.as_array())
            .map(|rows| {
                rows.iter()
                    .map(|a| Activity {
                        id: a
                            .get("activity_id")
                            .or_else(|| a.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        date: a
                            .get("occurred_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        kind: a.get("kind").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                        body: a.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut by_date: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, a) in activities.iter().enumerate() {
            by_date.entry(a.date.clone()).or_default().push(i);
        }
        book.push(Record { id, name, activities, by_date });
    }
    if book.is_empty() {
        return Err(format!("{} holds no records", dir.display()));
    }
    Ok(book)
}

/// The year the book lives in, so a bare `31 August` resolves. `None` if it spans years.
pub fn year_of(book: &Book) -> Option<String> {
    let years: std::collections::BTreeSet<&str> = book
        .iter()
        .flat_map(|r| &r.activities)
        // `get`, not `[..4]`. `len()` counts **bytes** and the slice cuts on them, so a date
        // beginning with a multibyte character panics — and `crates/graders/src/text.rs` and
        // `crates/devstore/src/main.rs::percent_decode` both carry a written lesson about exactly
        // this assumption. That the book is a trusted fixture today is a property of the data, not
        // of this function.
        .filter_map(|a| a.date.get(..4))
        .collect();
    (years.len() == 1).then(|| years.iter().next().unwrap().to_string())
}

/// One date reference, normalised. `hint` carries a bare `31 August` to a year.
pub fn iso(text: &str, hint: Option<&str>) -> Option<String> {
    if is_iso(text) {
        return Some(text.to_owned());
    }
    if let Some((month, day, year)) = long_date(text) {
        return Some(format!("{year}-{:02}-{day:02}", month + 1));
    }
    if let Some((day, month)) = day_month(text) {
        return hint.map(|y| format!("{y}-{:02}-{day:02}", month + 1));
    }
    None
}

fn is_iso(t: &str) -> bool {
    let b = t.as_bytes();
    t.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..].iter().all(u8::is_ascii_digit)
}

fn month_index(word: &str) -> Option<usize> {
    let w = word.to_lowercase();
    MONTHS.iter().position(|m| *m == w)
}

fn strip_ordinal(t: &str) -> &str {
    for suffix in ["st", "nd", "rd", "th"] {
        if let Some(head) = t.strip_suffix(suffix)
            && head.chars().all(|c| c.is_ascii_digit())
            && !head.is_empty()
        {
            return head;
        }
    }
    t
}

/// `August 18, 2026` → `(7, 18, "2026")`.
fn long_date(text: &str) -> Option<(usize, u32, String)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() != 3 {
        return None;
    }
    let month = month_index(words[0])?;
    let day: u32 = strip_ordinal(words[1].trim_end_matches(',')).parse().ok()?;
    let year = words[2];
    (year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()))
        .then(|| (month, day, year.to_owned()))
}

/// `31 August` → `(31, 7)`.
fn day_month(text: &str) -> Option<(u32, usize)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() != 2 {
        return None;
    }
    let day: u32 = strip_ordinal(words[0]).parse().ok()?;
    Some((day, month_index(words[1])?))
}

/// Every date reference in a span of text, in order of appearance, deduplicated.
///
/// Three forms, tried in the order the original tried them: ISO, `August 18, 2026`, `31 August`.
pub fn dates_in(text: &str, hint: Option<&str>) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut push = |d: String| {
        if !out.contains(&d) {
            out.push(d);
        }
    };
    // ISO first, over bare tokens.
    for w in &words {
        let bare = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if is_iso(bare) {
            push(bare.to_owned());
        }
    }
    // Then the two written forms, over windows of *cleaned* tokens. Punctuation has to come off
    // first: `(31 August)` is a date reference, and a tokenizer that kept the brackets would miss
    // it — the original matched on word boundaries, which see through them.
    let cleaned: Vec<&str> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    for w in cleaned.windows(3) {
        if let Some(d) = iso(&w.join(" "), hint) {
            push(d);
        }
    }
    for w in cleaned.windows(2) {
        if let Some(d) = iso(&w.join(" "), hint) {
            push(d);
        }
    }
    out
}

/// Every `AB-123` record id in a span of text, in order, deduplicated.
pub fn record_ids_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let bare = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let b = bare.as_bytes();
        let looks_like = b.len() == 6
            && b[0].is_ascii_uppercase()
            && b[1].is_ascii_uppercase()
            && b[2] == b'-'
            && b[3..].iter().all(u8::is_ascii_digit);
        if looks_like && !out.contains(&bare.to_owned()) {
            out.push(bare.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_date_forms_normalise_to_one() {
        assert_eq!(iso("2026-08-18", None).as_deref(), Some("2026-08-18"));
        assert_eq!(iso("August 18, 2026", None).as_deref(), Some("2026-08-18"));
        assert_eq!(iso("18th August", Some("2026")).as_deref(), Some("2026-08-18"));
    }

    #[test]
    fn a_bare_day_and_month_needs_a_year_and_says_so_by_returning_none() {
        // Guessing a year would resolve a claim to an activity in a year nobody named.
        assert_eq!(iso("31 August", None), None);
    }

    #[test]
    fn a_book_spanning_years_offers_no_hint() {
        // The hint exists only when it is unambiguous.
        let one = vec![Record {
            id: "AA-001".into(),
            name: "A".into(),
            activities: vec![Activity {
                id: "A1".into(),
                date: "2026-01-01".into(),
                kind: String::new(),
                body: String::new(),
            }],
            by_date: BTreeMap::new(),
        }];
        assert_eq!(year_of(&one).as_deref(), Some("2026"));

        let mut two = one.clone();
        two[0].activities.push(Activity {
            id: "A2".into(),
            date: "2025-01-01".into(),
            kind: String::new(),
            body: String::new(),
        });
        assert_eq!(year_of(&two), None);
    }

    #[test]
    fn record_ids_are_found_and_ordinary_words_are_not() {
        assert_eq!(record_ids_in("NC-003 and LH-004, then NC-003 again"), ["NC-003", "LH-004"]);
        assert!(record_ids_in("no ids here, 2026-08-18").is_empty());
    }

    #[test]
    fn dates_in_a_sentence_are_found_in_every_written_form() {
        let got = dates_in("on 2026-08-18 and again on August 20, 2026", None);
        assert_eq!(got, ["2026-08-18", "2026-08-20"]);
    }

    #[test]
    fn a_date_inside_brackets_is_still_a_date() {
        // "The engagement end date (31 August) has passed." — the brackets are punctuation, and a
        // tokenizer that kept them classified the sentence as naming nothing.
        assert_eq!(dates_in("end date (31 August) has passed", Some("2026")), ["2026-08-31"]);
        assert_eq!(dates_in("signed on August 18, 2026.", None), ["2026-08-18"]);
    }

    #[test]
    fn the_real_book_loads_and_indexes_its_activities() {
        let book = load(&crate::test_root().join("fixtures/account-coordination/book"))
            .expect("the coordination book is here");
        assert_eq!(book.len(), 10);
        assert!(book.iter().any(|r| r.id == "NC-003"), "the contradiction record is in the book");
        assert!(book.iter().all(|r| !r.name.is_empty()));
    }
}
