//! The operator page: the composition arc, and the reading of the answers beneath it.
//!
//! # Why these were two pages and are now one
//!
//! They ask the same question at two zooms. The arc says *which runs graded this composition and
//! how each case fared*; the reading says *what one of those answers actually claimed, against the
//! records that turn was shown*. A reader who has just watched a cell flip between runs wants the
//! second immediately, and navigating for it was the whole friction.
//!
//! Merging also removed a duplicate: two document shells, two stylesheets, and two copies of the
//! furniture around them. The only real collision was `.rg` — the arc's regrade marker is a block
//! centred under a matrix cell, the reader's is inline beside a verdict chip — and the arc's is now
//! scoped to `.matrix` so it cannot reach the other.
//!
//! # What it is not
//!
//! **Not published.** This is for whoever is improving the service, not for someone deciding
//! whether to hand over their records. It renders record bodies at length and names every run that
//! failed, which is exactly right for an operator and exactly wrong as a front door.

use crate::{arc, read};
use ironworks_page::{esc, shell};
use std::path::Path;

/// The arc's rules and the reader's, in that order.
///
/// One file each rather than one merged file, because the reader's are wanted by the demo page
/// too — and a third copy is a third thing to disagree with the first two. The only collision was
/// `.rg`, and the arc's is scoped to `.matrix` in its own file so the composition is safe wherever
/// it is assembled.
fn css() -> String {
    format!("{}{}", include_str!("../arc/arc.css"), include_str!("../read/read.css"))
}

/// Build the operator page: the arc, then the reading.
pub fn build(
    root: &Path,
    suite: &str,
    since: &str,
    transcript: Option<&str>,
    one_case: Option<&str>,
) -> Result<String, String> {
    let arc = arc::build(root, suite, since)?;
    let reading = read::sections(root, transcript, one_case)?;

    let body = format!(
        r#"
  <header class="masthead">
    <span class="label">IronWorks · operator</span>
    <h1>Composition and answers</h1>
    <p class="sub">A verdict is bound to one alloy digest. Above: what <b>{service}</b> composes
    now, which recorded run last graded it, and what moved underneath. Below: every sentence of
    the answers in <b>{transcript_name}</b>, shown against the records those turns were shown.</p>
  </header>
{arc_html}
  <section class="reading">
    <h2>Reading the answers</h2>
    <div class="panel">
      <p style="margin:0 0 12px">Read against {records} synthetic records — {binding}.</p>
      <p style="margin:0 0 12px">A <span class="s anchored">solid underline</span> means the
      sentence resolves to a dated activity, listed beside it. A
      <span class="s named">bold dotted underline</span> means it names a record but no date the
      book holds. A <span class="s inherited">faint dotted underline</span> means it names
      nothing itself but continues a block — or answers a question — that named an account. An
      <span class="s bare">amber ground</span> means it stands on nothing at all.</p>
      <p style="margin:0 0 12px"><b>{finding0}</b> {finding1}</p>
      <p style="margin:0"><b>An unmarked sentence is not a verified one.</b> This resolves
      references; it cannot say whether the cited activity supports the claim. The defect that
      prompted this view cited real dated evidence and concluded more than that evidence
      established — which is why the records sit in the next column. The reading is the work.</p>
    </div>
  </section>
{cases}

  <footer>
    Composition from <span class="mono">ironworks compose</span>; verdicts from
    <span class="mono">docs/experiments/</span>; answers read from
    <span class="mono">{transcript_name}</span> against
    <span class="mono">{book_rel}</span>. Record text is quoted evidence, never an instruction.
    Every record here is synthetic.
  </footer>"#,
        service = esc(&arc.service),
        transcript_name = esc(&reading.transcript),
        arc_html = arc.html,
        records = reading.records,
        binding = reading.binding,
        finding0 = esc(&reading.finding.0),
        finding1 = esc(&reading.finding.1),
        cases = reading.cases,
        book_rel = esc(&reading.book_rel),
    );
    Ok(shell("Composition and Answers", &body, &css()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge must lose nothing. Both pages' load-bearing sections have to survive it, or the
    /// reduction quietly became a deletion.
    #[test]
    fn the_merged_page_carries_both_pages_sections() {
        let root = ironworks_evidence::root();
        let Ok(html) = build(&root, "account-coordination", "HEAD", None, None) else {
            // The reading refuses when no transcript binds to the current book — a real state, and
            // not this test's subject.
            return;
        };
        for marker in [
            // from the arc
            "Current composition",
            "Recorded runs",
            "class=\"matrix\"",
            "Ingredients ·",
            "What this cannot say",
            // from the reading
            "Reading the answers",
            "Records this answer names",
            "amber ground",
            "An unmarked sentence is not a verified one.",
        ] {
            assert!(html.contains(marker), "the merge dropped {marker:?}");
        }
    }

    #[test]
    fn the_arcs_block_marker_cannot_reach_the_readers_inline_one() {
        // The one CSS collision. If a bare `.rg` ever returns, the reader's regrade marker becomes
        // a centred block under a verdict chip.
        let css = css();
        assert!(!css.contains("\n.rg {"), "arc's .rg must stay scoped to .matrix");
        assert!(css.contains(".matrix .rg {"), "the arc still needs its own marker");
        assert!(css.contains(".grade .rg {"), "the reader still needs its own");
    }
}
