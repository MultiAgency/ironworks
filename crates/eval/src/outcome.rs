//! Classify a turn before grading it, and never the other way round.
//!
//! # The rule this file exists to enforce
//!
//! **An unavailable dependency blocks. It does not fail, and it certainly does not get graded.**
//! Grading the empty string a dead turn returns is how a provider outage becomes an
//! answer-quality result — a number that looks like the service reasoning badly when nothing
//! reasoned at all. `docs/INVARIANTS.md` §F-14 states the general form: a check that could not
//! run is never a PASS, and `BLOCKED` is a distinct outcome rather than a bad FAIL.
//!
//! # The non-vacuity control
//!
//! A turn is only graded once it proves it was built over the whole book. That is asserted **per
//! case, not once per run**, because degradation is per turn: the record store can go away
//! between case 3 and case 4, and case 5's answer would otherwise be graded as though it had seen
//! everything.

/// What a turn's own telemetry says about the composition that produced it.
///
/// The telemetry line is the only place a composition identifies itself, which is the entire
/// reason this driver reads stderr at all:
///
/// ```text
/// alloy 14337 bytes (67f80eb2d20172fa) · envelope 14520 bytes · 40 record(s) [complete] · ...
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    pub alloy_bytes: usize,
    pub digest: String,
    pub envelope_bytes: usize,
    pub records: usize,
    pub state: String,
}

/// Parse the telemetry line, or `None` when the turn reported none.
///
/// Hand-parsed rather than matched: the shape is fixed and a dependency for one line is a poor
/// trade in a workspace that justifies its lockfile by category.
pub fn telemetry(stderr: &str) -> Option<Telemetry> {
    let after_alloy = stderr.find("alloy ")? + "alloy ".len();
    let rest = &stderr[after_alloy..];
    let alloy_bytes = leading_number(rest)?;
    let open = rest.find('(')?;
    let close = rest[open..].find(')')? + open;
    let digest = rest[open + 1..close].to_owned();
    if digest.is_empty() || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let after_envelope = rest.find("envelope ")? + "envelope ".len();
    let envelope_bytes = leading_number(&rest[after_envelope..])?;

    // `<n> record(s) [<state>]` — the count is the number immediately before "record(s)".
    let marker = rest.find("record(s)")?;
    let records = trailing_number(&rest[..marker])?;
    let open = rest[marker..].find('[')? + marker;
    let close = rest[open..].find(']')? + open;
    let state = rest[open + 1..close].to_owned();

    Some(Telemetry { alloy_bytes, digest, envelope_bytes, records, state })
}

