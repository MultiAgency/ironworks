//! Record a deterministic check run into an evidence state.
//!
//! ```text
//! cargo run -q -p ironworks-claims -- --json /tmp/run.json
//! cargo run -q -p ironworks-evidence --bin record-findings -- \
//!     --findings /tmp/run.json --state /tmp/E_t.db --candidate <digest> --loop loop-1
//! ```
//!
//! # Why this is a separate program
//!
//! arXiv 2609.01481 §3.4.3 hands its QA role `Runtime.check(A_t)` — a deterministic build-and-test
//! result — alongside what the role observes itself. `ironworks-claims` **is** that check for this
//! tree: every claim it measures, each `Pass`, `Fail` or `Blocked`, against a candidate.
//!
//! It does not write here itself, and that is the point. `DESIGN.md`'s layering row gives that
//! crate no internal dependencies at all, so the thing that verifies the workspace cannot be broken
//! by a change to something it verifies. It emits JSON; this reads it. The paper calls the same
//! split a "benchmark adapter" that normalizes a report into the evidence bundle.
//!
//! # The projection, and the one case it must not invent
//!
//! `Pass` and `Fail` are observations that decided. **`Blocked` is `Unobserved`** — the check could
//! not be performed, so nothing was established either way, and it must never arrive at a planner
//! as repair work. That is the whole of `docs/adr/0013-one-evidence-vocabulary-with-two-existing-projections.md`
//! applied to one more producer.
//!
//! Nothing here produces `Indeterminate`: a deterministic check either ran and decided or did not
//! run. Only an instrument that can shrug — a grader — can report that, which is why the two
//! projections are separate functions in separate crates rather than one that guesses.

use ironworks_domain::{Finding, Utc};
use ironworks_evidence::state::{
    CandidateDigest, ClaimId, EvidenceState, LoopId, Observation, SubjectId,
};

fn flag(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

/// The control names its checks in words — `symbol citations` — and an evidence id refuses
/// whitespace.
///
/// Joined with `-` here, at the one boundary that reads that report, so the evidence state's rule
/// stays a type with no exception for one producer. Deterministic, so one check is one claim on
/// every run.
fn id_form(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

/// The control's three verdicts, as findings. Total, and `Blocked` is the one that must not become
/// a decision.
fn from_control(verdict: &str) -> Option<Finding> {
    match verdict {
        "PASS" => Some(Finding::Upheld),
        "FAIL" => Some(Finding::Refuted),
        "BLOCKED" => Some(Finding::Unobserved),
        _ => None,
    }
}

fn main() -> std::process::ExitCode {
    let (Some(findings), Some(state), Some(candidate), Some(loop_id)) =
        (flag("--findings"), flag("--state"), flag("--candidate"), flag("--loop"))
    else {
        eprintln!(
            "usage: record-findings --findings <json> --state <db> --candidate <digest> --loop <id>"
        );
        return std::process::ExitCode::from(2);
    };

    let fail = |why: String| -> std::process::ExitCode {
        eprintln!("record-findings: {why}");
        std::process::ExitCode::from(2)
    };

    let raw = match std::fs::read_to_string(&findings) {
        Ok(r) => r,
        Err(e) => return fail(format!("cannot read {findings}: {e}")),
    };
    let doc: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(d) => d,
        Err(e) => return fail(format!("{findings} is not the expected JSON: {e}")),
    };
    let Some(rows) = doc.get("findings").and_then(|f| f.as_array()) else {
        return fail(format!("{findings} carries no `findings` array"));
    };

    let (candidate, loop_id) = match (CandidateDigest::parse(candidate), LoopId::parse(loop_id)) {
        (Ok(c), Ok(l)) => (c, l),
        (Err(e), _) | (_, Err(e)) => return fail(e.to_string()),
    };
    let state = match EvidenceState::open(&state) {
        Ok(s) => s,
        Err(e) => return fail(e.to_string()),
    };
    // Refused rather than defaulted. A clock before 1970 used to record `0` — a false instant, in a
    // store that cannot be corrected in place, and one that still sorts.
    let observed_at = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => match Utc::from_unix_seconds(since.as_secs()) {
            Ok(at) => at,
            Err(e) => return fail(e.to_string()),
        },
        Err(_) => return fail("the host clock reads before 1970; nothing can be dated".to_owned()),
    };

    let mut written = 0usize;
    for row in rows {
        let (Some(subject), Some(verdict)) = (
            row.get("subject").and_then(|v| v.as_str()),
            row.get("verdict").and_then(|v| v.as_str()),
        ) else {
            return fail(format!("a finding is missing `subject` or `verdict`: {row}"));
        };
        let Some(finding) = from_control(verdict) else {
            // A verdict spelling this build does not know is refused, never bucketed. Guessing
            // would put an unknown state into an append-only store nobody can correct in place.
            return fail(format!("a verdict this build does not understand: {verdict:?}"));
        };
        // Identity and location, taken from the fields that mean them. `ironworks-claims`
        // emits both because for its tally rows they differ: the claim is `tally:<transcript>`
        // and the subject is the `README.md:164` anchor where a reader should look. Measured
        // before the control carried a distinct claim, 27 of 43 identities were line anchors,
        // so the same claim arrived at each run as a new one — which is the one thing an
        // evidence state exists to show. A finding without a claim falls back to its subject,
        // which is what an older emitter produced and is correct for every named check.
        let identity = row.get("claim").and_then(|v| v.as_str()).unwrap_or(subject);
        let (claim, subj) =
            match (ClaimId::parse(id_form(identity)), SubjectId::parse(id_form(subject))) {
                (Ok(c), Ok(s)) => (c, s),
                (Err(e), _) | (_, Err(e)) => return fail(e.to_string()),
            };
        let observation = Observation {
            claim,
            subject: subj,
            finding,
            candidate: candidate.clone(),
            observed_at: observed_at.clone(),
        };
        if let Err(e) = state.append(&loop_id, &observation) {
            return fail(e.to_string());
        }
        written += 1;
    }

    let standing = match state.standing(&candidate) {
        Ok(s) => s,
        Err(e) => return fail(e.to_string()),
    };
    println!(
        "recorded {written} observation(s) against {}\n  \
         preserve {}  repair {}  unresolved {}  unmeasured {}",
        candidate.as_str(),
        standing.preserve.len(),
        standing.repair.len(),
        standing.unresolved.len(),
        standing.unmeasured.len()
    );
    std::process::ExitCode::SUCCESS
}
