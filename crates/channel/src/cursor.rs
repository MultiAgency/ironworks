//! The cursor: a **query**, never a stored truth.
//!
//! # The one design decision in this file
//!
//! A long-poll channel is told an offset, and everything below it stops being redelivered. That
//! offset is the only thing standing between a crash and a client seeing an answer twice, so
//! where it comes from matters more than almost anything else in the system.
//!
//! The reference implementation *stores* it, which creates a second copy of a fact already
//! implied by the update rows — and therefore a way for the two to disagree. It closes that with
//! a transaction: the delivered state and the advanced cursor commit together or neither does.
//! Its own test says so plainly: *"Boundary 6 cannot exist, and this proves it rather than
//! asserting it in prose."*
//!
//! Deriving the cursor instead removes the boundary rather than defending it. **There is no
//! write to be non-atomic with.** B6 is not enforced here; it is absent. The channel journal
//! exposes no method that sets an offset, and [`safe_cursor`] is a pure function of the rows, so
//! a state where the cursor has advanced past an unfinished update is not something this code
//! declines to produce — it is something it cannot represent.
//!
//! # What the barrier actually is
//!
//! > The safe cursor is the **lowest open update**, across every tenant.
//!
//! One line, and it is the whole cross-tenant guarantee. A fast tenant finishing update 11 while
//! a slow tenant is still working update 10 cannot move the offset past 10, because 10 is still
//! open and `min` does not care which tenant it belongs to. The same line gives graceful
//! shutdown for free: a process that finishes the update in hand and stops leaves the rest of the
//! batch open, so the cursor stays behind them and the next process is sent them again.
//!
//! # The update that could not be read
//!
//! [`Openness::Unestablished`] holds the barrier exactly as `Open` does, so "lowest open" is
//! really *lowest update the offset may not pass*. It is the same `min` and the same line; the
//! only change is that a row nobody could read counts as one of them. An unreadable row is not
//! evidence that a turn finished, and the offset is the one thing that stops the channel
//! redelivering — so passing it would trade an answer for a read error.

use crate::UpdateId;
use ironworks_domain::Openness;

