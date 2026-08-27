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

/// A tally in severity order, with a narrow no-break space before each initial.
fn counted(verdicts: impl Iterator<Item = String>) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in verdicts {
        *counts.entry(v).or_insert(0) += 1;
    }
    let mut pairs: Vec<(&String, &usize)> = counts.iter().collect();
    pairs.sort_by_key(|(v, _)| (order(v), (*v).clone()));
    pairs
        .iter()
        .map(|(v, n)| format!("{n}&#8239;{}", v.chars().next().unwrap_or('?')))
        .collect::<Vec<_>>()
        .join(" ")
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
    pub guidance_rel: &'a str,
    pub guidance_moved: bool,
    pub guidance_added: bool,
    pub head_rev: &'a str,
}

/// The arc's sections, without a page around them.
///
/// Returned as a body rather than a document so the operator page can place the reading beneath
/// it: the matrix and one answer are the same question at two zooms, and a reader who has just
/// seen a cell flip should not have to navigate to find out what the answer said.
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

    let mut heads = String::new();
    let mut marks = Vec::new();
    for (i, run) in cx.runs.iter().enumerate() {
        let moved = i > 0 && cx.runs[i - 1].alloy != run.alloy;
        let cls = if moved {
            "moved"
        } else if i > 0 {
            "same"
        } else {
            "first"
        };
        let alloy =
            if run.alloy.len() == 1 { run.alloy[0].clone() } else { "MOVED MID-RUN".to_owned() };
        heads.push_str(&format!(
            "<th class=\"run {cls}\" title=\"{}\">{}<span class=\"d\">{}</span></th>",
            esc(&run.label),
            esc(&short(&run.label)),
            esc(&alloy)
        ));
        marks.push(cls);
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
            counted(run.verdicts.iter().map(|(_, v)| v.clone()))
        ));
    }
    // The now column is the matched run's column, so its tally is that run's tally. Anything else
    // would total a set of cells that is not on the page.
    tally.push_str(&match matched {
        Some(m) => format!(
            "<td class=\"v tally now\">{}</td>",
            counted(m.verdicts.iter().map(|(_, v)| v.clone()))
        ),
        None => "<td class=\"v tally now\">not graded</td>".to_owned(),
    });

    let moved_between: Vec<String> = (1..cx.runs.len())
        .filter(|i| cx.runs[i - 1].alloy != cx.runs[*i].alloy)
        .map(|i| format!("{} → {}", cx.runs[i - 1].alloy[0], cx.runs[i].alloy[0]))
        .collect();
    let variance = (1..cx.runs.len()).any(|i| {
        cx.runs[i - 1].alloy == cx.runs[i].alloy && cx.runs[i - 1].verdicts != cx.runs[i].verdicts
    });

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
            let spread = {
                let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                for v in &probe.verdicts {
                    *counts.entry(v).or_insert(0) += 1;
                }
                let mut pairs: Vec<(&&str, &usize)> = counts.iter().collect();
                pairs.sort_by_key(|(v, _)| (order(v), **v));
                pairs
                    .iter()
                    .map(|(v, n)| format!("{n}&#8239;{}", esc(v)))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
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
        let (state, nums) = if cx.then.is_none() {
            ("<span class=\"unchanged\">not comparable</span>".to_owned(), esc(digest))
        } else if !was.contains_key(rel.as_str()) {
            ("<span class=\"added\">added</span>".to_owned(), esc(digest))
        } else if was[rel.as_str()] != digest {
            (
                "<span class=\"moved\">moved</span>".to_owned(),
                format!(
                    "{} <span class=\"arrow\">→</span> <span class=\"to\">{}</span>",
                    esc(was[rel.as_str()]),
                    esc(digest)
                ),
            )
        } else {
            ("<span class=\"unchanged\">unchanged</span>".to_owned(), esc(digest))
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
        esc(cx.guidance_rel)
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

    #[test]
    fn a_tally_reads_in_severity_order() {
        let verdicts = ["FAIL", "PASS", "PASS", "BLOCKED"].map(str::to_owned);
        assert_eq!(counted(verdicts.into_iter()), "2&#8239;P 1&#8239;F 1&#8239;B");
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
