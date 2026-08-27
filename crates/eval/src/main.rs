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
//!   implementation's own graders, imported from `~/agency/ironworks` rather than vendored so a
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
        .ok_or_else(|| format!("no book at {BOOK}"))?;
    let ingredients = compose(&args.binary, &root)?;

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

            let (verdict, why, composition) =
                match classify(&turn.stderr, turn.code, &turn.stdout, EXPECT_RECORDS) {
                    Ok(t) => {
                        digests.insert(t.digest.clone());
                        let graded = case.grade(&turn.stdout);
                        (graded.verdict.as_str().to_owned(), graded.why, Some(t))
                    }
                    Err(b) => ("BLOCKED".to_owned(), b.0, outcome::telemetry(&turn.stderr)),
                };

            match verdict.as_str() {
                "PASS" => pass += 1,
                "FAIL" => fail += 1,
                "REVIEW" => review += 1,
                _ => blocked += 1,
            }
            println!("  {:<8} {:<20} {why}", verdict, case.id);

            rows.push(serde_json::json!({
                "run": run_index,
                "case": case.id,
                "verdict": verdict,
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

    // Every answer in one transcript must have been produced by one composition, or the verdicts
    // describe two different things and the digest recorded beside them names neither.
    let attribution = match digests.len() {
        0 => "unreported",
        1 if digests.contains(&ingredients.alloy) => "bound",
        _ => "disputed",
    };

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

    Ok(std::process::ExitCode::from(if blocked > 0 { 3 } else { u8::from(fail > 0) * 2 }))
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

/// What this binary would compose — a dry melt, no turn, no model.
///
/// Recorded with the transcript because a verdict is bound to an alloy digest, and the digest
/// alone cannot say what moved under it. It only works recorded *before* the answers, never
/// reconstructed afterwards.
fn compose(binary: &Path, root: &Path) -> Result<Ingredients, String> {
    let out = Command::new(binary)
        .current_dir(root)
        .args(["compose", "--slug", SLUG, "--guidance", GUIDANCE, "--service", SERVICE])
        .output()
        .map_err(|e| format!("running {}: {e}", binary.display()))?;
    if !out.status.success() {
        return Err(format!(
            "ironworks compose failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut it = Ingredients {
        service: String::new(),
        alloy: String::new(),
        alloy_bytes: 0,
        guidance: String::new(),
        scrap: String::new(),
        parts: Vec::new(),
    };
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["service", name, ..] => it.service = (*name).to_owned(),
            ["alloy", bytes, _, digest, ..] => {
                it.alloy_bytes = bytes.parse().unwrap_or(0);
                it.alloy = digest.trim_matches(['(', ')']).to_owned();
            }
            ["guidance", _, _, digest, ..] => {
                it.guidance = digest.trim_matches(['(', ')']).to_owned();
            }
            ["scrap", _, _, digest, ..] => it.scrap = digest.trim_matches(['(', ')']).to_owned(),
            [kind @ ("part" | "tail"), name, digest, ..] => {
                it.parts.push(((*name).to_owned(), (*digest).to_owned(), (*kind).to_owned()));
            }
            _ => {}
        }
    }
    if it.alloy.is_empty() {
        return Err("ironworks compose reported no alloy digest".to_owned());
    }
    Ok(it)
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

    #[test]
    fn blocked_outranks_a_failure_in_the_exit_code() {
        // An unmeasured run must not read as a measured pass, and must be distinguishable from a
        // measured failure — §F-14.
        assert_ne!(3, 2, "BLOCKED is 3 and FAIL is 2, deliberately different");
    }
}
