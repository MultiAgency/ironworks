//! The arc page: cases down, runs across.
//!
//! **Rows are cases and columns are runs**, because the question is never "how did run 3 do" — it
//! is "what happened to `attention`". A per-run list of chips cannot answer that; a row can.
//!
//! The last column is the working tree, and it is **empty on purpose** when nothing graded this
//! composition: an ungraded composition should read as a gap in the evidence rather than as an
//! absence of news.

use super::data::{Composed, Ingredients, Probe, Run, Suite, matching_run};
use ironworks_page::esc;
use std::collections::BTreeMap;

/// PASS before FAIL before BLOCKED, so a tally reads in severity order.
fn order(verdict: &str) -> usize {
    match verdict {
        "PASS" => 0,
        "FAIL" => 1,
        "BLOCKED" => 2,
        _ => 9,
    }
}

fn chip(verdict: &str) -> String {
    let kind = if matches!(verdict, "PASS" | "FAIL" | "BLOCKED") { verdict } else { "OTHER" };
    format!("<span class=\"chip {kind}\">{}</span>", esc(verdict))
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// How many of each verdict, in severity order.
///
/// One function because there were two, and they had drifted apart on the question that matters:
/// the tally rendered as initials emitted its verdict's first character raw, while the tally
/// rendered in full escaped it. `crates/page/src/lib.rs::esc` is a security control over strings
/// nobody here wrote — a verdict is read out of a transcript — so the two renderings must not get
/// to disagree about it. They now share the count and the ordering and differ only in the closure.
fn tally<'a>(verdicts: impl Iterator<Item = &'a str>) -> Vec<(&'a str, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for v in verdicts {
        *counts.entry(v).or_insert(0) += 1;
    }
    let mut pairs: Vec<(&str, usize)> = counts.into_iter().collect();
    pairs.sort_by_key(|(v, _)| (order(v), *v));
    pairs
}

/// A tally, each verdict rendered by `show`, with a narrow no-break space before each.
fn spread<'a>(verdicts: impl Iterator<Item = &'a str>, show: impl Fn(&str) -> String) -> String {
    tally(verdicts)
        .iter()
        .map(|(v, n)| format!("{n}&#8239;{}", show(v)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A tally as initials — `2 P 1 F 1 B`. Escaped, because the initial comes from the transcript.
fn counted<'a>(verdicts: impl Iterator<Item = &'a str>) -> String {
    spread(verdicts, |v| esc(&v.chars().next().unwrap_or('?').to_string()))
}

/// `0009-…-poc.json run 2` → `0009 · run 2`.
fn short(label: &str) -> String {
    let leading: String = label.chars().take_while(char::is_ascii_digit).collect();
    let head = if leading.is_empty() { label.to_owned() } else { leading };
    match label.find("run ") {
        Some(at) => {
            let n: String = label[at + 4..].chars().take_while(char::is_ascii_digit).collect();
            format!("{head} · run {n}")
        }
        None => head,
    }
}

/// Everything the page needs that is not a run.
pub struct Context<'a> {
    pub suite: &'a Suite,
    pub service: &'a str,
    pub now: &'a Composed,
    pub runs: &'a [Run],
    pub probes: &'a [Probe],
    pub then: Option<&'a Ingredients>,
    pub since: &'a str,
    pub guidance_moved: bool,
    pub guidance_added: bool,
    pub head_rev: &'a str,
}

/// The arc's sections, without a page around them.
///
/// Returned as a body rather than a document so the operator page can place the reading beneath
/// it: the matrix and one answer are the same question at two zooms, and a reader who has just
/// seen a cell flip should not have to navigate to find out what the answer said.
/// What one composed part did between the two compositions the page compares.
///
/// **`NotComparable` is the BLOCKED shape and must never collapse into `Unchanged`.** There is no
/// earlier composition to compare against, so nothing was measured — while `Unchanged` is a
/// measurement that came back equal. The page paints them with the same class, which is exactly
/// why the distinction has to live somewhere a test can reach: a reader who is told "unchanged"
/// concludes the part is safe, and a reader told "not comparable" knows they were told nothing.
#[derive(Debug, PartialEq, Eq)]
enum PartState<'a> {
    NotComparable,
    Added,
    Moved { from: &'a str },
    Unchanged,
}

/// `was` is `None` when there is no earlier composition at all — not an empty one, which would
/// mean every part is `Added`.
fn part_state<'a>(
    was: Option<&BTreeMap<&'a str, &'a str>>,
    rel: &str,
    digest: &str,
) -> PartState<'a> {
    let Some(was) = was else { return PartState::NotComparable };
    match was.get(rel) {
        None => PartState::Added,
        Some(before) if *before != digest => PartState::Moved { from: before },
        Some(_) => PartState::Unchanged,
    }
}

