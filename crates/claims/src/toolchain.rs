//! The status-block claims that are about the tree's state, not about a number.
//!
//! # Why this came back after a count check was deliberately removed
//!
//! A sixth check once read `DESIGN.md`'s `cargo test --workspace N passed` line and compared it to
//! a run. It was removed with its claim, and rightly: a test total moves every time anyone adds a
//! test, so it generated churn rather than signal — five edits in one working day, each of them
//! noise.
//!
//! What went with it was not just the number. **Nothing was left checking that the tree builds at
//! all**, and the gap showed: with `cli` failing to compile against a mid-refactor API,
//! `ironworks-claims` reported every claim holding. A checker that goes green on a tree that does
//! not compile is worse than no checker, because its green is the thing a reader trusts.
//!
//! The distinction that makes this safe to have back: **`passing` and `clean` are binary, not
//! counts.** They cannot drift by one when someone writes a test. They can only be true or false,
//! which is the shape a claim has to have to be worth checking.
//!
//! # Why this is not a `#[test]`
//!
//! `cargo test --workspace` from inside a test invokes the harness that is running it. The same
//! reason `commands` lives here: this needs a subprocess, so it runs from the binary and not from
//! the suite.

use crate::Finding;
use std::path::Path;
use std::process::Command;

/// What the status block says about the toolchain, as written.
///
/// Read rather than assumed, because the failure mode this crate exists to prevent is prose and
/// tree disagreeing. **A claim the block stops making is BLOCKED, not retired** — [`unclaimed`]
/// says what retiring one silently cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub tests_pass: bool,
    pub clippy_clean: bool,
    pub fmt_clean: bool,
    pub docs_clean: bool,
}

pub fn claimed(design: &str) -> Claimed {
    let has = |needle: &str, word: &str| {
        design.lines().any(|l| {
            let l = l.trim();
            l.starts_with(needle) && l.contains(word)
        })
    };
    Claimed {
        tests_pass: has("cargo test --workspace", "passing"),
        clippy_clean: has("cargo clippy", "clean"),
        fmt_clean: has("cargo fmt", "clean"),
        docs_clean: has("cargo doc", "clean"),
    }
}

/// A BLOCKED finding for every status claim `DESIGN.md` does not make.
///
/// **One per missing claim.** The guard this replaced fired only when all four were gone, so
/// rewording the `cargo test --workspace … passing` line alone retired the one check that
/// establishes the tree compiles — no finding, exit 0, and the tally still saying every claim held
/// over a smaller denominator. A claim nobody makes is not a claim that is wrong, so each is BLOCKED
/// under the name it would have been measured under: the measurement did not happen.
pub fn unclaimed(claimed: Claimed) -> Vec<Finding> {
    [
        (claimed.tests_pass, "workspace tests", "`cargo test --workspace … passing`"),
        (claimed.clippy_clean, "clippy", "`cargo clippy … clean`"),
        (claimed.fmt_clean, "rustfmt", "`cargo fmt … clean`"),
        (claimed.docs_clean, "rustdoc", "`cargo doc … clean`"),
    ]
    .into_iter()
    .filter(|(made, ..)| !made)
    .map(|(_, subject, claim)| {
        Finding::blocked(
            subject,
            format!("DESIGN.md's status block does not state {claim}, so nothing measured it"),
        )
    })
    .collect()
}

