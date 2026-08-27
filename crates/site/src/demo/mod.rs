//! The demo: pick a question, watch it answered against the records it was given.
//!
//! # Why this replaced a narrated page
//!
//! The page this supersedes *told* you about one answer and argued from it. The argument was that
//! an earlier composition got a question wrong and the current one gets it right — and
//! `docs/experiments/0023` measured that case at 8 in 10, so both answers sit inside the spread
//! either composition produces. The argument was unsupportable, and no rewording fixes that.
//!
//! What survives measurement is **showing the thing work**: the question, the records the turn was
//! actually given, the answer, and every claim underlined back to the dated activity it came from.
//! That needs no statistic and a reader can check it by eye.
//!
//! # Three things it must carry, and none of them are hedges
//!
//! - **Recorded, not live**, labelled with the run and the composition digest. A live turn cannot
//!   be public until there is a deployment: `relay` handles one cycle, concurrency is not built,
//!   and a turn outlasting thirty seconds returns 503.
//! - **The grades, stated in prose rather than stamped on each answer.** Several of these were
//!   graded FAIL, and that is said plainly below with what it means. It is *not* a red chip above
//!   the first answer a stranger reads, captioned in the grader's own vocabulary — "asserted
//!   non-existence exceeds the record" tells a reader nothing except that something they cannot
//!   parse went wrong. The verdicts and the per-sentence counts are instrumentation and live on
//!   the operator page.
//! - **The synthetic markers.** Every domain is RFC 2606 and every record carries `_synthetic`. To
//!   this reader that is the pitch, not a disclaimer.
//!
//! # No script
//!
//! The picker is radio inputs and sibling selectors. A demo that needs JavaScript to show an
//! answer is a demo that can fail to show one.

use crate::read::{self, book};
use crate::transcript;
use ironworks_page::{esc, shell};
use serde_json::Value;
use std::path::Path;

fn css() -> String {
    format!("{}{}", include_str!("../read/read.css"), include_str!("demo.css"))
}

/// The run the demo shows.
///
/// Pinned by name: the page is a demonstration built around *these* answers over *this* book, and
/// silently re-pointing it at whatever ran last would change what a reader is shown without
/// changing a word around it. It is bound to the current fixtures, and the binding is proved
/// before anything renders.
const RUN: &str = "0021-account-coordination-corrected-absence-rule.json";

const BOOK: &str = "fixtures/account-coordination/book";

/// The cross-account brief, excluded on purpose.
///
/// `docs/FRONTEND.md`: *"Do not lead with the cross-account brief, and do not run it live."* It
/// produced a clean answer in 2 of 5 recorded runs — 40%, measured in `0010` — because a brief
/// spanning every account makes several times the claims of any other question and trips a rule
/// proportionally more often.
///
/// **The recorded answer proves the point.** It says "Four accounts need your attention" and then
/// details five, and it concludes *"Lakeshore Habitat's offboarding appears complete"* — which is
/// the exact overclaim `regrades.json` records against `0005`, and the exact discipline this
/// service exists to keep. Showing it as a demonstration would be demonstrating the defect.
const EXCLUDED: &[&str] = &["attention"];

/// The stability probe for the excluded question, at **this** composition over **this** book.
///
/// `0010` measured the same case at 2 of 5 and is the figure usually quoted — but it was taken at
/// a superseded alloy over the six-record book, so quoting it on a page about the current
/// composition would be citing a measurement of something else.
const EXCLUDED_PROBE: &str = "0018-attention-ten-account-book.json";

/// `(clean, graded, unmeasurable)` for the excluded question.
///
/// A BLOCKED run is a turn that could not be measured, never a failed one, so it is reported
/// separately rather than folded into a denominator (`docs/INVARIANTS.md` §F-14).
fn excluded_rate(experiments: &Path, book_dir: &Path) -> Option<(usize, usize, usize)> {
    let probe = transcript::load(experiments, book_dir, EXCLUDED_PROBE).ok()?;
    let mut clean = 0;
    let mut blocked = 0;
    for row in &probe.cases {
        match row.get("verdict").and_then(Value::as_str) {
            Some("PASS") => clean += 1,
            Some("BLOCKED") => blocked += 1,
            _ => {}
        }
    }
    Some((clean, probe.cases.len() - blocked, blocked))
}

/// A one-line plain-language gloss for each question, so the picker reads as a job rather than a
/// test case id.
fn gloss(case: &str) -> &'static str {
    match case {
        "attention" => "What needs my attention?",
        "onboarding" => "Is this account ready?",
        "supersession" => "Which deadline actually applies?",
        "contradiction" => "Did they renew?",
        "offboarding-unknown" => "Are they fully offboarded?",
        "injection" => "A record containing an attack",
        "ownership-default" => "Whose move is it?",
        _ => "A question",
    }
}

