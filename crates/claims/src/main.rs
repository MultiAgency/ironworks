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

use ironworks_claims::{Verdict, commands, members, paths, root, tallies, toolchain};

fn main() -> std::process::ExitCode {
    let root = root();
    let mut findings = tallies::check(&root);
    findings.extend(commands::check(&root));
    findings.extend(paths::check(&root));
    findings.extend(paths::check_anchors(&root));
    findings.extend(paths::check_symbols(&root));
    findings.extend(paths::check_experiments(&root));
    findings.extend(paths::check_experiment_numbers(&root));
    findings.extend(paths::check_bare_docs(&root));
    findings.extend(members::check(&root));
    // Last, because it is the slow one: it shells out to cargo twice.
    findings.extend(toolchain::check(&root));

    println!("measured claims vs what the tree says\n");
    for f in &findings {
        println!(
            "  {:<8} {:<42} {}",
            f.verdict.as_str().to_lowercase().replace("pass", "ok"),
            f.subject,
            f.detail
        );
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
