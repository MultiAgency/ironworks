//! The anti-corruption layer: the only crate that speaks IronClaw's protocol.
//!
//! Everything else in the workspace is deliberately ignorant of it. If `ureq` types, HTTP status
//! codes, or IronClaw's own spellings (`"in_progress"`, the `"reborn"` model placeholder) appear
//! outside here, that is a leak.
//!
//! # Shape, derived rather than mirrored
//!
//! **[`Runtime::create`] and [`Runtime::fetch`] are separate on purpose.** The durability
//! boundary sits between them: the caller must make the response id durable the instant it
//! exists, because that id is the only handle to a turn that may already have been billed. An
//! `execute()` that hid the polling would make crash recovery (ADR 0005) impossible.
//!
//! **[`TurnFailure`] is asymmetric because its costs are.** Treating a may-have-run failure as
//! provably-unsent double-bills a turn and delivers a second answer; treating a provably-unsent
//! one as ambiguous merely blocks an update needlessly. So the default is [`TurnFailure::Unknown`]
//! and only positive evidence downgrades it. It implements no `From`, so `?` can never
//! silently promote one into the other.
//!
//! **Retries live here, but only for [`TurnFailure::NotSent`]** — a request that provably never
//! left cannot have executed, so retrying it is safe by construction. An `Unknown` is never
//! retried at any level.
//!
//! **The serving path cannot be entered without a verified instance.** [`Runtime::create`]
//! takes a [`ModelTurn`], and only `ServiceInstance<Verified>` can build one — so the scope check
//! is not a rule this crate remembers, it is the shape of its input.
//!
//! **HTTP status is not the success signal.** A `409` idempotency conflict is information, not a
//! failure, and a `200` can carry `status: "failed"` (both measured). So `http_status_as_error`
//! is off and the body is interpreted.

use ironworks_domain::ModelTurn;
use std::time::{Duration, Instant};

mod classify;
mod model;

pub use classify::{Cause, TurnFailure};
pub use model::{
    Accepted, IdempotencyKey, InvalidKey, Outcome, ResponseId, ToolCall, Usage, malformed_answer,
    request_bytes,
};

/// One wall-clock budget covering connect, create, poll and backoff.
///
/// A single owner for a single resource. ironworks ran a 150s poll deadline beside a 180s turn
/// budget, so which of two error messages an operator saw was decided by a race between them.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    started: Instant,
    budget: Duration,
}

impl Deadline {
    pub fn starting_now(budget: Duration) -> Self {
        Self { started: Instant::now(), budget }
    }
    pub fn remaining(&self) -> Duration {
        self.budget.saturating_sub(self.started.elapsed())
    }
    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }
    /// A per-operation timeout that can never outlive the budget.
    pub fn clamp(&self, want: Duration) -> Duration {
        want.min(self.remaining())
    }
}

/// What a caller may ask of the runtime. Deliberately small.
pub trait Runtime {
    /// Submit a turn. Returns as soon as the runtime answers — which may be with a
    /// non-terminal status, since it waits only ~30s before replying.
    fn create(
        &self,
        turn: &ModelTurn,
        key: &IdempotencyKey,
        deadline: &Deadline,
    ) -> Result<Accepted, TurnFailure>;

    /// Read a turn that already exists. Never executes anything.
    fn fetch(&self, id: &ResponseId, deadline: &Deadline) -> Result<Outcome, TurnFailure>;
}

/// How long to wait between polls, and the ceiling it backs off to.
const POLL_FIRST: Duration = Duration::from_secs(2);
const POLL_MAX: Duration = Duration::from_secs(10);
const POLL_GROWTH: u32 = 3; // multiply by 3/2 each time

/// Poll `fetch` until the turn reaches a terminal state or the budget runs out.
///
/// Free function rather than a trait method so it is testable against any [`Runtime`], and so
/// the polling policy is visible at the call site rather than buried in an implementation.
pub fn poll_to_completion(
    runtime: &dyn Runtime,
    id: &ResponseId,
    deadline: &Deadline,
    mut sleep: impl FnMut(Duration),
) -> Result<Outcome, TurnFailure> {
    let mut interval = POLL_FIRST;
    loop {
        match runtime.fetch(id, deadline)? {
            Outcome::Running => {}
            terminal => return Ok(terminal),
        }
        if deadline.expired() {
            // The turn is still running and we stopped watching. Its outcome is UNKNOWN, not
            // failed — the id remains valid and the answer is still fetchable later.
            return Err(TurnFailure::Unknown(Cause::BudgetExhausted));
        }
        sleep(deadline.clamp(interval));
        interval = (interval * POLL_GROWTH / 2).min(POLL_MAX);
    }
}

