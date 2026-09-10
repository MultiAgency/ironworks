//! What is known about an **outbound** message, and how one attempt's outcome refines it.
//!
//! # Two axes, not one
//!
//! Delivery and acknowledgement are different facts about different directions:
//!
//! - **Delivery** is what this module models: *did the outbound answer reach the client?* It is
//!   tenant-scoped, because the answer belongs to one conversation.
//! - **Acknowledgement** is what [`crate::progress`] and the channel cursor model: *has the
//!   channel been told we are past this inbound update, so it will stop redelivering it?* It is
//!   channel-scoped and shared across tenants.
//!
//! The reference implementation keeps them apart as `DELIVERED` and `ACKED`. Collapsing them
//! into one "done" is how an update that was answered but never acknowledged gets compacted
//! away, and then redelivered to a bridge with no record of having answered it.
//!
//! # Evidence sharpens; it never forgets
//!
//! [`DeliveryState`] is a join-semilattice ordered by **how much client-visible content may
//! already exist**, and [`DeliveryState::after`] is the join. That makes the rule
//!
//! > a failed or partial retry may refine delivery evidence, but must never erase evidence that
//! > determines whether another retry would duplicate client-visible content
//!
//! a property of the lattice rather than a code-review convention: a join is monotone, so no
//! attempt outcome can move the state *down*. The concrete defect it prevents:
//! `bridge redeliver` is designed to be retried, so a failed redelivery is the normal path — and
//! in the reference implementation a failed attempt could overwrite `delivery_partial` (one
//! chunk is already in the client's room) with `delivery_uncertain` (nothing is known), losing
//! the only record of whether the *next* retry duplicates content a client has already read.
//!
//! # What this deliberately does not do
//!
//! Nothing here retries. [`Repair`] names what would fix each state, and every non-trivial repair
//! reports [`Repair::requires_operator`]. An automatic resend on ambiguous evidence is a
//! duplicate-delivery generator, and the reference corpus asserts against exactly that: *"a
//! permanent rejection was retried without operator action"*.

use std::fmt;

/// What is durably known about the outbound message for one turn.
///
/// The variants are ordered by risk of client-visible duplication, which is the only ordering
/// that matters when deciding whether a resend is safe. See `DeliveryState::rank`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// No attempt has been made. The bottom of the lattice.
    Pending,
    /// An attempt was made and **provably** delivered nothing: the channel rejected it before
    /// accepting any part. A resend cannot duplicate, because there is nothing to duplicate.
    KnownUnsent,
    /// An attempt was made and its outcome is unknown. `acknowledged_chunks` is how many parts
    /// the channel confirmed before contact was lost — `0` means nothing is known to have
    /// landed, which is *more* dangerous than [`Self::KnownUnsent`], not equivalent to it.
    Uncertain { acknowledged_chunks: u32 },
    /// The channel accepted the whole message. Terminal: a later failed attempt does not
    /// un-deliver it.
    Delivered,
}

impl DeliveryState {
    /// Position in the lattice: how much client-visible content may already exist.
    ///
    /// `Pending` < `KnownUnsent` < `Uncertain { 0 }` < `Uncertain { 1 }` < … < `Delivered`.
    ///
    /// `KnownUnsent` sits *below* `Uncertain { acknowledged_chunks: 0 }` on purpose. Both have
    /// zero confirmed chunks, but one is a proof of absence and the other is an absence of
    /// proof, and only the first makes a resend free.
    fn rank(self) -> u64 {
        match self {
            Self::Pending => 0,
            Self::KnownUnsent => 1,
            Self::Uncertain { acknowledged_chunks } => 2 + u64::from(acknowledged_chunks),
            Self::Delivered => u64::MAX,
        }
    }

    /// Fold one attempt's outcome into what is already known.
    ///
    /// This is the lattice join, so it is commutative, associative, idempotent, and — the point —
    /// monotone: `state.after(o) >= state` for every `state` and every `o`. Evidence cannot be
    /// erased by a later attempt that knows less.
    #[must_use]
    pub fn after(self, outcome: AttemptOutcome) -> Self {
        let observed = Self::from(outcome);
        if observed.rank() > self.rank() { observed } else { self }
    }

