//! `ironworks-eval` — grade a service's answers against its evidence suite.
//!
//! ```text
//! dev/probe-up.sh            # runtime + disposable record store
//! ironworks-eval             # this
//! ```
//!
//! # What replaced what
//!
//! This is the Rust successor to `dev/eval.py`. Two things did **not** come across, and both
//! were deletions rather than ports:
//!
//! - **The `account-analysis` suite.** It existed to measure parity against the reference
//!   implementation's own graders, imported from `~/agency/ironworks-py` rather than vendored so a
//!   copy could not drift into grading Rust output more kindly. Parity is no longer a goal, so
//!   the reason to run another language in the measuring path went with it. `account-analysis`
//!   now declares `evaluation: null` and carries no answer-quality claim — see
//!   `services/README.md`.
//! - **`--self-test`.** The graders' controls are `#[test]`s in `crates/graders` and run under
//!   `cargo test --workspace`, so they cannot be skipped by forgetting a flag.
//!
//! # One asymmetry that must not be silently absorbed
//!
//! Every turn here is one-shot with a fresh journal per case. `ironworks coordinate` has no
//! continuation, so this is `--isolate` semantics always, with no option to be otherwise. A
//! comparison against any harness that shares a thread across cases would be scoring this against
//! a run that can answer partly from history — a different and easier question.

mod finding;
mod outcome;
mod regrade;

use ironworks_graders::cases;
use outcome::{classify, needs_journal_recovery};
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "usage: ironworks-eval [--case <id>] [--runs <n>] [--json <path>]\n\
     \x20      [--records-url <url>] [--records-token <token>] [--binary <path>]\n\n\
     \x20  or: ironworks-eval --regrade <transcript> [--regrade <transcript> ...]\n\
     \x20      --recorded-grader <id> --json <path>\n\n\
     \x20  needs a runtime and a record store: dev/probe-up.sh, then its printed exports\n\
     \x20  regrade is offline and preserves the verdict each transcript recorded\n\
     \x20  the graders' own controls are `cargo test -p ironworks-graders`, not a flag here\n";

/// The dogfooded suite. It is the only one with graders in this repository.
const SLUG: &str = "coordination-eval";
const SERVICE: &str = "relationship-intelligence";
const GUIDANCE: &str = "fixtures/account-coordination/guidance.md";
const BOOK: &str = "fixtures/account-coordination/book";
const EXPECT_RECORDS: usize = 10;

struct Args {
    case: Option<String>,
    runs: usize,
    json: Option<PathBuf>,
    regrade: Vec<PathBuf>,
    recorded_grader: Option<String>,
    records_url: String,
    records_token: String,
    binary: PathBuf,
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("ironworks-eval: {message}");
            std::process::ExitCode::from(3)
        }
    }
}

