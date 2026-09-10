//! `ironworks-claims` — verify every written claim against what the tree measures.
//!
//! Most checks are also `#[test]`s and run with the suite. **Two cannot be**, and both are here
//! for the same reason: they need a subprocess. `commands` asks the built binary about its own
//! flags; `toolchain` shells out to cargo.
//!
//! # The count check, and why its replacement is a different shape
//!
//! A check once read `DESIGN.md`'s `cargo test --workspace N passed` line and ran the suite to
//! compare. It caught real drift twice, and it was still right to delete: a test total moves every
//! time anyone adds a test, so it generated churn rather than signal, and the status block now says
//! `passing` and asserts no total.
//!
//! **What went with it was more than the number.** Nothing was left checking that the tree builds
//! at all — and with one crate failing to compile against a mid-refactor API, this binary reported
//! every claim holding. A checker that goes green on a tree that does not compile is worse than no
//! checker, because its green is the thing a reader trusts.
//!
//! `toolchain` restores the control without the churn, and the difference is the shape of the
//! claim: `passing` and `clean` are **binary**. They cannot drift by one when somebody writes a
//! test; they can only be true or false. Its reader deliberately does *not* accept a count-shaped
//! line as `passing`, so the old form cannot come back by accident.

use ironworks_claims::{CHECKS, Verdict, root, self_description};

/// `--json <path>`, or `None`. Deliberately hand-parsed: one optional flag does not justify a
/// dependency in the one crate whose independence from this workspace is the point.
fn json_destination() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--json" {
            return args.next();
        }
    }
    None
}

fn main() -> std::process::ExitCode {
    let root = root();
    // `CHECKS` rather than a list here, so the suite and this binary cannot run different sets.
    // The last two are the slow ones: they shell out to a subprocess, `toolchain` twice.
    let mut findings = Vec::new();
    for check in CHECKS {
        findings.extend(check.findings(&root));
    }
    // **Last, and it has to be.** Its input is this binary's own output rather than the tree, so
    // every check must have reported before it can ask whether the prose names them all.
    let names: Vec<&str> = CHECKS.iter().map(|c| c.name).collect();
    let described = self_description::check(&root, &names, &findings);
    findings.extend(described);

    println!("measured claims vs what the tree says\n");
    for f in &findings {
        println!(
            "  {:<8} {:<42} {}",
            f.verdict.as_str().to_lowercase().replace("pass", "ok"),
            f.subject,
            f.detail
        );
    }

    // `--json <path>` emits the same findings a machine can read.
    //
    // **Emitting, never recording.** This crate's layering row says it depends on nothing inside
    // the workspace, and that is load-bearing rather than incidental: the thing that verifies the
    // tree must not be entangled with what it verifies, or a change to a crate could break the
    // control that checks that crate. So the findings leave as JSON and something else decides
    // what to do with them — `ironworks-evidence`'s recorder is the first such reader.
    if let Some(path) = json_destination() {
        let rows: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    // `claim` is the identity and `subject` is where to look. They are usually the
                    // same string and are emitted separately anyway, because for the tally rows
                    // they are not — and a reader that guessed which was which would be guessing.
                    "claim": f.claim,
                    "subject": f.subject,
                    "verdict": f.verdict.as_str(),
                    "detail": f.detail,
                })
            })
            .collect();
        let doc = serde_json::json!({ "findings": rows });
        if let Err(why) = std::fs::write(&path, format!("{doc:#}\n")) {
            eprintln!("ironworks-claims: cannot write {path}: {why}");
            return std::process::ExitCode::from(2);
        }
    }

    let fails = findings.iter().filter(|f| f.verdict == Verdict::Fail).count();
    let blocked = findings.iter().filter(|f| f.verdict == Verdict::Blocked).count();
    let held = findings.len() - fails - blocked;
    println!(
        "\n  {held}/{} claims hold · {fails} disagree · {blocked} unverifiable",
        findings.len()
    );

    // BLOCKED outranks nothing and is never a pass, but it is also not a disagreement: exit 3
    // says "could not measure", exit 2 says "measured, and it is wrong".
    if fails > 0 {
        std::process::ExitCode::from(2)
    } else if blocked > 0 {
        std::process::ExitCode::from(3)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