    /// What would move this to [`Self::Delivered`].
    pub fn repair(self) -> Repair {
        match self {
            Self::Delivered => Repair::None,
            Self::Pending => Repair::Send,
            Self::KnownUnsent => Repair::ResendSafe,
            Self::Uncertain { .. } => Repair::ResendMayDuplicate,
        }
    }

    /// Whether automatic work remains for this message.
    ///
    /// `Pending` is the only state the process may act on by itself. Once an attempt has been
    /// made and not confirmed, the next move belongs to an operator — which is also why an
    /// un-confirmed delivery does **not** hold the channel cursor open. A parked message must
    /// not deafen the bridge to every conversation behind it.
    pub fn awaits_automatic_work(self) -> bool {
        matches!(self, Self::Pending)
    }

    /// The stable spelling used on disk and in operator output. One source of truth, so a state
    /// cannot be written under one name and read under another.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::KnownUnsent => "known_unsent",
            Self::Uncertain { .. } => "uncertain",
            Self::Delivered => "delivered",
        }
    }
}

impl fmt::Display for DeliveryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uncertain { acknowledged_chunks: 0 } => {
                f.write_str("uncertain (nothing confirmed)")
            }
            Self::Uncertain { acknowledged_chunks } => {
                write!(
                    f,
                    "uncertain ({acknowledged_chunks} chunk(s) confirmed in the client's room)"
                )
            }
            other => f.write_str(other.tag()),
        }
    }
}

/// What one send attempt reported.
///
/// A transport reports what it observed; it does not decide the resulting state. That decision
/// is [`DeliveryState::after`], which also has the prior evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Every chunk was accepted.
    Complete,
    /// The channel refused it, before accepting any part. This must only be reported when the
    /// transport can *prove* nothing was accepted.
    Rejected,
    /// Contact was lost. `acknowledged_chunks` is how many parts the channel confirmed first.
    Ambiguous { acknowledged_chunks: u32 },
}

impl From<AttemptOutcome> for DeliveryState {
    fn from(outcome: AttemptOutcome) -> Self {
        match outcome {
            AttemptOutcome::Complete => Self::Delivered,
            AttemptOutcome::Rejected => Self::KnownUnsent,
            AttemptOutcome::Ambiguous { acknowledged_chunks } => {
                Self::Uncertain { acknowledged_chunks }
            }
        }
    }
}

/// What would move a message to delivered, and whether the process may do it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repair {
    /// Already delivered.
    None,
    /// Never attempted. The process sends it.
    Send,
    /// Attempted and provably unsent. A resend is free of duplication.
    ResendSafe,
    /// A resend may put a second copy of content in a client's room.
    ResendMayDuplicate,
}

