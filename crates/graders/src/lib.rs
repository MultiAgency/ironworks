//! Answer-quality graders for the services this seam serves.
//!
//! # What a grader is, and what it is not
//!
//! A grader checks a few **load-bearing claims**, never wording or style. It answers "did this
//! answer keep the discipline the service promises?" — not "is this a good answer", which is not
//! a mechanical question. [`Verdict::Review`] exists for the gap between them: the answer may be
//! sound and the grader cannot establish it.
//!
//! # Why this is Rust and not the Python it replaces
//!
//! The Python graders were imported from `~/agency/ironworks-py` so that answer quality could be
//! measured against the reference's own bar. That was the right call while **parity** was the
//! goal. It no longer is, and with it goes the reason to run a second language in the measuring
//! path — and the reason to express every rule as a regex.
//!
//! The rules are written out here instead. `_absence_as_negative` is the argument: its first
//! alternative had an unbounded noun, so it graded "there is no ambiguity" as an overclaim about
//! the record. That defect lived through four experiments inside a raw string and is plainly
//! visible as a named predicate.
//!
//! # The controls are the specification
//!
//! Every rule below is pinned by controls in both directions, and they are the reason a rewrite
//! was safe without parity to check against. A rule that only had accepted controls would pass by
//! vacuity the moment it stopped firing — which is exactly how a narrowing goes wrong — so each
//! one carries a rejected control that must still fail.

pub mod cases;
pub mod discipline;
pub mod text;

/// What a grader concluded about one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The answer kept the discipline this case tests.
    Pass,
    /// The answer broke it, and `why` names which rule.
    Fail,
    /// The answer may be sound; this grader cannot establish it mechanically.
    ///
    /// **Never a synonym for FAIL.** Collapsing the two would make the suite report a defect
    /// wherever it merely could not see one, which is the same error as treating a check that
    /// could not run as a failure (`docs/INVARIANTS.md` §F-14).
    Review,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Review => "REVIEW",
        }
    }
}

/// A verdict and the sentence an operator reads.
///
/// The reason travels with the verdict because a bare FAIL is unactionable: every recorded
/// regrade in `docs/experiments/regrades.json` was found by reading *why* a grader said what it
/// said, against the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graded {
    pub verdict: Verdict,
    pub why: String,
}

impl Graded {
    pub fn pass(why: impl Into<String>) -> Self {
        Self { verdict: Verdict::Pass, why: why.into() }
    }
    pub fn fail(why: impl Into<String>) -> Self {
        Self { verdict: Verdict::Fail, why: why.into() }
    }
    pub fn review(why: impl Into<String>) -> Self {
        Self { verdict: Verdict::Review, why: why.into() }
    }
}