/// Which of the three the matrix paints a run's column: `first`, `moved`, or `same`.
///
/// The class is computed once and read twice — the header row and the tally row both index it by
/// position — so it must not be recomputed anywhere. `moved` and `same` are a claim about the
/// **previous** run: a column marked `same` says the composition did not change, which is what
/// makes a verdict that moved under it variance rather than an effect. Painting a moved column
/// `same` would attribute a real change to noise, and it is the one mistake here that changes a
/// reader's conclusion.
fn column_class(runs: &[Run], i: usize) -> &'static str {
    if i == 0 {
        "first"
    } else if runs[i - 1].alloy != runs[i].alloy {
        "moved"
    } else {
        "same"
    }
}

/// Every composition change between consecutive runs, as `from → to`.
fn moved_between(runs: &[Run]) -> Vec<String> {
    (1..runs.len())
        .filter(|i| runs[i - 1].alloy != runs[*i].alloy)
        .map(|i| format!("{} → {}", runs[i - 1].alloy[0], runs[i].alloy[0]))
        .collect()
}

/// Whether any two consecutive runs graded the **same** composition and disagreed.
///
/// This is what licenses the legend's claim that a verdict which moved under an unchanged
/// composition is variance or a regrade rather than the effect of a change. Answering it `true`
/// when every composition differed would offer that excuse for a real regression.
fn has_variance(runs: &[Run]) -> bool {
    (1..runs.len())
        .any(|i| runs[i - 1].alloy == runs[i].alloy && runs[i - 1].verdicts != runs[i].verdicts)
}