/// The offset the channel may safely be told: everything below it is finished and may stop being
/// redelivered.
///
/// `None` means nothing has been journalled yet, which is not the same as zero — there is no
/// offset to send, rather than an offset of "the beginning".
///
/// `updates` need not be sorted, and duplicate ids are harmless. Both are properties worth having
/// because the caller is a database query whose ordering is not this function's business.
pub fn safe_cursor(updates: &[(UpdateId, Openness)]) -> Option<UpdateId> {
    let lowest_held = updates.iter().filter(|(_, o)| o.holds_barrier()).map(|(id, _)| *id).min();
    match lowest_held {
        // The barrier. Not `min` of *this tenant's* open work — `min` of everyone's, and an
        // update whose progress could not be read is one of them.
        Some(id) => Some(id),
        // Nothing outstanding: acknowledge through the highest update seen. `+1` is the channel's
        // convention — an offset names the next update wanted, not the last one handled.
        None => updates.iter().map(|(id, _)| *id).max().map(UpdateId::next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironworks_domain::Openness::{Open, Settled, Unestablished};

    fn u(n: u64) -> UpdateId {
        UpdateId::new(n)
    }

    #[test]
    fn an_empty_journal_has_no_offset_to_send() {
        // Not `Some(0)`. There is a difference between "acknowledge nothing" and "acknowledge
        // from the beginning", and only one of them is a sentence a channel understands.
        assert_eq!(safe_cursor(&[]), None);
    }

    #[test]
    fn a_finished_run_acknowledges_past_the_last_update() {
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(2), Settled)]), Some(u(3)));
    }

    /// The cross-tenant barrier, in the shape the reference corpus tests it: a later update from
    /// a faster tenant finishes first, and must not carry the offset past the earlier one.
    #[test]
    fn a_faster_tenant_cannot_acknowledge_a_slower_tenants_unfinished_work() {
        // Update 10 belongs to tenant alpha and is still running; 11 belongs to beta and is done.
        assert_eq!(safe_cursor(&[(u(10), Open), (u(11), Settled)]), Some(u(10)));
        // Only when alpha finishes does the offset clear both.
        assert_eq!(safe_cursor(&[(u(10), Settled), (u(11), Settled)]), Some(u(12)));
    }

    /// A crash on the third of three must leave the first two acknowledged: a batch is not a unit
    /// of durability. This is the defect the reference suite opens with.
    #[test]
    fn progress_within_a_batch_survives_a_crash_in_it() {
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(2), Settled), (u(3), Open)]), Some(u(3)));
    }

    /// A SIGTERM after the first update: the rest stay open and the next process is sent them
    /// again. Nothing in this file knows what a graceful shutdown is; it falls out of `min`.
    #[test]
    fn a_graceful_stop_leaves_the_rest_of_the_batch_unacknowledged() {
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(2), Open), (u(3), Open)]), Some(u(2)));
    }

    /// A settled-but-unsuccessful update must not hold the offset. If it did, one turn parked for
    /// operator reconciliation would deafen the bridge to every conversation behind it — and the
    /// process would look perfectly healthy while doing it.
    #[test]
    fn a_parked_update_does_not_deafen_the_channel() {
        // 5 is parked (settled, not delivered); 6 and 7 are ordinary.
        assert_eq!(safe_cursor(&[(u(5), Settled), (u(6), Settled), (u(7), Open)]), Some(u(7)));
    }

    #[test]
    fn order_and_duplicates_do_not_change_the_answer() {
        let shuffled = [(u(11), Settled), (u(9), Settled), (u(10), Open), (u(10), Open)];
        assert_eq!(safe_cursor(&shuffled), Some(u(10)));
    }

    /// An update whose progress could not be read holds the barrier, and the control is the same
    /// three rows answered.
    ///
    /// Without the second assertion a `safe_cursor` that refused to pass *anything* would pass
    /// this test while being useless, so the control is what gives the first assertion content.
    #[test]
    fn an_update_that_could_not_be_read_holds_the_barrier() {
        assert_eq!(
            safe_cursor(&[(u(5), Settled), (u(6), Unestablished), (u(7), Settled)]),
            Some(u(6)),
            "the offset passed an update nothing could be read about"
        );
        // POSITIVE CONTROL: the same three rows, read. The barrier is not permanent — it belongs
        // to the failed read, and it lifts when the read succeeds.
        assert_eq!(safe_cursor(&[(u(5), Settled), (u(6), Settled), (u(7), Settled)]), Some(u(8)));
    }

    /// An unreadable row does not beat an earlier open one, and does not hide behind a later one.
    /// It is one more member of the `min`, not a special case beside it.
    #[test]
    fn an_unreadable_row_is_ordered_with_the_open_ones_not_against_them() {
        assert_eq!(safe_cursor(&[(u(3), Open), (u(4), Unestablished)]), Some(u(3)));
        assert_eq!(safe_cursor(&[(u(3), Unestablished), (u(4), Open)]), Some(u(3)));
    }

    /// The over-correction control. `Unestablished` must not leak into the "nothing outstanding"
    /// branch: a journal where every row failed to read has no acknowledgeable offset at all, and
    /// answering `max + 1` there would acknowledge the entire batch on no evidence whatsoever.
    #[test]
    fn a_journal_that_answered_nothing_acknowledges_nothing() {
        assert_eq!(safe_cursor(&[(u(1), Unestablished), (u(2), Unestablished)]), Some(u(1)));
        // POSITIVE CONTROL: the same two rows, answered — `max + 1` is reachable, so the
        // assertion above is about `Unestablished` and not about the branch being dead.
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(2), Settled)]), Some(u(3)));
    }

    /// The gap case: ids from a channel are not dense. An offset past a hole is still correct,
    /// because the offset names what is wanted next, not how many were seen.
    #[test]
    fn a_gap_in_the_ids_is_not_a_hole_in_the_cursor() {
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(97), Settled)]), Some(u(98)));
        assert_eq!(safe_cursor(&[(u(1), Settled), (u(97), Open)]), Some(u(97)));
    }
}