/// Run one cargo invocation. `None` is clean; `Some(evidence)` is a failure that kept its reason.
///
/// `--quiet` because the output is not the measurement; the exit status is. A non-zero exit from
/// either of these is unambiguous — there is no partial credit for a tree that half-builds.
///
/// **A failure keeps its evidence, and that is not decoration.** The first version discarded the
/// child's output entirely. This check then reported `workspace tests` red twice in about twenty
/// invocations while `cargo test --workspace` passed on every direct run before and after — and
/// there was nothing left to say why, because the only process that saw the failure threw the
/// output away. Root cause **unestablished**: it reproduces only under `cargo run` and not from
/// the built binary, and an inherited jobserver was tested and ruled out (it warns, it does not
/// fail). §K-8 is the reason this matters more than it looks: a spurious red gets *investigated*,
/// spending exactly the attention the harness exists to save, and a red nobody can substantiate
/// trains its reader to skim the ones that are real. So the next occurrence arrives with the last
/// lines the child actually printed.
fn cargo_ok(root: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<Option<String>, String> {
    let out = Command::new(env!("CARGO"))
        .current_dir(root)
        .envs(env.iter().copied())
        .args(args)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if out.status.success() {
        return Ok(None);
    }
    // stderr first, because that is where `test` and `clippy` put a failure. `fmt --check` puts its
    // whole diff on **stdout** and leaves stderr empty, so a stderr-only read reported the failure
    // with no evidence at all — the precise thing the paragraph above says not to ship. Falling
    // back rather than branching on the subcommand keeps the next check from rediscovering this.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let evidence = if stderr.trim().is_empty() {
        summarize_fmt(&String::from_utf8_lossy(&out.stdout))
    } else {
        tail_of(&stderr)
    };
    Ok(Some(evidence))
}

fn tail_of(text: &str) -> String {
    let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" · ")
}

/// `fmt --check` emits a unified diff per hunk, which is far too long to put in a finding. What a
/// reader needs is which files and how many, so the diff is counted rather than quoted.
fn summarize_fmt(stdout: &str) -> String {
    // `Diff in <path>:<line>:` — the line must come off, or one file with eleven hunks reports as
    // eleven files and the count says the opposite of what it means.
    let files: std::collections::BTreeSet<&str> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("Diff in "))
        .map(|l| {
            let l = l.trim_end_matches(':');
            match l.rsplit_once(':') {
                Some((path, line))
                    if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    path
                }
                _ => l,
            }
        })
        .collect();
    if files.is_empty() {
        return tail_of(stdout);
    }
    let named: Vec<&str> = files.iter().take(3).copied().collect();
    format!(
        "{} file(s) need formatting: {}{}",
        files.len(),
        named.join(", "),
        if files.len() > named.len() { ", …" } else { "" }
    )
}