pub fn sections(cx: &Context<'_>) -> String {
    let matched = matching_run(cx.runs, &cx.now.alloy);
    let here: Vec<&Probe> =
        cx.probes.iter().filter(|p| p.alloy.len() == 1 && p.alloy[0] == cx.now.alloy).collect();

    let now_note = if let Some(m) = matched {
        format!(
            "repeats {}, the latest recorded run that composed exactly this alloy. It is not the \
             newest run: a composition that moves away and returns would otherwise be shown the \
             verdicts of whatever ran in between.",
            m.label
        )
    } else {
        "is empty because no recorded run composed this alloy. An ungraded composition is a gap in \
         the evidence, and filling it from the nearest run would be the one thing this page exists \
         to prevent."
            .to_owned()
    };

    let also = if here.is_empty() {
        String::new()
    } else {
        format!(
            " {} stability probe(s) also composed it; they are below, not in the matrix.",
            here.len()
        )
    };
    let band = match (matched, cx.runs.last()) {
        (Some(m), _) => format!(
            "<div class=\"band ok\"><b>Graded.</b> {} describes this composition, and the <em>now</em> \
             column below is that run — not the newest one.{}</div>",
            esc(&m.label),
            esc(&also)
        ),
        (None, Some(last)) => format!(
            "<div class=\"band\"><b>Not graded.</b> No recorded run describes this composition. The \
             last graded one was <span class=\"mono\">{}</span>, in {} — so every verdict below \
             describes bytes this binary would no longer send.{}</div>",
            esc(&last.alloy.join("/")),
            esc(&last.label),
            esc(&also)
        ),
        (None, None) => {
            "<div class=\"band\"><b>Not graded.</b> No recorded runs for this suite.</div>"
                .to_owned()
        }
    };

    // Cases in first-seen order across every run, so a case dropped later still has a row.
    let mut cases: Vec<String> = Vec::new();
    for run in cx.runs {
        for (case, _) in &run.verdicts {
            if !cases.contains(case) {
                cases.push(case.clone());
            }
        }
    }

    let marks: Vec<&'static str> = (0..cx.runs.len()).map(|i| column_class(cx.runs, i)).collect();
    let mut heads = String::new();
    for (i, run) in cx.runs.iter().enumerate() {
        let cls = marks[i];
        let alloy =
            if run.alloy.len() == 1 { run.alloy[0].clone() } else { "MOVED MID-RUN".to_owned() };
        heads.push_str(&format!(
            "<th class=\"run {cls}\" title=\"{}\">{}<span class=\"d\">{}</span></th>",
            esc(&run.label),
            esc(&short(&run.label)),
            esc(&alloy)
        ));
    }
    heads.push_str(&format!(
        "<th class=\"run now\" title=\"the working tree\">now<span class=\"d\">{}</span></th>",
        esc(&cx.now.alloy)
    ));

    let mut body = String::new();
    for case in &cases {
        let mut cells = String::new();
        for (i, run) in cx.runs.iter().enumerate() {
            let verdict = run.verdict(case).unwrap_or("—").to_owned();
            // The ledger entry's own words carry into the cell's title, so a reader can see *why*
            // a verdict moved rather than only that it did. Dropping it leaves a marker that
            // reports a change and cannot account for it.
            let mark = match run.regraded.get(case) {
                Some(g) => format!(
                    "<span class=\"rg\" title=\"{}\">regraded from {}</span>",
                    esc(&g.why),
                    esc(&g.was)
                ),
                None => String::new(),
            };
            let kind = if verdict == "—" { "OTHER" } else { &verdict };
            cells.push_str(&format!(
                "<td class=\"v {} {}\">{}{mark}</td>",
                marks[i],
                esc(kind),
                chip(&verdict)
            ));
        }
        let current =
            matched.map_or_else(|| "—".to_owned(), |m| chip(m.verdict(case).unwrap_or("—")));
        body.push_str(&format!(
            "<tr><th scope=\"row\">{}</th>{cells}<td class=\"v now\">{current}</td></tr>",
            esc(case)
        ));
    }

    let mut tally = String::new();
    for (i, run) in cx.runs.iter().enumerate() {
        tally.push_str(&format!(
            "<td class=\"v tally {}\">{}</td>",
            marks[i],
            counted(run.verdicts.iter().map(|(_, v)| v.as_str()))
        ));
    }
    // The now column is the matched run's column, so its tally is that run's tally. Anything else
    // would total a set of cells that is not on the page.
    tally.push_str(&match matched {
        Some(m) => format!(
            "<td class=\"v tally now\">{}</td>",
            counted(m.verdicts.iter().map(|(_, v)| v.as_str()))
        ),
        None => "<td class=\"v tally now\">not graded</td>".to_owned(),
    });

    let moved_between = moved_between(cx.runs);
    let variance = has_variance(cx.runs);

    let mut legend = String::new();
    if !moved_between.is_empty() {
        legend.push_str(&format!(
            "<span class=\"key\"><i class=\"bar moved\"></i>composition moved ({})</span>",
            esc(&moved_between.join("; "))
        ));
    }
    if cx.runs.iter().any(|r| !r.regraded.is_empty()) {
        legend.push_str(
            "<span class=\"key\"><i class=\"bar rgk\"></i>a standing verdict a later reading changed \
             — the transcript keeps what the grader produced at the time</span>",
        );
    }
    if variance {
        legend.push_str(
            "<span class=\"key\"><i class=\"bar same\"></i>same composition — a verdict that moved \
             here is variance or a regrade, not the effect of a change</span>",
        );
    }

    let probe_section = if cx.probes.is_empty() {
        String::new()
    } else {
        let mut prows = String::new();
        for probe in cx.probes {
            let spread = spread(probe.verdicts.iter().map(String::as_str), esc);
            let mark = if probe.alloy.len() == 1 && probe.alloy[0] == cx.now.alloy {
                " <span class=\"moved\">· this composition</span>"
            } else {
                ""
            };
            let alloy = probe.alloy.join("/");
            prows.push_str(&format!(
                "<tr><td class=\"mono\">{}</td><td>{}</td><td class=\"num\">{}{mark}</td>\
                 <td class=\"num\">{}</td><td class=\"mono\">{spread}</td><td>{}</td></tr>",
                esc(&probe.label),
                esc(&probe.case),
                esc(if alloy.is_empty() { "—" } else { &alloy }),
                probe.verdicts.len(),
                probe.verdicts.iter().map(|v| format!("{} ", chip(v))).collect::<String>()
            ));
        }
        format!(
            "
  <section>
    <h2>Stability probes</h2>
    <div class=\"panel scroll\">
      <table>
        <thead><tr><th>Transcript</th><th>Case</th><th>Alloy</th><th>Runs</th><th>Spread</th>
        <th>In order</th></tr></thead>
        <tbody>{prows}</tbody>
      </table>
      <div class=\"legend\"><span class=\"key\">One case graded repeatedly at a fixed composition.
      Not a column in the matrix above: a run that graded one case cannot be compared against
      runs that graded the suite, and its tally would read as a suite score. A spread across
      verdicts at one alloy is variance in the model, not an effect of a change.</span></div>
    </div>
  </section>
"
        )
    };

    let mut rows = String::new();
    let was: BTreeMap<&str, &str> = cx
        .then
        .map(|t| t.parts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect())
        .unwrap_or_default();
    for (rel, digest) in &cx.now.parts {
        let (state, nums) = match part_state(cx.then.map(|_| &was), rel, digest) {
            PartState::NotComparable => {
                ("<span class=\"unchanged\">not comparable</span>".to_owned(), esc(digest))
            }
            PartState::Added => ("<span class=\"added\">added</span>".to_owned(), esc(digest)),
            PartState::Moved { from } => (
                "<span class=\"moved\">moved</span>".to_owned(),
                format!(
                    "{} <span class=\"arrow\">→</span> <span class=\"to\">{}</span>",
                    esc(from),
                    esc(digest)
                ),
            ),
            PartState::Unchanged => {
                ("<span class=\"unchanged\">unchanged</span>".to_owned(), esc(digest))
            }
        };
        rows.push_str(&format!(
            "<tr><td class=\"mono\">{}</td><td class=\"state\">{state}</td><td class=\"num\">{nums}</td></tr>",
            esc(rel)
        ));
    }
    let (gstate, gnote) = if !cx.guidance_moved {
        ("<span class=\"unchanged\">unchanged</span>", esc(&cx.now.guidance))
    } else if cx.guidance_added {
        ("<span class=\"added\">added</span>", format!("not present at {}", esc(cx.since)))
    } else {
        ("<span class=\"moved\">moved</span>", esc(&cx.now.guidance))
    };
    rows.push_str(&format!(
        "<tr><td class=\"mono\">{}</td><td class=\"state\">{gstate}</td><td class=\"num\">{gnote}</td></tr>",
        esc(cx.suite.guidance)
    ));

    let body_html = format!(
        r#"
  <header class="masthead">
    <span class="label">IronWorks · operator</span>
    <h1>Composition arc</h1>
    <p class="sub">An answer-quality verdict is bound to one alloy digest. This is what the
    <b>{service}</b> service composes now, which recorded run last graded it, and what moved
    underneath.</p>
  </header>

  <section>
    <h2>Current composition</h2>
    <div class="panel">
      <div class="now">
        <div class="figure">
          <span class="label">Alloy</span>
          <span class="digest">{alloy}</span>
          <span class="bytes">{alloy_bytes} bytes</span>
        </div>
        <div class="figure">
          <span class="label">Guidance</span>
          <span class="digest small">{guidance}</span>
          <span class="bytes">{guidance_bytes} bytes · never printed</span>
        </div>
        <div class="figure">
          <span class="label">Service</span>
          <span class="digest small">{now_service}</span>
          <span class="bytes">suite {suite_name}</span>
        </div>
      </div>
      {band}
    </div>
  </section>

  <section>
    <h2>Recorded runs</h2>
    <div class="panel scroll">
      <table class="matrix">
        <thead><tr><th class="corner">Case</th>{heads}</tr></thead>
        <tbody>{body}</tbody>
        <tfoot><tr><th scope="row">tally</th>{tally}</tr></tfoot>
      </table>
      <div class="legend">{legend}</div>
    </div>
  </section>

{probe_section}
  <section>
    <h2>Ingredients · {since} → working tree</h2>
    <div class="panel scroll">
      <table>
        <thead><tr><th>Part</th><th>State</th><th>Digest</th></tr></thead>
        <tbody>{rows}</tbody>
      </table>
    </div>
  </section>

  <section class="cannot">
    <h2>What this cannot say</h2>
    <p>Ingredient digests were not recorded with the runs above, so a verdict move cannot be
    attributed to an ingredient retroactively. <span class="mono">ironworks compose</span> makes
    that possible from the next run onward: record its output beside the transcript.</p>
    <p>Verdicts shown are the ones that stand. A transcript is never rewritten — it holds what
    the grader produced at the time — so a later finding is recorded in
    <span class="mono">docs/experiments/regrades.json</span> and applied here, with the cell
    marked and the original named. A regrade is a reading of an answer already given, never a
    second run.</p>

    <p>The <em>now</em> column {now_note}</p>

    <p>Transcripts that grade a single case repeatedly are listed as stability probes rather than
    as columns, because a run that graded one case cannot be scored against runs that graded the
    suite. They are separated by shape, never by file name, so nothing is omitted for being
    called something else.</p>

    <p>Guidance is reported as a digest and never as text. An alloy fuses scrap <em>and</em> a
    tenant's guidance, so a composition can move with no repo diff at all — which is why the two
    are shown side by side rather than one standing in for the other.</p>
  </section>

  <footer>
    Generated from <span class="mono">docs/experiments/</span> and
    <span class="mono">ironworks compose</span> at <span class="mono">{head_rev}</span>.
    Every record behind these verdicts is synthetic.
    <a href="/read/">Reading the answers →</a>
  </footer>
"#,
        service = esc(cx.service),
        alloy = esc(&cx.now.alloy),
        alloy_bytes = thousands(cx.now.alloy_bytes),
        guidance = esc(&cx.now.guidance),
        guidance_bytes = thousands(cx.now.guidance_bytes),
        now_service = esc(&cx.now.service),
        suite_name = esc(cx.suite.name),
        now_note = esc(&now_note),
        since = esc(cx.since),
        head_rev = esc(cx.head_rev),
    );
    body_html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(alloy: &[&str], verdicts: &[(&str, &str)]) -> Run {
        Run {
            label: "0001-r".to_owned(),
            alloy: alloy.iter().map(|a| (*a).to_owned()).collect(),
            verdicts: verdicts.iter().map(|(c, v)| ((*c).to_owned(), (*v).to_owned())).collect(),
            regraded: BTreeMap::new(),
        }
    }

    /// A part that was never compared must not read as one that came back equal.
    ///
    /// Both are painted `unchanged` on the page, so nothing downstream can tell them apart — this
    /// is the only place the difference exists, and §F-14's rule is the same one: an unmeasured
    /// result is not a passing one.
    #[test]
    fn a_part_that_was_never_compared_is_not_a_part_that_did_not_move() {
        let was: BTreeMap<&str, &str> = [("p.md", "aaaa")].into_iter().collect();

        // No earlier composition at all: nothing was measured.
        assert_eq!(part_state(None, "p.md", "aaaa"), PartState::NotComparable);
        // The same digest, and this time it was actually compared.
        assert_eq!(part_state(Some(&was), "p.md", "aaaa"), PartState::Unchanged);

        assert_eq!(part_state(Some(&was), "p.md", "bbbb"), PartState::Moved { from: "aaaa" });
        assert_eq!(part_state(Some(&was), "new.md", "cccc"), PartState::Added);

        // An *empty* earlier composition is a comparison, not the absence of one: every part is
        // new. Folding it into `NotComparable` would hide a composition that added everything.
        let none: BTreeMap<&str, &str> = BTreeMap::new();
        assert_eq!(part_state(Some(&none), "p.md", "aaaa"), PartState::Added);
    }

    /// The matrix's three column classes, and what each one licenses a reader to conclude.
    ///
    /// `same` is not decoration: the legend uses it to say a verdict that moved under it is
    /// variance or a regrade rather than the effect of a change. Painting a column that *did*
    /// move as `same` offers that excuse for a real regression, which is the one error here that
    /// changes a conclusion.
    #[test]
    fn a_column_is_marked_same_only_when_the_composition_did_not_move() {
        let a = run(&["aaaa"], &[("c", "PASS")]);
        let b = run(&["bbbb"], &[("c", "PASS")]);

        // The first column has no previous run to have moved from, so it is neither.
        assert_eq!(column_class(std::slice::from_ref(&a), 0), "first");
        assert_eq!(column_class(&[a.clone(), a.clone()], 0), "first");

        assert_eq!(column_class(&[a.clone(), a.clone()], 1), "same");
        assert_eq!(column_class(&[a.clone(), b.clone()], 1), "moved");

        // A run that graded two compositions is not equal to either of them.
        let both = run(&["aaaa", "bbbb"], &[("c", "PASS")]);
        assert_eq!(column_class(&[a.clone(), both.clone()], 1), "moved");
        assert_eq!(column_class(&[both.clone(), a.clone()], 1), "moved");
    }

    /// The two claims the legend makes about the evidence, each with the case that must not
    /// produce it — a check that can only invent a match is the same defect as one that can only
    /// invent a difference (§K-8).
    #[test]
    fn the_legend_claims_variance_only_where_one_composition_disagreed_with_itself() {
        let a = run(&["aaaa"], &[("c", "PASS")]);
        let a2 = run(&["aaaa"], &[("c", "FAIL")]);
        let b = run(&["bbbb"], &[("c", "FAIL")]);

        // Same alloy, different verdicts: variance, and this is the only shape that is.
        assert!(has_variance(&[a.clone(), a2.clone()]));
        // Same alloy, same verdicts: nothing moved at all.
        assert!(!has_variance(&[a.clone(), a.clone()]));
        // Different alloy AND different verdicts: that is the effect of a change, and calling it
        // variance would excuse a regression the page exists to show.
        assert!(!has_variance(&[a.clone(), b.clone()]));
        // Nothing to compare.
        assert!(!has_variance(std::slice::from_ref(&a)));
        assert!(!has_variance(&[]));

        // And the moves are named in order, from and to, only where there was one.
        assert_eq!(moved_between(&[a.clone(), b.clone()]), vec!["aaaa → bbbb".to_owned()]);
        assert!(moved_between(&[a.clone(), a.clone()]).is_empty());
        assert_eq!(
            moved_between(&[a.clone(), b.clone(), a.clone()]),
            vec!["aaaa → bbbb".to_owned(), "bbbb → aaaa".to_owned()]
        );
    }

    /// Nothing a transcript or a composition carries reaches the arc page as markup.
    ///
    /// # Why this renderer especially
    ///
    /// `sections` interpolates through `esc` at **37 separate places**, which is 37 chances to
    /// forget one — and every value it renders comes from outside: run labels and case names are
    /// read from transcript filenames and rows, part paths and digests from the composition, and
    /// verdicts from recorded runs. A missed `esc` here publishes attacker-reachable bytes as
    /// markup on the page `docs/FRONTEND.md` describes (§A-1).
    ///
    /// The payload goes in **every** `String` the context carries, so the test does not depend on
    /// guessing which field a future edit will forget.
    #[test]
    fn no_transcript_or_composition_field_reaches_the_arc_page_as_markup() {
        use crate::arc::data::{Composed, Ingredients, Probe, Run, Suite};
        const PAYLOAD: &str = "<img src=x onerror=alert(1)>";
        let poisoned = |what: &str| format!("{what}{PAYLOAD}");

        let suite = Suite {
            name: "account-coordination",
            patterns: &["account-coordination"],
            slug: "acme",
            service: Some("relationship-intelligence"),
            guidance: "fixtures/account-coordination/guidance.md",
        };
        let now = Composed {
            service: poisoned("svc"),
            alloy: poisoned("a1b2"),
            alloy_bytes: 14_337,
            guidance: poisoned("g1"),
            guidance_bytes: 900,
            parts: vec![(poisoned("personas/A.md"), poisoned("d1"))],
        };
        let runs = vec![Run {
            label: poisoned("0021-run"),
            alloy: vec![now.alloy.clone()],
            verdicts: vec![(poisoned("attention"), poisoned("PASS"))],
            regraded: std::collections::BTreeMap::new(),
        }];
        let probes = vec![Probe {
            label: poisoned("0023-probe"),
            case: poisoned("attention"),
            alloy: vec![now.alloy.clone()],
            verdicts: vec![poisoned("PASS")],
        }];
        let then = Ingredients {
            parts: vec![(poisoned("personas/B.md"), poisoned("d2"))],
            missing: vec![poisoned("personas/gone.md")],
        };

        let html = sections(&Context {
            suite: &suite,
            service: &poisoned("relationship-intelligence"),
            now: &now,
            runs: &runs,
            probes: &probes,
            then: Some(&then),
            since: &poisoned("HEAD"),
            guidance_moved: true,
            guidance_added: true,
            head_rev: &poisoned("4cb47cf"),
        });

        assert!(
            !html.contains(PAYLOAD),
            "a field reached the arc page verbatim — some interpolation is unescaped:\n{html}"
        );
        assert!(!html.contains("<img"), "an <img> tag reached the arc page:\n{html}");

        // The positive control §K-8 requires: the payload must be present *escaped*, and the
        // page's own markup must survive. Without these, a renderer that emitted nothing would
        // pass every assertion above.
        assert!(html.contains("&lt;img"), "the payload must appear, escaped:\n{html}");
        assert!(html.contains("<table") || html.contains("<div"), "the page must render:\n{html}");
    }

    #[test]
    fn a_tally_reads_in_severity_order() {
        let verdicts = ["FAIL", "PASS", "PASS", "BLOCKED"];
        assert_eq!(counted(verdicts.into_iter()), "2&#8239;P 1&#8239;F 1&#8239;B");
    }

    #[test]
    fn a_tally_escapes_the_verdict_it_was_given() {
        // A verdict is read out of a transcript, so it is not ours. The initial rendering used to
        // emit the first character raw while the full rendering escaped — one tally, two answers
        // to the question `crates/page/src/lib.rs::esc` exists to answer. Both go through it now.
        assert_eq!(counted(["<script>"].into_iter()), "1&#8239;&lt;");
        assert_eq!(spread(["<b>"].into_iter(), esc), "1&#8239;&lt;b&gt;");

        // The positive control: an ordinary verdict is unchanged by the escaper, so the assertion
        // above is not passing because everything is being mangled.
        assert_eq!(spread(["PASS"].into_iter(), esc), "1&#8239;PASS");
    }

    #[test]
    fn a_run_label_shortens_to_its_number_and_run() {
        assert_eq!(short("0009-account-coordination-poc.json"), "0009");
        assert_eq!(short("0009-account-coordination-poc.json run 2"), "0009 · run 2");
    }

    #[test]
    fn an_unrecognised_verdict_chips_as_other() {
        // A dash must not be styled as though it were a verdict.
        assert!(chip("—").contains("chip OTHER"));
        assert!(chip("PASS").contains("chip PASS"));
    }
}