fn leading_number(text: &str) -> Option<usize> {
    let digits: String = text.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn trailing_number(text: &str) -> Option<usize> {
    let digits: String = text
        .trim_end()
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// A turn that could not be measured, with the sentence saying why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked(pub String);

/// True when the model provider, not the seam, is what went wrong.
pub fn no_model(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    ["not a valid model name", "model provider rejected", "rate limited"]
        .iter()
        .any(|p| s.contains(p))
        || (s.contains("provider ") && s.contains("unavailable"))
}

/// True when the turn outran the runtime's synchronous window.
pub fn turn_timeout(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("timed out writing") || s.contains("timed out reading")
}

/// True only when **another invocation must adopt the same journalled logical turn**.
///
/// This is recovery, not retry, and the distinction is the whole of it. The runtime gives up
/// waiting at ~30s but **the turn still ran**; re-invoking against the *same* journal adopts the
/// unresolved turn and replays one logical turn rather than billing a second execution. A fresh
/// journal here would mint a new idempotency key, which is the forbidden thing.
///
/// A definite never-sent turn is explicitly not recoverable — it is safe to try again, and saying
/// otherwise would turn a safe retry into a phantom adoption.
pub fn needs_journal_recovery(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    turn_timeout(&s)
        || (s.contains("a turn may have run")
            && s.contains("do not retry")
            && s.contains("recover it from the journal"))
}

/// Classify a finished turn. `Ok` means it is safe to grade; `Err` means it must not be.
pub fn classify(
    stderr: &str,
    exit_code: i32,
    answer: &str,
    expected_records: usize,
) -> Result<Telemetry, Blocked> {
    if no_model(stderr) {
        return Err(Blocked(format!("model provider unavailable: {}", last_line(stderr))));
    }
    if turn_timeout(stderr) {
        return Err(Blocked(
            "turn exceeded the runtime's ~30s synchronous limit — a boundary on our side of the \
             seam, not answer quality"
                .to_owned(),
        ));
    }
    if exit_code != 0 {
        return Err(Blocked(format!("turn exited {exit_code}: {}", last_line(stderr))));
    }
    let Some(t) = telemetry(stderr) else {
        return Err(Blocked(
            "the turn reported no composition telemetry; nothing identifies what was graded"
                .to_owned(),
        ));
    };
    if t.state != "complete" || t.records != expected_records {
        return Err(Blocked(format!(
            "built over {} record(s) [{}], expected {expected_records} [complete] — an answer over \
             a partial book grades nothing",
            t.records, t.state
        )));
    }
    if answer.trim().is_empty() {
        return Err(Blocked("the turn succeeded but returned no answer text".to_owned()));
    }
    Ok(t)
}

fn last_line(text: &str) -> String {
    text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "ironworks: alloy 14337 bytes (67f80eb2d20172fa) · envelope 14520 bytes · \
40 record(s) [complete] · turn ok";

    #[test]
    fn telemetry_identifies_the_composition_that_was_graded() {
        let t = telemetry(GOOD).expect("parses");
        assert_eq!(t.alloy_bytes, 14337);
        assert_eq!(t.digest, "67f80eb2d20172fa");
        assert_eq!(t.envelope_bytes, 14520);
        assert_eq!(t.records, 40);
        assert_eq!(t.state, "complete");
    }

    #[test]
    fn a_turn_with_no_telemetry_is_blocked_not_graded() {
        let got = classify("ironworks: something happened", 0, "an answer", 40);
        assert!(
            matches!(got, Err(Blocked(ref w)) if w.contains("no composition telemetry")),
            "{got:?}"
        );
    }

    #[test]
    fn a_partial_book_blocks_because_it_grades_nothing() {
        // The non-vacuity control, per case: the store can die between case 3 and case 4.
        let partial = GOOD.replace("40 record(s)", "6 record(s)");
        let got = classify(&partial, 0, "an answer", 40);
        assert!(
            matches!(got, Err(Blocked(ref w)) if w.contains("built over 6 record(s)")),
            "{got:?}"
        );
    }

    #[test]
    fn an_incomplete_supply_blocks_even_at_the_right_count() {
        let degraded = GOOD.replace("[complete]", "[partial]");
        assert!(classify(&degraded, 0, "an answer", 40).is_err());
    }

    #[test]
    fn a_provider_outage_is_never_an_answer_quality_result() {
        let out = "ironworks: model provider rejected the request";
        assert!(
            matches!(classify(out, 2, "", 40), Err(Blocked(ref w)) if w.contains("provider unavailable"))
        );
    }

    #[test]
    fn a_nonzero_exit_blocks() {
        assert!(classify(GOOD, 2, "an answer", 40).is_err());
    }

    #[test]
    fn an_empty_answer_blocks_rather_than_being_graded() {
        assert!(
            matches!(classify(GOOD, 0, "   ", 40), Err(Blocked(ref w)) if w.contains("no answer text"))
        );
    }

    #[test]
    fn a_clean_turn_is_graded() {
        assert!(classify(GOOD, 0, "an answer", 40).is_ok());
    }

    #[test]
    fn recovery_is_distinguished_from_retry() {
        // Recovery: the turn ran and its answer must be adopted from the journal.
        assert!(needs_journal_recovery("ironworks: timed out writing the request"));
        assert!(needs_journal_recovery(
            "a turn MAY have run — do NOT retry; recover it from the journal"
        ));
        // Not recovery: nothing ran, so a fresh attempt is safe and adoption would be a phantom.
        assert!(!needs_journal_recovery("no turn ran; it is safe to try again"));
    }
}