pub fn check(root: &Path) -> Vec<Finding> {
    let design = match std::fs::read_to_string(root.join("DESIGN.md")) {
        Ok(t) => t,
        Err(e) => {
            return vec![Finding::blocked("DESIGN.md", format!("could not be read: {e}"))];
        }
    };
    let claimed = claimed(&design);

    // A reader that reads nothing passes everything, and here that is the worst version of it:
    // every arm below is skipped for a claim the status block does not make, so rewording one line
    // would silently retire the check that establishes the tree compiles at all — the gap this
    // module's header says it came back to close. `unclaimed` reports each such claim BLOCKED.
    let mut out = unclaimed(claimed);

    if claimed.tests_pass {
        out.push(match cargo_ok(root, &[], &["test", "--workspace", "--quiet"]) {
            Ok(None) => Finding::pass("workspace tests", "`cargo test --workspace` passes"),
            Ok(Some(evidence)) => Finding::fail(
                "workspace tests",
                format!(
                    "DESIGN.md claims `passing`; `cargo test --workspace` does not — {evidence}"
                ),
            ),
            Err(why) => Finding::blocked("workspace tests", why),
        });
    }

    if claimed.clippy_clean {
        out.push(
            match cargo_ok(root, &[], &["clippy", "--workspace", "--all-targets", "--quiet"]) {
                Ok(None) => Finding::pass("clippy", "`cargo clippy --all-targets` is clean"),
                Ok(Some(evidence)) => Finding::fail(
                    "clippy",
                    format!(
                        "DESIGN.md claims `clean`; `cargo clippy --all-targets` is not — {evidence}"
                    ),
                ),
                Err(why) => Finding::blocked("clippy", why),
            },
        );
    }

    // Formatting is the third binary claim and the newest. It is checkable at all only because
    // `rustfmt.toml` now declares the house style: rustfmt's default sub-width heuristics disagreed
    // with a 100-column codebase in 1332 places, so `cargo fmt --check` was a wall of noise nobody
    // could read a regression out of. `--check` writes nothing; a non-zero exit is the whole
    // measurement.
    if claimed.fmt_clean {
        out.push(match cargo_ok(root, &[], &["fmt", "--check"]) {
            Ok(None) => Finding::pass("rustfmt", "`cargo fmt --check` is clean"),
            Ok(Some(evidence)) => Finding::fail(
                "rustfmt",
                format!("DESIGN.md claims `clean`; `cargo fmt --check` is not — {evidence}"),
            ),
            Err(why) => Finding::blocked("rustfmt", why),
        });
    }

    if claimed.docs_clean {
        // `--no-deps`, so this is *this* workspace's doc comments and not every dependency's.
        // rustdoc is the only thing that evaluates `rustdoc::broken_intra_doc_links`, denied in
        // `Cargo.toml` — neither `cargo test` nor `cargo clippy` invokes it — so without this arm
        // that denial would be inert.
        //
        // **And `-D warnings`, because the claim is `clean`.** Exit status alone passed this arm
        // over twelve warnings — public docs linking private items, an unclosed HTML tag, a
        // redundant link target — so the word had come to mean "builds". Set here rather than
        // for every invocation: `cargo test` hands `RUSTDOCFLAGS` to each doctest as well, and a
        // warning inside an example is not what this arm claims anything about.
        let deny = [("RUSTDOCFLAGS", "-D warnings")];
        out.push(match cargo_ok(root, &deny, &["doc", "--workspace", "--no-deps", "--quiet"]) {
            Ok(None) => Finding::pass("rustdoc", "`cargo doc --workspace --no-deps` is clean"),
            Ok(Some(evidence)) => Finding::fail(
                "rustdoc",
                format!("DESIGN.md claims `clean`; `cargo doc` is not — {evidence}"),
            ),
            Err(why) => Finding::blocked("rustdoc", why),
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader is two-sided: it must find the claims that are there and not invent ones that
    /// are not. Without the second half, deleting a claim from `DESIGN.md` would leave this
    /// checking a sentence nobody makes.
    #[test]
    fn the_status_block_is_read_the_way_it_is_written() {
        let design = "```\ncargo test --workspace       passing\n\
             cargo clippy --all-targets   clean\ncargo fmt --check            clean\n\
             cargo doc --no-deps          clean\n```";
        assert_eq!(
            claimed(design),
            Claimed { tests_pass: true, clippy_clean: true, fmt_clean: true, docs_clean: true }
        );

        assert_eq!(
            claimed("nothing here at all"),
            Claimed { tests_pass: false, clippy_clean: false, fmt_clean: false, docs_clean: false },
            "a claim nobody makes must not be checked"
        );

        // Each claim is read independently: a block making one must not turn the others on.
        assert_eq!(
            claimed("cargo clippy --all-targets   clean"),
            Claimed { tests_pass: false, clippy_clean: true, fmt_clean: false, docs_clean: false },
            "one claim must not carry its neighbours"
        );
        assert_eq!(
            claimed("cargo doc --no-deps          clean"),
            Claimed { tests_pass: false, clippy_clean: false, fmt_clean: false, docs_clean: true },
            "and the newest one carries no neighbours either"
        );

        // A count-shaped claim is deliberately NOT read as `passing`. That form was removed
        // because it churns, and silently accepting it again would bring the churn back.
        assert_eq!(
            claimed("cargo test --workspace       677 passed"),
            Claimed { tests_pass: false, clippy_clean: false, fmt_clean: false, docs_clean: false }
        );
    }
}
