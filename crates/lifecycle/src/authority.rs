//! What the authoritative systems say exists — and the planner that reconciles it against what
//! IronWorks recorded.
//!
//! # The governing invariant
//!
//! > **Absence of local lifecycle evidence is not evidence of absence of external authority.**
//!
//! The reference proved this twice, in the two crash windows where nothing durable is written yet:
//! a process that dies after registering an org credential, or after minting a member, leaves
//! live authority and an empty journal. It also proved the consequence — provisioning that
//! consulted its own journal *"mints a SECOND live token and leaves the first authenticating.
//! That is credential accumulation manufactured by the tool meant to prevent it."*
//!
//! So the journal records what IronWorks **attempted and reached**; this module records what the
//! outside world **has**; and only the second may decide whether to create anything.
//!
//! # Three answers, not two
//!
//! [`Presence`] is `Present`, `Absent`, or `Unverifiable`. The third is not a failure to be
//! smoothed away — it is the answer that must never be read as either of the others, because both
//! misreadings are expensive in opposite directions: treating it as `Absent` creates a second
//! credential, and treating it as `Present` skips a step that never ran.

use crate::state::{InstanceSpec, ProvisionProgress, Stage};
use crate::{BlockedReason, OpaqueId};

/// The five places provisioning creates durable, authority-bearing state.
///
/// The reference's own summary says four. The code creates five: seeded tenant data is a separate
/// durable artifact with a separate reversal path — and, decisively, a separate *policy*. See
/// [`Authority::reversible_by_compensation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Authority {
    /// An Account-Service org credential that resolves to exactly one org.
    OrgRegistration,
    /// The tenant's business records in the account store.
    TenantData,
    /// A sealed runtime member.
    Member,
    /// That member's confined tool surface.
    MemberConfinement,
    /// The registry entry that makes the instance routable.
    RegistryEntry,
}

impl Authority {
    pub const ALL: [Authority; 5] = [
        Self::OrgRegistration,
        Self::TenantData,
        Self::Member,
        Self::MemberConfinement,
        Self::RegistryEntry,
    ];

    /// Which stage's completion implies IronWorks created this.
    pub fn created_at(self) -> Stage {
        match self {
            Self::OrgRegistration => Stage::OrgRegistered,
            Self::TenantData => Stage::DataInitializationSettled,
            Self::Member => Stage::MemberMinted,
            Self::MemberConfinement => Stage::MemberConfined,
            Self::RegistryEntry => Stage::Staged,
        }
    }

    /// Whether a **failed provisioning run** may undo this.
    ///
    /// `TenantData` is the one that may not, and it is a deliberate product decision rather than
    /// an omission. The reference's compensator says so in its own output: the rows *"are NOT
    /// removed here — run: ./deprovision.sh --execute --confirm <slug>"*. They are the tenant's
    /// business records; destroying them because a later step failed is worse than leaving them,
    /// and destructive removal belongs behind an explicit confirmation.
    pub fn reversible_by_compensation(self) -> bool {
        !matches!(self, Self::TenantData)
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::OrgRegistration => "org_registration",
            Self::TenantData => "tenant_data",
            Self::Member => "member",
            Self::MemberConfinement => "member_confinement",
            Self::RegistryEntry => "registry_entry",
        }
    }
}

/// What an authoritative system said about one authority. Three-valued, deliberately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// It exists. Carries the opaque identifier the system uses, never a credential.
    Present { id: OpaqueId },
    /// Measured absent, by something entitled to say so.
    Absent,
    /// **Could not be measured.** Never `Absent`, never `Present`.
    Unverifiable(BlockedReason),
}

impl Presence {
    /// Whether this may be acted on at all. `Unverifiable` may not.
    pub fn is_measured(&self) -> bool {
        !matches!(self, Self::Unverifiable(_))
    }
}

/// One observation about one authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityFact {
    pub authority: Authority,
    pub presence: Presence,
}

/// What reconciliation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityInventory {
    /// Every authority was asked about. Individual answers may still be `Unverifiable`.
    Reconciled(Vec<AuthorityFact>),
    /// Reconciliation itself could not run. Nothing may be concluded about anything.
    Blocked(BlockedReason),
}

impl AuthorityInventory {
    pub fn presence(&self, authority: Authority) -> Presence {
        match self {
            Self::Blocked(reason) => Presence::Unverifiable(reason.clone()),
            Self::Reconciled(facts) => facts
                .iter()
                .find(|f| f.authority == authority)
                .map(|f| f.presence.clone())
                // Not asked about is not absent. A reconciler that omits an authority has told us
                // nothing about it, and silence is the one thing that must not read as `Absent`.
                .unwrap_or(Presence::Unverifiable(BlockedReason::NotObserved)),
        }
    }
}

