//! Translating what authoritative systems report into lifecycle observations.
//!
//! # The one rule
//!
//! A reconciler reports **only what the system it asked is entitled to say**, and says nothing
//! about anything else. Silence is not absence, so an authority this crate never mentions comes
//! back as [`Presence::Unverifiable`] — which blocks, correctly, because nobody asked.
//!
//! That is why a single reconciler cannot produce a clean overall verdict, and should not. The
//! Account Service knows about org credentials; it has never heard of a sealed runtime member.
//! An adapter that reported `Absent` for one would be manufacturing a fact.
//!
//! # What it must never do
//!
//! Create, change, adopt, or provision anything. Every call here is a read, and the lifecycle
//! store is not touched at all — reconcilers hand back an inventory and the caller decides.

pub mod ironclaw;
pub use ironclaw::{
    Admin, Applied, Binding, BoundMember, FleetInventory, IronclawMember, MemberConfinement,
    Reading, unaccounted_members,
};

use ironworks_domain::{CredentialRef, ScopeFailure, ScopeResolver};
use ironworks_lifecycle::{
    Authority, AuthorityFact, AuthorityInventory, BlockedReason, InstanceSpec, OpaqueId, Presence,
    Reconcile,
};
use ironworks_records::{Book, CredentialSource};

/// What the Account Service can be asked about an instance.
///
/// # What it observes, and what it cannot
///
/// It observes **org registration**, by resolving a credential server-side and seeing which org
/// comes back. That is the same question `Book::resolve` answers for the serving path, asked for
/// a different reason.
///
/// It observes **presence, not absence**, and the asymmetry is real rather than a limitation to
/// be tidied away later. "Does this credential authorise org X?" and "does org X have any
/// credential at all?" are different questions, and only the first can be asked with a credential
/// in hand. A rejected credential is evidence about *that credential*; concluding from it that no
/// registration exists is the same mistake as concluding from an empty journal that nothing was
/// created. The second question needs the operator-side identity map, which is not this service's
/// to answer.
///
/// It says nothing about tenant data, members, confinement or the registry entry. Tenant data is
/// deliberate: reading a catalog requires a `RecordScope`, which is obtainable only from a
/// verified service instance — a serving-path gate this operator-side crate should not be
/// punching through to satisfy a report.
pub struct AccountService<'a, C: CredentialSource> {
    book: &'a Book<C>,
    /// **Where** the org credential lives. Never its value — lifecycle stores no secrets, and
    /// this crate resolves the handle only at the moment of asking.
    credential: CredentialRef,
}

impl<'a, C: CredentialSource> AccountService<'a, C> {
    pub fn new(book: &'a Book<C>, credential: CredentialRef) -> Self {
        Self { book, credential }
    }

    /// The org-registration observation on its own, so a caller can compose several reconcilers
    /// without one of them deciding what the others said.
    pub fn org_registration(&self, spec: &InstanceSpec) -> Result<Presence, BlockedReason> {
        match self.book.resolve(&self.credential) {
            // A credential resolves, and to this instance's org. Authority exists here, whoever
            // created it — custody is the caller's question, decided by comparing identifiers.
            Ok(evidence) if evidence.org() == &spec.org => {
                Ok(Presence::Present { id: OpaqueId::new(evidence.org().as_str()) })
            }
            // A credential resolves, to somebody else. Reported as present with the org it
            // actually authorises, so the disagreement reaches the operator rather than being
            // flattened into "not what we expected".
            Ok(evidence) => Ok(Presence::Present { id: OpaqueId::new(evidence.org().as_str()) }),
            // The credential was refused. That is a fact about the credential, NOT about whether
            // this org has a registration — so it is unverifiable, never absent.
            Err(ScopeFailure::Unauthorized) => {
                Ok(Presence::Unverifiable(BlockedReason::CredentialRejected))
            }
            // The service could not be reached or could not be read. Nothing may be concluded
            // about anything it holds.
            Err(ScopeFailure::Unreachable { detail }) => {
                Err(BlockedReason::Unreachable { system: format!("account service ({detail})") })
            }
            Err(ScopeFailure::Malformed { detail }) => Err(BlockedReason::Malformed { detail }),
        }
    }
}

impl<C: CredentialSource> Reconcile for AccountService<'_, C> {
    fn observe(&self, spec: &InstanceSpec) -> AuthorityInventory {
        match self.org_registration(spec) {
            // Exactly one fact. The other four authorities are not mentioned, because this
            // service has never heard of them — and an unmentioned authority is unverifiable,
            // which is the honest answer.
            Ok(presence) => AuthorityInventory::Reconciled(vec![AuthorityFact {
                authority: Authority::OrgRegistration,
                presence,
            }]),
            // The service itself is unusable, so not even the one authority it owns can be
            // reported. `Blocked` is stronger than a single unverifiable fact: it says nothing
            // was learned at all.
            Err(reason) => AuthorityInventory::Blocked(reason),
        }
    }
}

/// The operator credential for the IronClaw admin surface.
///
/// A second, sealed copy of the discipline `ironworks-records` applies to record-store credentials —
/// redacted `Debug`, no `Display`, and the only way out is private. Duplicated rather than shared
/// because `book::Secret::expose` is `pub(crate)` **on purpose**, and widening it so a second
/// crate could reuse it would trade a real seal for twenty lines. (The honest fix is for
/// `CredentialSource` and `Secret` to live in `ironworks-domain` beside `CredentialRef`; that is a
/// consolidation, not this slice.)
pub struct OperatorToken(String);

impl OperatorToken {
    /// Resolves an `env:NAME` handle. Deliberately the only scheme understood: a resolver that
    /// silently accepted an unknown shape would return nothing and look like an unreachable
    /// instance.
    pub fn from_env(reference: &CredentialRef) -> Option<Self> {
        let name = reference.handle().strip_prefix("env:")?;
        std::env::var(name).ok().filter(|v| !v.is_empty()).map(Self)
    }

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for OperatorToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperatorToken(redacted)")
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ironclaw_tests;