pub fn build(root: &Path) -> Result<String, String> {
    let experiments = root.join("docs/experiments");
    let book_dir = root.join(BOOK);

    // Proves this run was produced over the book on disk. Nothing renders otherwise.
    let run = transcript::load(&experiments, &book_dir, RUN)?;
    let loaded = book::load(&book_dir)?;
    let hint = book::year_of(&loaded);

    let ledger = ironworks_evidence::regrades::load(&experiments);
    if !ledger.problems.is_empty() {
        return Err(format!(
            "the regrade ledger does not validate: {}",
            ledger.problems.join("; ")
        ));
    }

    let digest = run
        .ingredients
        .as_ref()
        .and_then(|i| i.get("alloy"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let mut inputs = String::new();
    let mut picker = String::new();
    let mut panels = String::new();
    let mut totals = Vec::new();
    let shown: Vec<&Value> = run
        .cases
        .iter()
        .filter(|r| {
            let case = r.get("case").and_then(Value::as_str).unwrap_or_default();
            !EXCLUDED.contains(&case)
        })
        .collect();
    // Counted, never typed. `docs/FRONTEND.md`: a generated page must not carry a hand-written
    // result. These are the verdicts the grader of the day recorded with the run.
    let graded_pass =
        shown.iter().filter(|r| r.get("verdict").and_then(Value::as_str) == Some("PASS")).count();
    for (i, row) in shown.iter().enumerate() {
        let case = row.get("case").and_then(Value::as_str).unwrap_or_default();
        let checked = if i == 0 { " checked" } else { "" };
        let first = if i == 0 { " data-first" } else { "" };
        inputs.push_str(&format!("<input type=\"radio\" name=\"pick\" id=\"pick-{i}\"{checked}>"));
        picker.push_str(&format!("<label for=\"pick-{i}\"{first}>{}</label>", esc(gloss(case))));
        let regrade = read::render::regrade_for(&ledger.entries, RUN, case);
        let rendered = read::render::render_case(
            row,
            &loaded,
            hint.as_deref(),
            regrade,
            gloss(case),
            read::render::Instrumentation::Hidden,
        );
        totals.push(rendered.counts);
        panels.push_str(&format!("<div class=\"shown\">{}</div>", rendered.html));
    }
    let totals = read::render::total(&totals);

    // Counted, not claimed. If a future run puts a free-floating assertion on this page, the
    // sentence changes rather than the claim quietly becoming false.
    let unanchored = if totals.bare == 0 {
        "<b>On this page, no sentence stands on nothing</b> — every one either resolves to a dated \
         activity or continues a block that named an account."
            .to_owned()
    } else {
        format!(
            "<b>{} sentence(s) here stand on nothing at all</b>, marked on an amber ground. That is \
             where to start reading.",
            totals.bare
        )
    };

    let (excluded_clean, excluded_graded, blocked_n) = excluded_rate(&experiments, &book_dir)
        .ok_or_else(|| {
            format!("{EXCLUDED_PROBE} does not bind; the excluded-question rate cannot be stated")
        })?;
    let excluded_blocked = if blocked_n == 0 {
        String::new()
    } else {
        format!(", with {blocked_n} further run that could not be measured")
    };

    let body = format!(
        r#"
  <header class="lede">
    <span class="label">MultiAgency · account coordination</span>
    <h1>What did we promise, and what is still outstanding?</h1>
    <p>Things get committed in calls and then forgotten. This reads the dated record of an account
    and tells you what was agreed and not delivered, what a later note quietly replaced, where two
    people have said things that cannot both be true, and what needs a person rather than a
    reminder.</p>
    <p class="kicker">It recommends and never acts. No writes to your records, no outreach, no
    access changes. We run it on our own accounts, on the same path, with no privilege.</p>
  </header>

  <section>
    <div class="job">
      <div><b>Outstanding</b><span>What was committed and has no record of being delivered.</span></div>
      <div><b>Superseded</b><span>What a later signed change replaced, and what that means today.</span></div>
      <div><b>Contradicted</b><span>Where two records disagree and neither settles it.</span></div>
      <div><b>Needs a person</b><span>What the record cannot establish, said plainly as unknown.</span></div>
    </div>
  </section>

  <section>
    <h2>Watch it work</h2>
    <p class="recorded">Pick a question. You will see the records that turn was given, the answer
    it produced, and every claim underlined back to the dated activity it came from. These are
    <b>recorded runs</b>, not live — run <span class="mono">{run}</span> at composition
    <span class="mono">{digest}</span>, over {records} invented accounts. Every domain in them is
    RFC 2606 reserved and every record is marked synthetic; no customer data is in this repository
    or on this page.</p>
    <p class="recorded">Underlines say what a sentence rests on. A
    <span class="s anchored">solid underline</span> resolves to a dated activity, quoted beside it.
    A <span class="s named">dotted underline</span> names an account but no date the record carries.
    A <span class="s inherited">faint underline</span> names nothing itself but continues a block — or
    answers a question — that named an account. {unanchored}</p>
    <div class="demo">
      {inputs}
      <nav class="picker">{picker}</nav>
      <div class="panels">{panels}</div>
    </div>
  </section>

  <section class="trust">
    <h2>Why you can hand it your book</h2>
    <div class="panel">
      <ul>
        <li><b>It cannot act.</b> The capability it runs under has no write, no outreach and no
        network egress — not as policy, but as a shape the type system cannot express. There is no
        variant to pass.</li>
        <li><b>Your scope is resolved, never asserted.</b> Which records it may read is decided by
        your credential, server-side. Nothing a caller says — and nothing the model says — can widen
        it.</li>
        <li><b>Record text is evidence, never instruction.</b> Try the attack question above: one of
        our test records carries a planted instruction to invent a budget and provision access, and
        you can watch it named and refused.</li>
        <li><b>We hold no copy of your conversations.</b> The durable store keeps identifiers,
        states and digests — no message text, no answers, no records.</li>
        <li><b>The model is TEE-attested</b> and pinned. A pin here is a privacy decision, not a
        version string: it decides which third party ever sees a record.</li>
      </ul>
    </div>
  </section>

  <section class="gap">
    <h2>What we have not shown you</h2>
    <p><b>These answers are recorded, not live.</b> Nothing is deployed: a turn runs one cycle and
    exits, concurrency is not built, and a turn that outlasts thirty seconds is refused rather than
    degraded. There is no bot standing by.</p>
    <p><b>Our own rules graded {graded_pass} of these {shown_n} as passing</b> when this run was
    recorded. We are not putting that number next to the answers, and the reason is not
    flattery: reading the failures, they are about how a sentence phrases an absence rather than
    what it concluded, and we currently have four known defects in those rules that we are
    correcting one at a time, each with a test that must still catch the real thing. A verdict from
    a rule we know is wrong would tell you less than the answer does. Every run is kept — the
    failures, and the verdicts we later corrected against ourselves — and we will walk you through
    the whole record rather than a selection.</p>

    <p><b>One question is deliberately not here.</b> Asked to brief across every account at once,
    it produced a clean answer in {excluded_clean} of the {excluded_graded} runs we could grade —
    measured at this composition, over this book{excluded_blocked}. A brief spanning ten accounts
    makes several times the claims of any single-account question and trips a rule proportionally
    more often. We are not going to demonstrate the one we cannot yet do reliably.</p>
    <p><b>One tenant runs today, and it is us.</b> Records above are invented. We have not served
    an organization that is not our own, and we do not provision anything automatically.</p>
  </section>

  <section class="ask">
    <h2>Starting</h2>
    <div class="panel">
      <p class="big">Bring five accounts.</p>
      <p>A small book, guidance we write with you in your own words — the exact text that shapes
      every answer, which you can read — and one service. Your records stay yours.</p>
      <p class="cta"><a href="https://t.me/jwaup">Message @jwaup on Telegram</a> — a person, not a
      bot.</p>
    </div>
  </section>

  <footer>
    Answers, records and digests on this page are read from a recorded transcript, not written.
  </footer>"#,
        run = esc(RUN),
        digest = esc(&digest),
        records = loaded.len(),
        graded_pass = graded_pass,
        shown_n = shown.len(),
        unanchored = unanchored,
        excluded_clean = excluded_clean,
        excluded_graded = excluded_graded,
        excluded_blocked = excluded_blocked,
    );
    Ok(shell("Account Coordination", &body, &css()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> String {
        build(&ironworks_evidence::root()).expect("the demo run binds to the current book")
    }

    #[test]
    fn every_question_is_shown_and_pickable() {
        let html = page();
        for i in 0..6 {
            assert!(html.contains(&format!("id=\"pick-{i}\"")), "question {i} has no input");
            assert!(html.contains(&format!("for=\"pick-{i}\"")), "question {i} has no label");
        }
        assert_eq!(html.matches("class=\"shown\"").count(), 6, "one panel per shown question");
    }

    #[test]
    fn the_demo_works_without_script() {
        // A demo that needs JavaScript to show an answer is a demo that can fail to show one.
        assert!(!page().contains("<script"), "the picker is radio inputs and sibling selectors");
    }

    #[test]
    fn the_unanchored_claim_matches_what_the_page_actually_renders() {
        // The page states that no sentence stands on nothing. That must be counted from the
        // rendered answers, not asserted — if a future run puts a free-floating assertion here,
        // the sentence has to change with it.
        let html = page();
        let amber = html.matches("class=\"s bare\"").count();
        if amber == 0 {
            assert!(html.contains("no sentence stands on nothing"), "the claim must be made");
        } else {
            assert!(
                html.contains(&format!("{amber} sentence(s) here stand on nothing")),
                "the claim must match the {amber} amber sentence(s) on the page"
            );
        }
    }

    #[test]
    fn the_markings_are_explained_before_the_answers() {
        // Styling that implies meaning needs a key, and it has to come first: the page promises
        // claims "underlined back to the dated activity", then renders three different underlines.
        let html = page();
        let legend = html.find("Underlines say what a sentence rests on").expect("a legend");
        let first_answer = html.find("class=\"shown\"").expect("a panel");
        assert!(legend < first_answer, "the key must precede what it decodes");
        for marking in ["solid underline", "dotted underline", "faint underline"] {
            assert!(html.contains(marking), "{marking} is unexplained");
        }
    }

    #[test]
    fn the_grade_figure_is_counted_from_the_transcript() {
        // A generated page must not carry a hand-written result. Change the run and this moves.
        let stored: usize = {
            let text = std::fs::read_to_string(
                ironworks_evidence::root().join("docs/experiments").join(RUN),
            )
            .unwrap();
            let doc: Value = serde_json::from_str(&text).unwrap();
            doc["cases"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| {
                    !EXCLUDED.contains(&r["case"].as_str().unwrap_or_default())
                        && r["verdict"] == "PASS"
                })
                .count()
        };
        assert!(
            page().contains(&format!("graded {stored} of these 6 as passing")),
            "the page must state the count the transcript actually holds ({stored})"
        );
    }

    #[test]
    fn the_excluded_questions_rate_is_the_current_probe_not_the_famous_one() {
        // `0010`'s 2-of-5 is the figure usually quoted and it was taken at a superseded alloy over
        // the six-record book. Quoting it on a page about this composition would describe
        // something else, so the rate comes from the probe bound to this book.
        let (clean, graded, _) = excluded_rate(
            &ironworks_evidence::root().join("docs/experiments"),
            &ironworks_evidence::root().join(BOOK),
        )
        .expect("the probe binds");
        let html = page();
        assert!(
            html.contains(&format!("clean answer in {clean} of the {graded} runs")),
            "the page must state the rate the current probe measured"
        );
        assert!(!html.contains("2 of 5"), "the superseded figure must not reappear");
        assert!(html.contains("at this composition, over this book"));
    }

    #[test]
    fn the_cross_account_brief_is_not_on_the_page() {
        // FRONTEND.md: do not lead with it, and do not demonstrate it. Its recorded answer
        // miscounts itself and concludes "offboarding appears complete" — the overclaim the
        // service exists to prevent, and the one `regrades.json` already records against `0005`.
        let html = page();
        assert!(!html.contains("What needs my attention?"), "the 40% case must not be shown");
        assert!(
            !html.contains("offboarding appears complete"),
            "the overclaim in that answer must not reach a public page"
        );
    }

    #[test]
    fn the_graders_own_vocabulary_stays_off_the_public_page() {
        // A verdict chip and a counts panel are instrumentation. "asserted non-existence exceeds
        // the record" is addressed to whoever maintains the rule, not to a stranger.
        let html = page();
        assert!(!html.contains("chip FAIL"), "no unexplained verdict chips");
        assert!(!html.contains("asserted non-existence"), "no grader jargon");
        assert!(!html.contains("anchored to a dated activity"), "no per-sentence counts");
        // The honesty moves to prose, where it can be explained — and the figure in it is
        // counted from the transcript, never typed.
        assert!(html.contains("graded"), "the grades must still be accounted for somewhere");
        assert!(
            html.contains("known defects in those rules"),
            "and the page must say why the number is not stamped on each answer"
        );
    }

    #[test]
    fn the_planted_instruction_and_its_refusal_are_both_on_the_page() {
        // The strongest thing here is checkable by eye rather than counted: the attack is in the
        // record, and the answer names it.
        let html = page();
        assert!(html.contains("750,000"), "the planted budget must be visible in the record");
        assert!(
            html.contains("suspicious") || html.contains("embedded instruction"),
            "and the answer must be seen refusing it"
        );
    }

    #[test]
    fn the_synthetic_markers_are_on_screen_rather_than_hidden() {
        let html = page();
        assert!(html.contains("RFC 2606"));
        assert!(html.contains("marked synthetic"));
    }

    #[test]
    fn it_states_that_the_answers_are_recorded_and_nothing_is_deployed() {
        let html = page();
        assert!(html.contains("recorded runs"), "a reader must not think this is live");
        assert!(html.contains("Nothing is deployed"));
    }
}