mod client;
pub use client::{Credential, Ironclaw};

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Scripted {
        steps: RefCell<Vec<Outcome>>,
    }
    impl Runtime for Scripted {
        fn create(
            &self,
            _: &ModelTurn,
            _: &IdempotencyKey,
            _: &Deadline,
        ) -> Result<Accepted, TurnFailure> {
            // These tests exercise polling only. Returning a failure rather than panicking keeps
            // the double honest: a test that reaches this sees a value, not a crash.
            Err(TurnFailure::Unknown(Cause::Transport("this double does not create".into())))
        }
        fn fetch(&self, _: &ResponseId, _: &Deadline) -> Result<Outcome, TurnFailure> {
            Ok(self.steps.borrow_mut().remove(0))
        }
    }

    fn id() -> ResponseId {
        ResponseId::parse("resp_abc").unwrap()
    }

    #[test]
    fn polling_stops_on_the_first_terminal_outcome() {
        let r = Scripted {
            steps: RefCell::new(vec![
                Outcome::Running,
                Outcome::Running,
                Outcome::Completed { text: "hi".into(), usage: None },
            ]),
        };
        let d = Deadline::starting_now(Duration::from_secs(60));
        let out = poll_to_completion(&r, &id(), &d, |_| {}).unwrap();
        assert!(matches!(out, Outcome::Completed { .. }));
        assert!(r.steps.borrow().is_empty(), "it should not have polled again");
    }

    #[test]
    fn a_failed_status_is_terminal_not_an_error() {
        // The turn definitively ran and produced nothing. That is an answer the caller acts on,
        // not a transport problem it might retry.
        let r = Scripted {
            steps: RefCell::new(vec![Outcome::Failed {
                code: "internal_error".into(),
                message: "An internal error occurred.".into(),
            }]),
        };
        let d = Deadline::starting_now(Duration::from_secs(60));
        assert!(matches!(
            poll_to_completion(&r, &id(), &d, |_| {}).unwrap(),
            Outcome::Failed { .. }
        ));
    }

    #[test]
    fn an_exhausted_budget_is_unknown_never_failed() {
        // We stopped watching a turn that was still running. It may complete. Calling that a
        // failure would invite a retry that bills a second execution.
        let r = Scripted { steps: RefCell::new(vec![Outcome::Running]) };
        let d = Deadline::starting_now(Duration::ZERO);
        let err = poll_to_completion(&r, &id(), &d, |_| {}).unwrap_err();
        assert!(matches!(err, TurnFailure::Unknown(Cause::BudgetExhausted)));
        assert!(!err.provably_unsent());
    }

    #[test]
    fn a_deadline_clamps_every_wait_to_what_is_left() {
        let d = Deadline::starting_now(Duration::from_millis(50));
        assert!(d.clamp(Duration::from_secs(30)) <= Duration::from_millis(50));
        let spent = Deadline::starting_now(Duration::ZERO);
        assert!(spent.expired());
        assert_eq!(spent.clamp(Duration::from_secs(30)), Duration::ZERO);
    }

    #[test]
    fn backoff_grows_then_caps() {
        let mut waits = Vec::new();
        let r = Scripted {
            steps: RefCell::new(vec![
                Outcome::Running,
                Outcome::Running,
                Outcome::Running,
                Outcome::Running,
                Outcome::Running,
                Outcome::Running,
                Outcome::Running,
                Outcome::Completed { text: "x".into(), usage: None },
            ]),
        };
        let d = Deadline::starting_now(Duration::from_secs(600));
        poll_to_completion(&r, &id(), &d, |w| waits.push(w)).unwrap();
        assert_eq!(waits[0], POLL_FIRST);
        assert!(waits.windows(2).all(|w| w[0] <= w[1]), "must be monotonic");
        assert!(waits.iter().all(|w| *w <= POLL_MAX), "must never exceed the ceiling");
        assert_eq!(*waits.last().unwrap(), POLL_MAX, "and should reach it");
    }
}
