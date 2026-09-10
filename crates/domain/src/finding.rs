//! What one claim's evidence establishes — and whether anything ever looked.
//!
//! # Two questions, not one scale
//!
//! An evidence state answers two independent questions about every claim it carries:
//!
//! 1. **Was the observation made at all?**
//! 2. **If it was, what did it establish?**
//!
//! Flattening those into one enum is how a repository loses the difference between *nothing was
//! measured* and *something was measured and it was bad* — and that difference is the one this
//! codebase has spent the most measurements defending. `docs/experiments/0050-blocked-was-being-read-as-fail.md`
//! found seven of eleven blocking causes scored as product failures; §F-14 is the rule; the
//! `Verdict` in `ironworks-graders` says in its own doc that REVIEW is *never* a synonym for FAIL.
//!
//! [`Finding`] is the join of those two questions, and it is deliberately the **only** vocabulary
//! an evidence state uses. The existing verdict types are projections of it, not rivals:
//!
//! | source | Upheld | Refuted | Indeterminate | Unobserved |
//! |---|---|---|---|---|
//! | `ironworks_graders::Verdict` | `Pass` | `Fail` | `Review` | — (it has no such case) |
//! | `ironworks_lifecycle::inspect::Verdict` | `Pass` | `Fail` | — (it has no such case) | `Blocked` |
//! | `ironworks_eval`'s driver | a graded case | a graded case | a graded case | `Blocked(_)` |
//!
//! **Neither existing enum is wrong and neither is complete.** A grader looks at an answer that
//! exists, so it can be undecided but never unobserved. A reconciliation either read the world or
//! did not, and has no notion of an instrument that shrugged. Each is the projection its own domain
//! needs; an evidence state spans both, so it needs the join.
//!
//! # Why not the two-valued partition
//!
//! arXiv 2609.01481 §A.4 partitions its evidence bundle into `verified ∪ gap`, disjoint and
//! exhaustive, and places "claims without sufficient evidence" in `gap` alongside observed
//! failures. That is a two-valued projection of this type, and
//! [`tests::the_two_valued_partition_cannot_be_inverted`] is the demonstration that it discards
//! information rather than summarising it: two claims in genuinely different states arrive at the
//! planner as the same instruction.
//!
//! The cost is not theoretical. A `gap` that is really `Unobserved` sends the next iteration to
//! repair something that may already work, and the repair's own evidence is then read as progress.

/// What one claim's evidence establishes.
///
/// See the module documentation for why this has four cases and why the existing verdict types are
/// projections of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Finding {
    /// Observed, and the evidence supports the claim.
    Upheld,
    /// Observed, and the evidence contradicts it. A determinate negative.
    Refuted,
    /// Observed, and the instrument could not decide. **Not a failure** — the claim may hold; this
    /// says only that what was collected cannot settle it. A person reads it; a tally does not
    /// count it.
    Indeterminate,
    /// Not observed. **Not a failure and not a pass** — there is no evidence in either direction,
    /// because the measurement did not happen.
    Unobserved,
}

impl Finding {
    /// Every case, so a caller that must be exhaustive can be checked rather than trusted.
    pub const fn all() -> [Self; 4] {
        [Self::Upheld, Self::Refuted, Self::Indeterminate, Self::Unobserved]
    }

    /// Whether anything looked. The first of the two questions.
    pub fn observed(self) -> bool {
        !matches!(self, Self::Unobserved)
    }

    /// Whether the observation settled the claim. The second question, and `false` for both of the
    /// cases that are not verdicts about the claim.
    pub fn decided(self) -> bool {
        matches!(self, Self::Upheld | Self::Refuted)
    }

    /// The only case that supports a claim. Written as its own predicate because `!refuted` is the
    /// mistake it exists to prevent: three of the four cases are not a pass.
    pub fn upheld(self) -> bool {
        matches!(self, Self::Upheld)
    }

    /// How this is stored and read back.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Upheld => "upheld",
            Self::Refuted => "refuted",
            Self::Indeterminate => "indeterminate",
            Self::Unobserved => "unobserved",
        }
    }

    /// The inverse of [`Self::wire`]. `None` for a value this build does not understand — never
    /// silently a failure, for the same reason `ToolState::Unrecognized` is never read as
    /// `Disabled`.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::all().into_iter().find(|f| f.wire() == raw)
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_axes_are_independent() {
        // Exactly one case is unobserved, and exactly two are decided. If a later edit adds a
        // case, this fails rather than letting the new one inherit whichever half it resembles.
        assert_eq!(Finding::all().iter().filter(|f| !f.observed()).count(), 1);
        assert_eq!(Finding::all().iter().filter(|f| f.decided()).count(), 2);
        // Undecided-but-observed is exactly one case, and it is not the unobserved one.
        let undecided: Vec<_> =
            Finding::all().into_iter().filter(|f| f.observed() && !f.decided()).collect();
        assert_eq!(undecided, [Finding::Indeterminate]);
    }

    #[test]
    fn only_one_case_supports_a_claim() {
        // The positive control for `upheld`: a predicate that answered `true` for anything else
        // would pass every "is it a pass" test written the obvious way.
        assert_eq!(Finding::all().into_iter().filter(|f| f.upheld()).count(), 1);
        assert!(Finding::Upheld.upheld());
    }

    #[test]
    fn every_case_round_trips() {
        for f in Finding::all() {
            assert_eq!(Finding::parse(f.wire()), Some(f), "{f} did not round trip");
        }
        assert_eq!(Finding::parse("gap"), None, "an unknown value is not guessed at");
    }

    /// The demonstration that `verified ∪ gap` loses something, rather than summarising it.
    ///
    /// arXiv 2609.01481 §A.4 places observed failures and "claims without sufficient evidence" in
    /// one bucket. Modelled here as a function, that bucket is not injective: two claims in
    /// genuinely different states become the same instruction to the next iteration.
    #[test]
    fn the_two_valued_partition_cannot_be_inverted() {
        fn paper_bucket(f: Finding) -> &'static str {
            if f.upheld() { "verified" } else { "gap" }
        }
        let gaps: Vec<Finding> =
            Finding::all().into_iter().filter(|f| paper_bucket(*f) == "gap").collect();
        assert_eq!(gaps.len(), 3, "three distinct states collapse into one");
        // The pair that matters: a measured failure and a measurement that never happened.
        assert_eq!(paper_bucket(Finding::Refuted), paper_bucket(Finding::Unobserved));
        assert_ne!(Finding::Refuted, Finding::Unobserved);
        // And the one that sends an iteration to repair something that may already work.
        assert_eq!(paper_bucket(Finding::Indeterminate), paper_bucket(Finding::Refuted));
    }
}