fn run() -> Result<std::process::ExitCode, String> {
    let args = parse()?;
    let root = repo_root();

    // A transcript is evidence and is never replaced. Refusing here rather than at the end means
    // a run that cannot be recorded costs nothing instead of a full suite of billed turns.
    if let Some(path) = &args.json
        && path.exists()
    {
        return Err(format!(
            "{} already exists. A transcript is evidence and is never replaced — choose a new name.",
            path.display()
        ));
    }

    if !args.regrade.is_empty() {
        if args.case.is_some() || args.runs != 1 {
            return Err("--regrade cannot be combined with --case or --runs".to_owned());
        }
        let output = args.json.as_deref().ok_or_else(|| {
            "--regrade requires --json so its provenance cannot be discarded".to_owned()
        })?;
        return regrade::run(&root, &args.regrade, args.recorded_grader.as_deref(), output);
    }

    let book = ironworks_evidence::fingerprint::book(root.join(BOOK))
        .map_err(|e| format!("cannot read the book at {BOOK}: {e}"))?
        .ok_or_else(|| format!("no book at {BOOK}"))?;
    let ingredients = compose(&root)?;

    println!("eval: account-coordination graders from crates/graders");
    println!(
        "eval: composing {SERVICE} — alloy {} ({}), guidance {}",
        ingredients.alloy_bytes, ingredients.alloy, ingredients.guidance
    );
    println!(
        "eval: {} case(s) x {} run(s), fresh journal each, records {}",
        selected(&args).count(),
        args.runs,
        args.records_url
    );
    println!();

    let mut rows = Vec::new();
    let (mut pass, mut fail, mut review, mut blocked) = (0, 0, 0, 0);
    let mut digests = std::collections::BTreeSet::new();

    for run_index in 1..=args.runs {
        for case in selected(&args) {
            let journal = std::env::temp_dir().join(format!(
                "ironworks-eval-{}-{}-{run_index}.db",
                std::process::id(),
                case.id
            ));
            let _ = std::fs::remove_file(&journal);
            let turn = turn(&args, &root, case.question, &journal)?;
            let _ = std::fs::remove_file(&journal);

            // `finding` is the typed half and `verdict` is what a transcript has always spelled.
            // They are produced together so the two can never disagree, and the counters below read
            // the type rather than the string.
            let (verdict, why, composition, finding) =
                match classify(&turn.stderr, turn.code, &turn.stdout, EXPECT_RECORDS) {
                    Ok(t) => {
                        digests.insert(t.digest.clone());
                        let graded = case.grade(&turn.stdout);
                        let finding = finding::from_verdict(graded.verdict);
                        (graded.verdict.as_str().to_owned(), graded.why, Some(t), finding)
                    }
                    // Nothing was observed. Deliberately not reachable from a grader — see
                    // `finding::no_verdict_can_produce_unobserved`.
                    Err(b) => (
                        "BLOCKED".to_owned(),
                        b.0,
                        outcome::telemetry(&turn.stderr),
                        ironworks_domain::Finding::Unobserved,
                    ),
                };

            // Exhaustive on the type, where the string match this replaced ended in `_ =>
            // blocked` — so a verdict spelling nobody had counted was silently tallied as an
            // unmeasured run.
            match finding {
                ironworks_domain::Finding::Upheld => pass += 1,
                ironworks_domain::Finding::Refuted => fail += 1,
                ironworks_domain::Finding::Indeterminate => review += 1,
                ironworks_domain::Finding::Unobserved => blocked += 1,
            }
            println!("  {:<8} {:<20} {why}", verdict, case.id);

            rows.push(serde_json::json!({
                "run": run_index,
                "case": case.id,
                "verdict": verdict,
                "finding": finding.wire(),
                "why": why,
                "question": case.question,
                "expect": case.expect,
                "answer": turn.stdout,
                "composition": composition.map(|t| serde_json::json!({
                    "alloy": t.alloy_bytes,
                    "digest": t.digest,
                    "envelope": t.envelope_bytes,
                    "records": t.records,
                    "state": t.state,
                })),
            }));
        }
    }

    let attribution = attribution(&digests, &ingredients.alloy);

    println!("\n{}", "-".repeat(74));
    println!("  {pass}/{} pass · {fail} fail · {review} review · {blocked} blocked", rows.len());
    println!(
        "  alloy digest {} — this result describes that composition and no other",
        ingredients.alloy
    );
    println!("  book {} ({} records) · attribution {attribution}", book.fingerprint, book.records);
    println!("  scope: one-shot turns, fresh journal per case. No continuation is built, so this");
    println!("         is --isolate semantics always.");

    if let Some(path) = &args.json {
        let transcript = serde_json::json!({
            "suite": "account-coordination",
            "attribution": attribution,
            "book": {
                "records": book.records,
                "fingerprint": book.fingerprint,
                "path": BOOK,
            },
            "ingredients": {
                "service": ingredients.service,
                "alloy": ingredients.alloy,
                "alloy_bytes": ingredients.alloy_bytes,
                "guidance": ingredients.guidance,
                "scrap": ingredients.scrap,
                "parts": ingredients.parts,
            },
            "grader": regrade::current_provenance(),
            "cases": rows,
        });
        std::fs::write(path, serde_json::to_string_pretty(&transcript).map_err(|e| e.to_string())?)
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        println!("  transcript written to {}", path.display());
    }

    Ok(std::process::ExitCode::from(exit_code(fail, blocked)))
}

/// Whether a transcript's verdicts can be attributed to the composition recorded beside them.
///
/// A transcript carries one alloy digest and many answers. If the answers were not all produced by
/// that composition, the digest names none of them and the verdicts describe two different things
/// — so this is a claim about the evidence, not a summary of it, and the three values are not
/// ranked. `unreported` is the BLOCKED shape: no run reported a digest, so nothing was compared.
/// `disputed` is the one that must never be read as a weaker `bound`; it means the suite ran
/// against more than one composition, or against one that is not the one being reported.
///
/// Its own function so the rule is testable. Inline it was five lines in the middle of a
/// 140-line `run`, reachable only by billing a full suite of model turns.
fn attribution(digests: &std::collections::BTreeSet<String>, alloy: &str) -> &'static str {
    match digests.len() {
        0 => "unreported",
        1 if digests.contains(alloy) => "bound",
        _ => "disputed",
    }
}

