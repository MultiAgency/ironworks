//! Every sentence of a graded answer, shown against the record it names.
//!
//! **An unmarked sentence is not a verified one.** This resolves references; it cannot say whether
//! the cited activity supports the claim. The defect that prompted the tool cited real dated
//! evidence and concluded more than that evidence established — which is why the records sit in
//! the next column. The reading is the work.

use super::anchors::{Kind, anchors_in, classify, sentences};
use super::book::Book;
use ironworks_evidence::regrades::Regrade;
use ironworks_page::esc;
use serde_json::Value;
use std::collections::BTreeMap;

/// Escape, then restore the only markdown these answers actually use.
pub fn inline(text: &str) -> String {
    let escaped = esc(text);
    crate::markdown::replace_bold(&escaped, &["FACT", "STATED", "UNKNOWN"])
}

/// The heading for one reading: the case, plus its run when the case was run more than once.
///
/// A stability probe records the same case many times. Sections all headed `attention` read as one
/// answer rendered five times rather than as five answers to one question, which is the entire
/// point of such a run.
pub fn label_of(row: &Value, repeated: bool) -> String {
    let case = row.get("case").and_then(Value::as_str).unwrap_or_default();
    match row.get("run").and_then(Value::as_u64) {
        Some(run) if repeated => format!("{case} · run {run}"),
        _ => case.to_owned(),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counts {
    pub anchored: usize,
    pub named: usize,
    pub inherited: usize,
    pub bare: usize,
}

pub struct Rendered {
    pub html: String,
    pub counts: Counts,
}

/// Whether to show the grader's own output beside the answer.
///
/// **The operator view shows it; the demo does not.** A verdict chip and a counts panel are
/// instrumentation: the grade is a statement about the *grader*, phrased in its vocabulary
/// ("asserted non-existence exceeds the record"), and a stranger reading that learns nothing about
/// the service except that something they cannot parse went wrong. Honesty about the grades
/// belongs on the demo in prose, where it can be explained, not as an unexplained red chip above
/// the first answer anyone sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instrumentation {
    Shown,
    Hidden,
}

pub fn render_case(
    row: &Value,
    book: &Book,
    hint: Option<&str>,
    regrade: Option<&Regrade>,
    label: &str,
    instrumentation: Instrumentation,
) -> Rendered {
    let question = row.get("question").and_then(Value::as_str).unwrap_or_default();
    let answer = row.get("answer").and_then(Value::as_str).unwrap_or_default();

    let mut counts = Counts::default();
    let mut seen: std::collections::BTreeSet<(usize, usize)> = std::collections::BTreeSet::new();
    let mut evidence: Vec<(usize, usize, bool)> = Vec::new();
    let mut html: Vec<String> = Vec::new();

    // A single-account question puts the whole answer in that account's scope — "Has Northline
    // Cooperative renewed?" is answered about Northline whether or not the prose says so again.
    // Without this the commentary in every single-account answer reads as standing on nothing.
    let asked = anchors_in(question, book, hint, &[]).records;

    for block in sentences(answer) {
        // One pass to learn what the block is about, so a sentence continuing it is read in that
        // scope — and so a date in it resolves to the account the block named rather than to
        // whichever record happens to share the date.
        let mut within = asked.clone();
        for line in &block {
            for part in &line.parts {
                for r in anchors_in(part, book, hint, &[]).records {
                    if !within.contains(&r) {
                        within.push(r);
                    }
                }
            }
        }
        let scope = !within.is_empty();
        for line in &block {
            let mut marked = Vec::new();
            for part in &line.parts {
                let a = anchors_in(part, book, hint, &within);
                let kind = classify(&a, line.heading, scope);
                match kind {
                    Kind::Anchored => counts.anchored += 1,
                    Kind::Named => counts.named += 1,
                    Kind::Inherited => counts.inherited += 1,
                    Kind::Bare => counts.bare += 1,
                    Kind::Heading => {}
                }
                for hit in &a.activities {
                    let key = (hit.record, hit.activity);
                    if seen.insert(key) {
                        evidence.push((hit.record, hit.activity, hit.pinned));
                    }
                }
                let title = if a.activities.is_empty() {
                    if kind == Kind::Bare {
                        " title=\"names no record and no date the book holds\"".to_owned()
                    } else {
                        String::new()
                    }
                } else {
                    let cited: std::collections::BTreeSet<String> = a
                        .activities
                        .iter()
                        .map(|h| {
                            format!(
                                "{} {}",
                                book[h.record].id, book[h.record].activities[h.activity].id
                            )
                        })
                        .collect();
                    format!(" title=\"{}\"", esc(&cited.into_iter().collect::<Vec<_>>().join(", ")))
                };
                marked.push(format!(
                    "<span class=\"s {}\"{title}>{}</span>",
                    kind.as_str(),
                    inline(part)
                ));
            }
            let text = marked.join(" ");
            if line.bullet {
                // Consecutive bullets are one list. A list per item doubles the spacing and reads
                // as several lists of one.
                if html.last().is_some_and(|h| h.ends_with("</li></ul>")) {
                    let last = html.len() - 1;
                    let trimmed = html[last].trim_end_matches("</ul>").to_owned();
                    html[last] = format!("{trimmed}<li>{text}</li></ul>");
                } else {
                    html.push(format!("<ul><li>{text}</li></ul>"));
                }
            } else if line.heading {
                html.push(format!("<div class=\"h\">{text}</div>"));
            } else {
                html.push(format!("<p>{text}</p>"));
            }
        }
    }

    let mut ev = String::new();
    for (r, a, pinned) in &evidence {
        let rec = &book[*r];
        let act = &rec.activities[*a];
        let loose =
            if *pinned { "" } else { " <span class=\"loose\">· matched on date alone</span>" };
        ev.push_str(&format!(
            "<div class=\"item\"><span class=\"meta\">{} · {} · {} · {}{}{loose}</span>\
             <div class=\"body\">{}</div></div>",
            esc(&rec.id),
            esc(&rec.name),
            esc(&act.id),
            esc(if act.date.is_empty() { "undated" } else { &act.date }),
            if act.kind.is_empty() { String::new() } else { format!(" · {}", esc(&act.kind)) },
            esc(&act.body)
        ));
    }
    if evidence.is_empty() {
        ev.push_str(
            "<p class=\"loose\">This answer resolves to no dated activity in the book.</p>",
        );
    }

    let stored = row.get("verdict").and_then(Value::as_str).unwrap_or_default();
    let verdict = regrade.map_or(stored, |g| g.verdict.as_str());
    let mark = regrade.map_or(String::new(), |g| {
        format!("<span class=\"rg\" title=\"{}\">regraded from {}</span>", esc(&g.why), esc(&g.was))
    });
    let warn = if counts.bare > 0 { " class=\"warn\"" } else { "" };
    let (grade_block, counts_block) = match instrumentation {
        Instrumentation::Hidden => (String::new(), String::new()),
        Instrumentation::Shown => (
            format!(
                "<div class=\"grade\">\n        <span class=\"chip {verdict}\">{verdict}</span>{mark}\n        <span class=\"why\">{why}</span>\n      </div>",
                verdict = esc(verdict),
                why = esc(row.get("why").and_then(Value::as_str).unwrap_or_default()),
            ),
            format!(
                "<div class=\"counts\">\n        <span><b>{}</b> anchored to a dated activity</span>\n        <span><b>{}</b> name a record, no date the book holds</span>\n        <span><b>{}</b> continue a block that named one</span>\n        <span{warn}><b>{}</b> name nothing at all</span>\n      </div>",
                counts.anchored, counts.named, counts.inherited, counts.bare
            ),
        ),
    };

    let html = format!(
        r#"
  <section class="case">
    <h2>{label}</h2>
    <div class="panel">
      <p class="ask">{question}</p>
      {grade_block}
      <div class="cols">
        <div class="answer">{answer_html}</div>
        <div class="ev">
          <span class="label">Records this answer names</span>
          {ev}
        </div>
      </div>
      {counts_block}
    </div>
  </section>"#,
        label = esc(label),
        question = esc(question),
        answer_html = html.join(""),
    );
    Rendered { html, counts }
}

/// The unanchored-sentence result, stated from what was actually counted.
///
/// It used to be a sentence typed into the page saying there were none — true of the transcript it
/// was written against, and left standing for every other one. **A generated page must not carry a
/// hand-written result**; that is the same rule the digests obey one file over.
pub fn bare_finding(bare: usize, cases: usize, one_case: bool) -> (String, String) {
    let scope = if one_case || cases == 1 {
        format!("this {}", if one_case { "answer" } else { "reading" })
    } else {
        format!("these {cases} answers")
    };
    if bare == 0 {
        return (
            format!("No sentence in {scope} stands on nothing."),
            "That is a real result and a small one: the marker is a cheap guard against a \
             free-floating assertion, not the reason to read the page."
                .to_owned(),
        );
    }
    (
        format!(
            "{bare} sentence{} in {scope} stand{} on nothing at all.",
            if bare == 1 { "" } else { "s" },
            if bare == 1 { "s" } else { "" }
        ),
        "Each is on the amber ground below, and names no record and no date the book holds. That \
         is where to start reading, not where to stop: the marker is a cheap guard against a \
         free-floating assertion, not the reason to read the page."
            .to_owned(),
    )
}

/// Totals across every case in a reading.
pub fn total(counts: &[Counts]) -> Counts {
    counts.iter().fold(Counts::default(), |mut acc, c| {
        acc.anchored += c.anchored;
        acc.named += c.named;
        acc.inherited += c.inherited;
        acc.bare += c.bare;
        acc
    })
}

/// The regrade for one case in one transcript, if the ledger holds one.
pub fn regrade_for<'a>(
    ledger: &'a BTreeMap<(String, String), Regrade>,
    transcript: &str,
    case: &str,
) -> Option<&'a Regrade> {
    ledger.get(&(transcript.to_owned(), case.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finding_is_counted_never_typed() {
        // The defect it replaced: a sentence saying "none" that stayed true of one transcript and
        // wrong for every other.
        assert!(bare_finding(0, 7, false).0.starts_with("No sentence in these 7 answers"));
        assert!(bare_finding(1, 7, false).0.starts_with("1 sentence in these 7 answers stands"));
        assert!(bare_finding(3, 7, false).0.starts_with("3 sentences in these 7 answers stand"));
    }

    #[test]
    fn reading_one_case_says_answer_not_reading() {
        // A `--case` reading of a single answer must not make a claim about seven.
        assert!(bare_finding(0, 1, true).0.contains("this answer"));
        assert!(bare_finding(0, 1, false).0.contains("this reading"));
    }

    #[test]
    fn a_repeated_case_is_labelled_by_its_run() {
        let row = serde_json::json!({"case": "attention", "run": 3});
        assert_eq!(label_of(&row, true), "attention · run 3");
        assert_eq!(label_of(&row, false), "attention", "one run needs no number");
    }

    #[test]
    fn evidence_tiers_survive_escaping_as_labels() {
        assert!(inline("**FACT**").contains("lbl FACT"));
        assert!(inline("<script>").contains("&lt;script&gt;"));
    }
}
