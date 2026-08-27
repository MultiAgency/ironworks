//! Whether this turn continues the last one.
//!
//! Always chaining is the obvious implementation and it is wrong. IronClaw persists
//! `instructions` and replays them, so input grows by one whole alloy per turn — measured in
//! `docs/experiments/0001-alloy-drift.md` at ~3,100 tokens per turn, a 30-turn thread costing
//! $4.91 with the last turn roughly five times the first. Never chaining is also wrong: it is a
//! service that cannot be asked a follow-up.
//!
//! So continuation is a decision with a type, and a [`Fresh`](Continuation::Fresh) turn always
//! says **why** it is fresh. A chain that breaks silently looks identical to a service that has
//! forgotten you, and the operator would have nothing to read.
//!
//! # Two reasons a chain breaks, and only one of them is about money
//!
//! **The composition changed.** The reference refuses to continue a thread across a composition
//! change, using the instructions digest as thread identity, and it is right to: the persisted
//! instructions in that thread are the *old* alloy, so a chained turn would be answered under
//! guidance nobody is running any more — including, if the safety tail moved, an older safety
//! tail. This is a correctness break, not a cost one.
//!
//! **The thread got long.** Cost, and it stands alone even if the composition never moved.
//!
//! # Why there is no separate token bound
//!
//! The design called for "a bounded turn count *or* token estimate". They are the same lever.
//! Instructions dominate the growth and are re-persisted at a constant size every turn, so tokens
//! are proportional to turns for any one composition — a second knob would be the first knob in
//! different units, and two knobs for one property is one knob and one thing to keep in sync.

use crate::instance::ThreadPointer;

/// Why a turn is not continuing anything.
///
/// A reason per variant rather than a bool, because "the composition moved" and "this thread is
/// long enough" want different operator responses: the first is expected after any edit to scrap
/// or guidance, and the second is the cost bound doing its job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshBecause {
    /// Nothing to continue. The first turn in a conversation.
    NoPriorTurn,
    /// The instructions are not the ones that thread was built on. Continuing would answer under
    /// an alloy nobody is running.
    CompositionMoved { was: String, now: String },
    /// Long enough that the next turn's input is mostly its own history.
    ThreadLongEnough { turns: u32, limit: u32 },
}

impl std::fmt::Display for FreshBecause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPriorTurn => f.write_str("first turn in this conversation"),
            Self::CompositionMoved { was, now } => {
                write!(f, "composition moved {was} -> {now}; the thread carries the old alloy")
            }
            Self::ThreadLongEnough { turns, limit } => {
                write!(f, "thread reached {turns} turns (limit {limit})")
            }
        }
    }
}

/// What this turn does about the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    Fresh(FreshBecause),
    Chain(ThreadPointer),
}

impl Continuation {
    /// The pointer to send, if any. `Fresh` yields `None`, so a caller cannot chain by accident.
    pub fn previous(&self) -> Option<&ThreadPointer> {
        match self {
            Self::Chain(p) => Some(p),
            Self::Fresh(_) => None,
        }
    }

    pub fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh(_))
    }
}

/// What the journal knows about a conversation's thread.
///
/// Carried as one value rather than three arguments so a caller cannot pair one thread's pointer
/// with another's digest — the mistake that would chain across compositions while believing it
/// had checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorThread {
    pub pointer: ThreadPointer,
    /// The alloy digest the thread was built on.
    pub alloy: String,
    /// How many turns it already holds.
    pub turns: u32,
    /// What it has been shown of the book, if anything. `None` is never shown — a different fact
    /// from shown-an-empty-book, which carries a fingerprint like any other.
    pub supplied: Option<crate::supply::SuppliedMark>,
}

/// How long a thread may run before it is cheaper to start over.
///
/// Not a `Default`, for the same reason the pins have none: the number is a cost decision, and a
/// silent default is the one value that outranks a deliberate one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadLimit(u32);