/// The three-valued exit contract: 0 clean, 2 a measured failure, 3 an unmeasured run.
///
/// **BLOCKED outranks FAIL** (§F-14). A run that could not measure everything must not report the
/// code for "measured, and something failed" — the two are different facts and an operator acts
/// on them differently. Its own function so the rule is testable; it used to be inline, and the
/// test named for it asserted `3 != 2` on two integer literals, which is true whatever this does.
fn exit_code(fail: usize, blocked: usize) -> u8 {
    if blocked > 0 { 3 } else { u8::from(fail > 0) * 2 }
}

fn selected(args: &Args) -> impl Iterator<Item = &'static cases::Case> + '_ {
    cases::CASES.iter().filter(move |c| args.case.as_deref().is_none_or(|want| want == c.id))
}

struct Turn {
    stdout: String,
    stderr: String,
    code: i32,
}

/// One case, one fresh journal.
///
/// **The retry loop is recovery, not a retry.** The runtime gives up waiting at ~30s but the turn
/// still ran; re-invoking against the *same* journal adopts the unresolved turn and replays one
/// logical turn rather than billing a second execution. A fresh journal here would mint a new
/// idempotency key, which is the forbidden thing.
fn turn(args: &Args, root: &Path, question: &str, journal: &Path) -> Result<Turn, String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let out = Command::new(&args.binary)
            .current_dir(root)
            .args(["coordinate", "--slug", SLUG, "--guidance", GUIDANCE])
            .arg("--journal")
            .arg(journal)
            .args(["--ask", question])
            .env("IRONWORKS_RECORDS", &args.records_url)
            .env("IRONWORKS_RECORDS_TOKEN", &args.records_token)
            .output()
            .map_err(|e| format!("running {}: {e}", args.binary.display()))?;
        let turn = Turn {
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code().unwrap_or(-1),
        };
        if attempt >= 4 || !needs_journal_recovery(&turn.stderr) {
            return Ok(turn);
        }
    }
}

struct Ingredients {
    service: String,
    alloy: String,
    alloy_bytes: usize,
    guidance: String,
    scrap: String,
    parts: Vec<(String, String, String)>,
}

