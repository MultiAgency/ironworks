//! Joining this crate's two verdict channels into one evidence vocabulary.
//!
//! This is the only crate that can see both halves — `ironworks-graders` is a leaf with no
//! dependencies at all, and `ironworks-domain` cannot reach it — so the projection lives here
//! rather than beside either type it reads. That is also where it belongs by role: the driver is
//! what already holds a graded verdict in one hand and a blocked run in the other, and
//! `ironworks_domain::Finding`'s whole purpose is that those are two different questions.

use ironworks_domain::Finding;
use ironworks_graders::Verdict;

/// A grader's verdict, as an evidence finding.
///
/// **Total and injective, and it can never produce [`Finding::Unobserved`].** A grader reads an
/// answer that exists; not being able to decide about it is `Review`, which is a different fact
/// from the answer never having been produced. Collapsing those is the defect
/// `docs/experiments/0050-blocked-was-being-read-as-fail.md` measured in the other direction.
pub fn from_verdict(verdict: Verdict) -> Finding {
    match verdict {
        Verdict::Pass => Finding::Upheld,
        Verdict::Fail => Finding::Refuted,
        Verdict::Review => Finding::Indeterminate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_projection_is_injective_and_never_unobserved() {
        // Injective: three verdicts, three distinct findings. A mapping that sent two verdicts to
        // one finding would be the collapse this type exists to prevent, and it would pass any
        // test that only checked `Pass -> Upheld`.
        let mapped: Vec<Finding> =
            [Verdict::Pass, Verdict::Fail, Verdict::Review].into_iter().map(from_verdict).collect();
        let mut distinct = mapped.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), mapped.len(), "two verdicts collapsed into one finding");
        // A grader looks at an answer, so it can never report that nothing was looked at.
        assert!(mapped.iter().all(|f| f.observed()));
    }

    #[test]
    fn no_verdict_can_produce_unobserved() {
        // The load-bearing half of the join: `Unobserved` is unreachable from a grader, so the
        // only thing that can put it in an evidence state is a run that produced no answer. If
        // this ever fails, a blocked run and a graded one have become indistinguishable — which is
        // `0050` exactly.
        for verdict in [Verdict::Pass, Verdict::Fail, Verdict::Review] {
            assert_ne!(from_verdict(verdict), Finding::Unobserved, "{verdict:?}");
        }
    }
}
