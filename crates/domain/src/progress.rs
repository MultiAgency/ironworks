//! The narrowest possible window between a channel coordinator and a tenant's work.
//!
//! # Why this trait is one method returning two values
//!
//! A channel cursor is **shared across tenants**: one inbound stream, one offset, and every
//! tenant's unfinished work constrains where it may be. So the thing that computes the cursor is
//! necessarily cross-tenant — the first object in this system that is not scoped to a single
//! verified instance.
//!
//! That makes it the natural place for a scope leak. A coordinator that could see records,
//! guidance, prompts or answers in order to decide an offset would be a cross-tenant object
//! holding tenant business context, and no amount of care at the call sites would take that
//! authority back.
//!
//! So the coordinator is given exactly one question to ask and exactly two answers to receive.
//! It cannot learn a tenant's org, its records, its persona, its question, or its answer,
//! because there is no method that returns any of them:
//!
//! > **Channel-level state may coordinate ordering across tenants, but may never acquire tenant
//! > business authority or model context.**
//!
//! This is enforced twice. Here, by the shape of the trait; and in `ironworks-channel`, by a
//! test over that crate's own `Cargo.toml` asserting it does not depend on the crates where
//! records, service definitions and the runtime transport live — so those types cannot be
//! *named* there, let alone reached.

use crate::ids::IdempotencyKey;
use std::fmt;

/// Why automatic processing stopped, in words an operator can act on.
///
/// It cannot be empty, because the value *is* the alarm. A blank reason is an alarm with no
/// content: it still shows up in a count, still fails a gate, and tells whoever is paged nothing
/// about what to do — which is worse than no alarm, because it looks like one that was handled.
/// Enforcing that at construction is cheaper than enforcing it at every write, and it means the
/// storage layer's job is only to refuse a row that lost its reason, never to invent one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptyReason(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a reason may not be blank — it is the whole content of an operator alarm")]
pub struct BlankReason;

impl NonEmptyReason {
    pub fn parse(raw: impl Into<String>) -> Result<Self, BlankReason> {
        let v = raw.into();
        // Trimmed, so a reason of spaces is rejected rather than stored as one.
        let v = v.trim().to_owned();
        if v.is_empty() { Err(BlankReason) } else { Ok(Self(v)) }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonEmptyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a turn still has automatic work outstanding.
///
/// # `Settled` is not `succeeded`
///
/// It means *this process will not act on it again without an operator*. A turn parked for
/// reconciliation is settled: the answer may never have reached the client, and a human must
/// decide. Treating it as still-open would be the more cautious-looking choice and is the wrong
/// one — the channel offset would stop advancing, and every conversation behind that one update
/// would go unanswered while the bridge looked healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Openness {
    /// Automatic work remains. The cursor may not advance past this.
    Open,
    /// No automatic work remains, for any reason — delivered, abandoned, or parked for a human.
    Settled,
}

impl Openness {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// The only thing a channel coordinator may learn about a tenant's turn.
///
/// Implemented by the durable turn journal. See the module documentation for why it has this
/// shape.
pub trait TurnProgress {
    /// `Settled` for a key that was never journalled: an update whose turn does not exist has no
    /// automatic work outstanding. The alternative — treating "unknown" as `Open` — would let a
    /// typo or a compacted row stall the cursor permanently.
    fn openness(&self, key: &IdempotencyKey) -> Openness;
}