/// What this tree composes for the suite — a dry melt, no turn, no model.
///
/// Recorded with the transcript because a verdict is bound to an alloy digest, and the digest
/// alone cannot say what moved under it. It only works recorded *before* the answers, never
/// reconstructed afterwards.
///
/// # In process, with the same functions the binary composes with
///
/// This used to run `target/debug/ironworks compose` and recover the figures from its terminal
/// output by field position — `split_whitespace`, `parse().unwrap_or(0)`, and a `_ => {}` that
/// swallowed anything unmatched. Only the alloy digest was guarded, so a reworded line would have
/// written `0` bytes, an empty scrap digest and an empty parts list into the transcript, silently
/// — and `crates/site/src/demo/mod.rs::build` publishes those figures on the page.
///
/// `crates/site/src/arc/data.rs::compose` was moved off that technique for exactly this reason.
/// The fix had landed in the reader and not in the writer. Reading `target/debug` had a second
/// mode too: a binary older than the tree reported a composition that no longer existed, and the
/// suite required someone to have run `cargo build` first for its figures to mean anything.
fn compose(root: &Path) -> Result<Ingredients, String> {
    let definition = ironworks_services::service::load(SERVICE).map_err(|e| e.to_string())?;
    let guidance_path = root.join(GUIDANCE);
    let guidance_text = std::fs::read_to_string(&guidance_path)
        .map_err(|e| format!("cannot read {}: {e}", guidance_path.display()))?;
    // Parsed, not merely read, for the reason `ironworks compose` parses it: an alloy fingerprint
    // for guidance bound to another tenant or another service describes a composition that would
    // never have served.
    let guidance = ironworks_domain::Guidance::parse(&guidance_text, SLUG, SERVICE)
        .map_err(|e| e.to_string())?;

    let composition = definition.composition();
    let alloy = ironworks_domain::melt(composition, &guidance, &ironworks_services::Heap)
        .map_err(|e| e.to_string())?;
    let parts = composition.part_digests(&ironworks_services::Heap).map_err(|e| e.to_string())?;

    Ok(Ingredients {
        service: definition.service_id().to_string(),
        alloy: alloy.digest(),
        alloy_bytes: alloy.len(),
        guidance: guidance.digest(),
        scrap: ironworks_services::digest(),
        // The melt order, tail last, with each part's role named the way `ironworks compose`
        // names it — the same rows the parsed output carried.
        parts: parts
            .into_iter()
            .map(|(part, digest)| {
                let role =
                    if &part == composition.safety_tail() { "tail" } else { "part" }.to_owned();
                (part.to_string(), digest, role)
            })
            .collect(),
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| ".".into())
}

fn parse() -> Result<Args, String> {
    let mut args = Args {
        case: None,
        runs: 1,
        json: None,
        regrade: Vec::new(),
        recorded_grader: None,
        records_url: std::env::var("IRONWORKS_RECORDS").unwrap_or_else(|_| {
            let port = std::env::var("PROBE_STORE_PORT").unwrap_or_else(|_| "3996".into());
            format!("http://127.0.0.1:{port}")
        }),
        records_token: "dev-token-coordination".to_owned(),
        binary: repo_root().join("target/debug/ironworks"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"));
        match flag.as_str() {
            "--case" => args.case = Some(value()?),
            "--runs" => {
                args.runs =
                    value()?.parse().map_err(|_| format!("--runs needs a number\n\n{USAGE}"))?;
            }
            "--json" => args.json = Some(PathBuf::from(value()?)),
            "--regrade" => args.regrade.push(PathBuf::from(value()?)),
            "--recorded-grader" => args.recorded_grader = Some(value()?),
            "--records-url" => args.records_url = value()?,
            "--records-token" => args.records_token = value()?,
            "--binary" => args.binary = PathBuf::from(value()?),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}\n\n{USAGE}")),
        }
    }
    if args.runs == 0 {
        return Err(format!("--runs must be at least 1\n\n{USAGE}"));
    }
    if let Some(want) = &args.case
        && cases::case(want).is_none()
    {
        let known: Vec<&str> = cases::CASES.iter().map(|c| c.id).collect();
        return Err(format!("no case {want:?}; known: {}", known.join(", ")));
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_addressable_by_id() {
        // `--case` names must stay in step with the suite, or a typo silently runs everything.
        for case in cases::CASES {
            assert!(cases::case(case.id).is_some(), "{} is not addressable", case.id);
        }
    }

    #[test]
    fn the_suite_is_the_seven_coordination_cases() {
        assert_eq!(cases::CASES.len(), 7);
    }

    fn digests(of: &[&str]) -> std::collections::BTreeSet<String> {
        of.iter().map(|d| (*d).to_owned()).collect()
    }

    /// The claim is about *evidence*, so each of the three values needs the case that produces it
    /// and a case that must not.
    #[test]
    fn a_transcript_is_bound_only_to_the_one_composition_that_produced_every_answer() {
        const ALLOY: &str = "aaaaaaaaaaaaaaaa";
        const OTHER: &str = "bbbbbbbbbbbbbbbb";

        // The only way to be bound: every run reported the digest being reported.
        assert_eq!(attribution(&digests(&[ALLOY]), ALLOY), "bound");

        // Nothing was compared. Distinct from `disputed`, which is a disagreement that happened.
        assert_eq!(attribution(&digests(&[]), ALLOY), "unreported");

        // One digest, and it is not this one — the suite measured a composition that is not the
        // one the transcript names. This is the case a check that only counted would call bound.
        assert_eq!(attribution(&digests(&[OTHER]), ALLOY), "disputed");

        // Two, one of which matches: still disputed. Half the answers came from somewhere else.
        assert_eq!(attribution(&digests(&[ALLOY, OTHER]), ALLOY), "disputed");
    }

    #[test]
    fn blocked_outranks_a_failure_in_the_exit_code() {
        // An unmeasured run must not read as a measured pass, and must be distinguishable from a
        // measured failure — §F-14.
        assert_eq!(exit_code(0, 0), 0, "a clean run");
        assert_eq!(exit_code(3, 0), 2, "measured, and something failed");
        assert_eq!(exit_code(0, 1), 3, "nothing failed, but the run was not complete");
        // The ranking itself: one blocked check outranks any number of failures, because
        // "some of this was never measured" is the fact an operator has to see first.
        assert_eq!(exit_code(9, 1), 3, "a failure masked the fact that the run was incomplete");
        // And BLOCKED is never a pass, whatever else happened.
        for fail in 0..4 {
            assert_ne!(exit_code(fail, 1), 0, "a blocked run reported success");
        }
    }
}