/// Whether IronWorks accounts for authority that exists.
///
/// Three of these four are **holdings** — something that is, or may be, authority in the world:
/// [`Self::Managed`], [`Self::KnownUnmanaged`], [`Self::Unverifiable`]. [`Self::Absent`] is the
/// fourth state and is not a holding at all: it is the measured answer that there is nothing
/// there. Keeping it distinct from `Unverifiable` is the whole point — "we looked and found
/// nothing" and "we could not look" are different sentences.
///
/// Derived by comparing an inventory against progress — never reported by the reconciler, which
/// has no idea what IronWorks recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Custody {
    /// It exists, and IronWorks' journal accounts for it.
    Managed,
    /// **It exists and IronWorks did not record creating it.** The reference found an instance
    /// holding 43 members, 41 of which the registry had never heard of — accumulated over weeks
    /// by proof scripts that minted and never cleaned up, with nothing reporting it because
    /// nothing was looking.
    KnownUnmanaged,
    /// Measured absent.
    Absent,
    /// Whether it exists could not be determined. **Never read as either of the others** — both
    /// misreadings are expensive, in opposite directions.
    Unverifiable(BlockedReason),
}

/// One authority, what exists, and whether we account for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holding {
    pub authority: Authority,
    pub presence: Presence,
    pub custody: Custody,
}

/// Ask the authoritative systems what exists.
///
/// One method, because reconciliation has exactly one job. Implementations in this slice are
/// **fixtures only** — the point is to make the crash boundaries representable and testable
/// before any foreign system is wired in, so that when the adapters arrive they are filling a
/// shape whose semantics are already proven.
pub trait Reconcile {
    fn observe(&self, spec: &InstanceSpec) -> AuthorityInventory;
}

/// What provisioning should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    /// Record a stage IronWorks never journalled, because the authority it would create already
    /// exists. **This is P2 and P4.** Nothing is created; the fact is adopted.
    AdoptExisting { stage: Stage, authority: Authority, id: OpaqueId },
    /// Run this stage.
    Run(Stage),
    /// Every stage is done and the smoke passed. Make it routable.
    Activate,
    /// Routable already. Whether it is *serving* is a separate question this does not answer.
    Nothing,
    /// Something could not be measured. **Refuses rather than guesses**, because both guesses are
    /// expensive: assuming absent duplicates authority, assuming present skips a step.
    Blocked { reason: BlockedReason, about: Option<Authority> },
}

/// Reconcile progress against the world, and say what to do next.
///
/// Pure, so every crash boundary is exercisable without a database or a foreign system.
///
/// The ordering rule is the reference's: **resume restarts at the first stage not reached**, and
/// redoes nothing before it. What this adds is the step the reference could not take — before
/// running a stage, it asks whether the authority that stage would create already exists.
pub fn plan(progress: &ProvisionProgress, inventory: &AuthorityInventory) -> NextAction {
    if let AuthorityInventory::Blocked(reason) = inventory {
        return NextAction::Blocked { reason: reason.clone(), about: None };
    }
    if matches!(progress, ProvisionProgress::Activated { .. }) {
        return NextAction::Nothing;
    }

    for stage in Stage::ALL {
        if progress.has_reached(stage) {
            continue;
        }
        // The first stage not reached. Before running it, ask whether its authority is already
        // out there — the journal being silent is not evidence that it is not.
        let Some(authority) = Authority::ALL.iter().copied().find(|a| a.created_at() == stage)
        else {
            return NextAction::Run(stage);
        };
        return match inventory.presence(authority) {
            Presence::Present { id } => NextAction::AdoptExisting { stage, authority, id },
            Presence::Absent => NextAction::Run(stage),
            Presence::Unverifiable(reason) => {
                NextAction::Blocked { reason, about: Some(authority) }
            }
        };
    }
    NextAction::Activate
}

/// Every authority, what exists, and whether IronWorks accounts for it.
///
/// This is the answer to the question lifecycle actually exists to answer: *what authority
/// currently exists for this instance, what did IronWorks create, and what remains uncertain?*
pub fn holdings(recorded: &crate::Recorded, inventory: &AuthorityInventory) -> Vec<Holding> {
    Authority::ALL
        .iter()
        .copied()
        .map(|authority| {
            let presence = inventory.presence(authority);
            let custody = match (&presence, recorded.ids.get(&authority)) {
                // We recorded creating one, and this is it.
                (Presence::Present { id }, Some(ours)) if id == ours => Custody::Managed,
                // We recorded creating one, and this is a DIFFERENT one. The stray-member case:
                // the stage was reached, so "did we get this far?" says yes — and the thing out
                // there is still not ours.
                (Presence::Present { .. }, Some(_)) => Custody::KnownUnmanaged,
                // Something exists and we never wrote down creating anything here. Whether the
                // stage was reached is not the question; identity is.
                (Presence::Present { .. }, None) => {
                    if recorded.progress.has_reached(authority.created_at()) {
                        // We reached the stage but recorded no identifier — older state, or a
                        // stage that creates nothing identifiable. We cannot claim it is ours.
                        Custody::Unverifiable(BlockedReason::NoRecordedIdentity)
                    } else {
                        Custody::KnownUnmanaged
                    }
                }
                (Presence::Absent, _) => Custody::Absent,
                (Presence::Unverifiable(reason), _) => Custody::Unverifiable(reason.clone()),
            };
            Holding { authority, presence, custody }
        })
        .collect()
}
