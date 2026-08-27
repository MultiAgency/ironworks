//! Which tenant an inbound update belongs to.
//!
//! # Routing is not addressing
//!
//! Routing asks *whose conversation is this?* Addressing asks *is this message for me?* They are
//! separate questions and conflating them is the origin of one of the corpus's worst defects
//! (E-4): the current implementation returns the same "no" for "not a registered conversation"
//! and "not addressed to us", so an update already in flight — with a possibly-billed answer —
//! whose tenant had left the registry was recorded as *not addressed to us*, its cursor advanced
//! past it, and it became unrecoverable.
//!
//! **Routing decides whether to answer. It cannot decide what already happened.** This module
//! answers only the first question, and returns [`RouteState::Ambiguous`] rather than guessing.

use crate::ids::Slug;
use std::fmt;

/// An opaque channel address — a group id, a room, a mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelRef(String);

impl ChannelRef {
    pub fn new(addr: impl Into<String>) -> Self {
        Self(addr.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of room an update arrived in, **as the channel attested it**.
///
/// Three-valued for the reason [`crate::AttemptOutcome`] is: "the channel said nothing" is a
/// different fact from "the channel said this is a group", and collapsing them is what would let
/// an adapter that has not been taught to report chat kind quietly admit a group.
///
/// The distinction only has to be sharp enough for the one decision that turns on it — whether a
/// message is a one-to-one conversation with the bot — so there are two attested values and not a
/// taxonomy of Telegram's four chat types. A room this cannot tell apart from a group is a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    /// The channel attested a one-to-one conversation with the bot.
    Private,
    /// The channel attested something else: a group, a supergroup, a broadcast channel.
    NonPrivate,
    /// The channel made no claim. **Never read as [`Private`](Self::Private).**
    ///
    /// An older adapter, an update carrying no chat, or a chat type this build does not
    /// recognise all land here, and every one of them is a room whose privacy is unestablished.
    Unattested,
}

impl ChatKind {
    /// Whether the channel *proved* this is a one-to-one conversation.
    ///
    /// The whole type in one predicate, so no call site has to remember that `Unattested` is not
    /// a maybe. Both other variants answer `false`.
    pub fn is_attested_private(self) -> bool {
        matches!(self, Self::Private)
    }
}

/// Why a channel could not be routed to exactly one tenant.
///
/// Ambiguity is reported, never resolved by picking. The current implementation builds a
/// `{channel: tenant}` map, so a duplicate silently keeps the last entry and serves that whole
/// conversation with **the wrong tenant's credentials and data** (B-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAmbiguity {
    /// More than one tenant claims this channel.
    MultipleTenants { channel: ChannelRef, tenants: Vec<Slug> },
}

impl fmt::Display for RouteAmbiguity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleTenants { channel, tenants } => {
                let names: Vec<&str> = tenants.iter().map(Slug::as_str).collect();
                write!(
                    f,
                    "channel {channel} is claimed by {} tenants ({}) — one conversation must map \
                     to exactly one tenant, or messages misroute across tenants",
                    tenants.len(),
                    names.join(", ")
                )
            }
        }
    }
}

/// Three-valued, deliberately. `Option` cannot express the third case, and the third case is the
/// one that misroutes a tenant's private records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteState {
    Present(Slug),
    Absent,
    Ambiguous(RouteAmbiguity),
}

impl RouteState {
    /// The tenant, if routing was unambiguous. `Ambiguous` yields `None` exactly as `Absent`
    /// does — a caller that only wants the happy path cannot accidentally treat a collision as a
    /// match.
    pub fn tenant(&self) -> Option<&Slug> {
        match self {
            Self::Present(s) => Some(s),
            Self::Absent | Self::Ambiguous(_) => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous(_))
    }
}

/// Resolve a channel against a routing table.
///
/// Takes a slice of claims rather than a map on purpose: a map has already lost the collision by
/// the time you can look anything up in it.
pub fn resolve(claims: &[(ChannelRef, Slug)], channel: &ChannelRef) -> RouteState {
    let mut matched: Vec<&Slug> =
        claims.iter().filter(|(c, _)| c == channel).map(|(_, s)| s).collect();
    matched.sort();
    matched.dedup();
    match matched.len() {
        0 => RouteState::Absent,
        1 => RouteState::Present(matched[0].clone()),
        _ => RouteState::Ambiguous(RouteAmbiguity::MultipleTenants {
            channel: channel.clone(),
            tenants: matched.into_iter().cloned().collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_the_channel_did_not_describe_is_never_private() {
        // The one that matters. An adapter that has not been taught to report chat kind, an
        // update carrying no chat at all, and a type this build cannot read all arrive as
        // `Unattested` — and a share command admitted from any of them would be a group message
        // treated as a private one.
        assert!(ChatKind::Private.is_attested_private());
        assert!(!ChatKind::NonPrivate.is_attested_private());
        assert!(!ChatKind::Unattested.is_attested_private());
    }

    fn slug(s: &str) -> Slug {
        Slug::parse(s).unwrap()
    }
    fn ch(s: &str) -> ChannelRef {
        ChannelRef::new(s)
    }

    fn table() -> Vec<(ChannelRef, Slug)> {
        vec![(ch("-1001"), slug("acme")), (ch("-1002"), slug("globex"))]
    }

    #[test]
    fn a_single_claim_routes() {
        assert_eq!(resolve(&table(), &ch("-1001")), RouteState::Present(slug("acme")));
    }

    #[test]
    fn an_unclaimed_channel_is_absent() {
        assert_eq!(resolve(&table(), &ch("-999")), RouteState::Absent);
    }

    /// The case a map cannot represent: it would silently keep one of them.
    #[test]
    fn two_tenants_claiming_one_channel_is_ambiguous_not_a_match() {
        let mut t = table();
        t.push((ch("-1001"), slug("rival")));
        let r = resolve(&t, &ch("-1001"));
        assert!(r.is_ambiguous());
        assert_eq!(r.tenant(), None, "a collision must never present as a match");
        assert!(r.to_string_lossy().contains("misroute"));
    }

    #[test]
    fn the_same_tenant_claiming_twice_is_not_ambiguous() {
        // A duplicated identical claim is a configuration wart, not a misrouting hazard.
        let mut t = table();
        t.push((ch("-1001"), slug("acme")));
        assert_eq!(resolve(&t, &ch("-1001")), RouteState::Present(slug("acme")));
    }

    #[test]
    fn ambiguity_names_every_claimant() {
        let mut t = table();
        t.push((ch("-1001"), slug("rival")));
        match resolve(&t, &ch("-1001")) {
            RouteState::Ambiguous(RouteAmbiguity::MultipleTenants { tenants, .. }) => {
                assert_eq!(tenants, vec![slug("acme"), slug("rival")]);
            }
            other => panic!("{other:?}"),
        }
    }

    // Small helper so the display assertion above reads cleanly.
    impl RouteState {
        fn to_string_lossy(&self) -> String {
            match self {
                Self::Ambiguous(a) => a.to_string(),
                _ => String::new(),
            }
        }
    }
}