impl Repair {
    /// Whether a human must decide. Both post-attempt repairs report `true`: the difference
    /// between them is *diagnostic* — it tells the operator whether saying yes duplicates — not
    /// a licence for one of them to proceed unattended.
    pub fn requires_operator(self) -> bool {
        matches!(self, Self::ResendSafe | Self::ResendMayDuplicate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state, including several `Uncertain` widths, so the join laws are checked across
    /// the whole lattice rather than at three convenient points.
    fn all() -> Vec<DeliveryState> {
        let mut v = vec![DeliveryState::Pending, DeliveryState::KnownUnsent];
        v.extend((0..4).map(|n| DeliveryState::Uncertain { acknowledged_chunks: n }));
        v.push(DeliveryState::Delivered);
        v
    }

    fn outcomes() -> Vec<AttemptOutcome> {
        let mut v = vec![AttemptOutcome::Complete, AttemptOutcome::Rejected];
        v.extend((0..4).map(|n| AttemptOutcome::Ambiguous { acknowledged_chunks: n }));
        v
    }

    /// The invariant the whole module exists for, checked exhaustively rather than by example.
    #[test]
    fn an_attempt_never_lowers_what_is_known() {
        for state in all() {
            for outcome in outcomes() {
                let after = state.after(outcome);
                assert!(
                    after.rank() >= state.rank(),
                    "{state:?}.after({outcome:?}) = {after:?} forgot evidence"
                );
            }
        }
    }

    #[test]
    fn the_join_is_commutative_associative_and_idempotent() {
        // Commutativity and associativity are stated over the underlying join, which `after`
        // exposes one argument at a time.
        let join = |a: DeliveryState, b: DeliveryState| if b.rank() > a.rank() { b } else { a };
        for a in all() {
            assert_eq!(join(a, a), a, "not idempotent at {a:?}");
            for b in all() {
                assert_eq!(join(a, b), join(b, a), "not commutative at {a:?}/{b:?}");
                for c in all() {
                    assert_eq!(join(join(a, b), c), join(a, join(b, c)));
                }
            }
        }
    }

    /// The specific regression: a failed redelivery must not erase the chunk count the next one
    /// depends on. This is the reference corpus's
    /// `test_a_FAILED_redelivery_preserves_the_delivery_evidence_the_next_one_needs`.
    #[test]
    fn a_failed_retry_keeps_the_chunks_already_in_the_clients_room() {
        let known = DeliveryState::Uncertain { acknowledged_chunks: 1 };

        // A later attempt that reports "nothing was sent" is talking about *itself*. It cannot
        // speak for the chunk the client can already read.
        assert_eq!(known.after(AttemptOutcome::Rejected), known);
        // Nor can a later attempt that knows nothing at all.
        assert_eq!(known.after(AttemptOutcome::Ambiguous { acknowledged_chunks: 0 }), known);
        // But a later attempt that got further does refine it.
        assert_eq!(
            known.after(AttemptOutcome::Ambiguous { acknowledged_chunks: 3 }),
            DeliveryState::Uncertain { acknowledged_chunks: 3 }
        );
    }

    #[test]
    fn proof_of_absence_outranks_absence_of_proof() {
        // The distinction that decides whether an operator's "yes, resend" duplicates.
        assert!(
            DeliveryState::KnownUnsent.rank()
                < DeliveryState::Uncertain { acknowledged_chunks: 0 }.rank()
        );
        assert_eq!(DeliveryState::KnownUnsent.repair(), Repair::ResendSafe);
        assert_eq!(
            DeliveryState::Uncertain { acknowledged_chunks: 0 }.repair(),
            Repair::ResendMayDuplicate
        );
    }

    #[test]
    fn delivery_is_terminal_under_a_later_failure() {
        for outcome in outcomes() {
            assert_eq!(DeliveryState::Delivered.after(outcome), DeliveryState::Delivered);
        }
    }

    #[test]
    fn only_an_unattempted_message_is_the_processs_own_business() {
        assert!(DeliveryState::Pending.awaits_automatic_work());
        for parked in [
            DeliveryState::KnownUnsent,
            DeliveryState::Uncertain { acknowledged_chunks: 0 },
            DeliveryState::Uncertain { acknowledged_chunks: 2 },
            DeliveryState::Delivered,
        ] {
            assert!(!parked.awaits_automatic_work(), "{parked:?} would be retried unattended");
        }
        assert!(!DeliveryState::Pending.repair().requires_operator());
        assert!(DeliveryState::KnownUnsent.repair().requires_operator());
        assert!(DeliveryState::Uncertain { acknowledged_chunks: 9 }.repair().requires_operator());
        assert!(!DeliveryState::Delivered.repair().requires_operator());
    }

    #[test]
    fn every_state_has_a_distinct_stable_tag() {
        // `Uncertain` shares one tag across widths on purpose: the width is a separate column.
        let tags: Vec<&str> = all().iter().map(|s| s.tag()).collect();
        assert!(tags.contains(&"pending") && tags.contains(&"delivered"));
        assert_eq!(DeliveryState::Uncertain { acknowledged_chunks: 7 }.tag(), "uncertain");
    }
}