impl ThreadLimit {
    /// # Panics
    /// Never. A limit of zero is rejected as an error, not asserted away.
    pub fn new(turns: u32) -> Result<Self, &'static str> {
        if turns == 0 {
            return Err("a thread limit of zero would mean never continuing; say so with Fresh");
        }
        Ok(Self(turns))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Decide, from what the journal holds and what this turn composes.
///
/// Order matters and is deliberate: **correctness before cost**. A thread whose composition moved
/// is refused for that reason even when it is also over the limit, because that is the reason an
/// operator needs to see — the cost bound would be the less alarming half of the truth.
pub fn decide(prior: Option<&PriorThread>, alloy: &str, limit: ThreadLimit) -> Continuation {
    let Some(prior) = prior else {
        return Continuation::Fresh(FreshBecause::NoPriorTurn);
    };
    let now = alloy.to_owned();
    if prior.alloy != now {
        return Continuation::Fresh(FreshBecause::CompositionMoved {
            was: prior.alloy.clone(),
            now,
        });
    }
    if prior.turns >= limit.get() {
        return Continuation::Fresh(FreshBecause::ThreadLongEnough {
            turns: prior.turns,
            limit: limit.get(),
        });
    }
    Continuation::Chain(prior.pointer.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ResponseId, Slug};

    fn alloy(text: &str) -> crate::composition::Alloy {
        // The one constructor a test can reach; the digest is over exactly these bytes.
        crate::composition::melt(&composition(), &guidance(), &Parts(text.to_owned())).unwrap()
    }

    struct Parts(String);
    impl crate::composition::PartSource for Parts {
        fn read(&self, p: &crate::composition::PartRef) -> Option<&str> {
            match p.as_str() {
                "p" => Some(&self.0),
                "tail" => Some("## Safety\n\nrules"),
                _ => None,
            }
        }
    }
    fn composition() -> crate::composition::Composition {
        crate::composition::Composition::new(
            vec![crate::composition::PartRef::new("p")],
            "# GUIDANCE",
            crate::composition::PartRef::new("tail"),
        )
        .unwrap()
    }
    fn guidance() -> crate::composition::Guidance {
        crate::composition::Guidance::parse(
            &format!(
                "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\n# C\n\n{}",
                "Real content about the organization. ".repeat(20)
            ),
            "acme",
            "account-analysis",
        )
        .unwrap()
    }
    fn pointer() -> ThreadPointer {
        crate::instance::ThreadScope::for_test(
            Slug::parse("acme").unwrap(),
            crate::ids::OrgId::parse("acme-sales").unwrap(),
        )
        .pointer(ResponseId::parse("resp_001").unwrap())
    }
    fn prior(a: &crate::composition::Alloy, turns: u32) -> PriorThread {
        PriorThread { pointer: pointer(), alloy: a.digest(), turns, supplied: None }
    }
    fn limit(n: u32) -> ThreadLimit {
        ThreadLimit::new(n).unwrap()
    }

    #[test]
    fn the_first_turn_of_a_conversation_is_fresh_and_says_so() {
        let a = alloy("role");
        assert_eq!(
            decide(None, &a.digest(), limit(8)),
            Continuation::Fresh(FreshBecause::NoPriorTurn)
        );
    }

    #[test]
    fn an_unchanged_composition_inside_the_limit_chains() {
        // The positive control. Without it every refusal below would pass on a decider that never
        // continues anything, which is the service that cannot be asked a follow-up.
        let a = alloy("role");
        let c = decide(Some(&prior(&a, 3)), &a.digest(), limit(8));
        assert_eq!(c.previous().map(|p| p.response().as_str()), Some("resp_001"));
        assert!(!c.is_fresh());
    }

    #[test]
    fn a_moved_composition_breaks_the_chain_because_the_thread_carries_the_old_alloy() {
        // Not a cost rule. The persisted instructions in that thread are the previous alloy, so a
        // chained turn is answered under guidance — and a safety tail — nobody is running.
        let before = alloy("role");
        let after = alloy("role, revised");
        assert_ne!(before.digest(), after.digest(), "the fixture must actually move the digest");
        let c = decide(Some(&prior(&before, 1)), &after.digest(), limit(8));
        assert!(matches!(c, Continuation::Fresh(FreshBecause::CompositionMoved { .. })), "{c:?}");
        assert_eq!(c.previous(), None, "a fresh decision cannot leak a pointer");
    }

    #[test]
    fn a_long_thread_breaks_the_chain_on_cost_alone() {
        let a = alloy("role");
        assert!(
            !decide(Some(&prior(&a, 7)), &a.digest(), limit(8)).is_fresh(),
            "under the limit chains"
        );
        let at = decide(Some(&prior(&a, 8)), &a.digest(), limit(8));
        assert_eq!(at, Continuation::Fresh(FreshBecause::ThreadLongEnough { turns: 8, limit: 8 }));
    }

    #[test]
    fn a_moved_composition_outranks_the_cost_bound() {
        // Both apply. The operator needs the correctness reason, not the cheaper-sounding one.
        let before = alloy("role");
        let after = alloy("role, revised");
        let c = decide(Some(&prior(&before, 99)), &after.digest(), limit(8));
        assert!(matches!(c, Continuation::Fresh(FreshBecause::CompositionMoved { .. })), "{c:?}");
    }

    #[test]
    fn a_limit_of_zero_is_refused_rather_than_meaning_never() {
        // "Never continue" is expressible — do not pass a prior thread — so a zero limit is a
        // mistake rather than a setting, and silently honouring it would hide the mistake.
        assert!(ThreadLimit::new(0).is_err());
        assert_eq!(ThreadLimit::new(1).unwrap().get(), 1);
    }

    #[test]
    fn every_fresh_reason_reads_as_a_sentence() {
        // A broken chain looks exactly like a service that forgot you, so the reason is what an
        // operator has to read.
        for r in [
            FreshBecause::NoPriorTurn,
            FreshBecause::CompositionMoved { was: "aaa".into(), now: "bbb".into() },
            FreshBecause::ThreadLongEnough { turns: 9, limit: 8 },
        ] {
            let said = r.to_string();
            assert!(said.len() > 12 && !said.contains("Fresh"), "{said}");
        }
    }
}
