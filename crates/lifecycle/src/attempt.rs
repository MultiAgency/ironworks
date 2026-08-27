//! One recorded attempt to establish what a member's settings catalogue reports.
//!
//! # Facts, never a verdict
//!
//! Nothing here persists `Verified`, `Drifted`, or a `confined` boolean. What is durable is *what
//! happened*: whether the credential could be identified, whether it was the recorded member,
//! which policy the comparison was made against, and what the catalogue said. A judgment is
//! [`ConfinementAttempt::historical`], derived on read — because "confined" is a claim about a
//! moment, and §L-6's custody boundary means it can be false a second later.
//!
//! # The ladder
//!
//! [`Reached`] nests rather than repeating fields across four flat variants, and the nesting is the
//! point: each rung is a fact that had to be established before the next could be attempted. A
//! policy claim about a credential that was never authenticated is therefore not merely disallowed,
//! it is unconstructible — [`PolicyBinding`] lives under [`Reached::Bound`], which can only be
//! built from a [`BoundSubject`].
//!
//! # One canonical home per fact
//!
//! The member, the timestamp and the fingerprint are each stored exactly once, at the level that
//! owns them. [`ConfinementFacts`] is reconstructed on demand by
//! [`ConfinementAttempt::facts`] rather than persisted, so two stored copies of one value cannot
//! disagree when they are read back.

use crate::confinement::{ConfinementFacts, ConfinementPolicy, Drift, PolicyBinding, assess_parts};
use crate::runtime::{BoundSubject, ObservedAt, RuntimeSubject, SubjectMismatch};
use crate::{BlockedReason, OpaqueId};
use std::collections::{BTreeMap, BTreeSet};

/// Why an attempt could not measure, in a form that is safe to keep.
///
/// # It is payload-free on purpose
///
/// [`BlockedReason`]'s `Unreachable`, `Malformed` and `NoCredential` variants carry strings built
/// from adapter errors, and an audit of their producers found all three hazards live:
///
/// - **Up to 180 bytes of an upstream response body.** `ironworks_reconcile::MemberConfinement`'s
///   apply path interpolates `first_line(&body)` into a `Malformed` detail.
/// - **The credential handle.** Both `NoCredential` producers on the bound member — the apply path
///   and the catalogue read — and `ironworks_reconcile::MemberConfinement::authenticated_subject`
///   name `member_credential.handle()`.
/// - **A transport error whose text can carry the URL**, including any userinfo an operator put in
///   `IRONCLAW_API`. `ironworks_reconcile::Admin::get_as_member` and
///   `ironworks_reconcile::Admin::post_as_member` interpolate `ureq`'s error into `Unreachable`.
///
/// Named by symbol rather than by line, because the audit is the durable part and line numbers
/// are not: the first spelling of this list cited five offsets and three of them had already
/// drifted onto unrelated doc comments by the time anyone read it back.
///
/// So none of it is stored. The rich reason still reaches the operator's terminal at the moment of
/// failure, where it is useful and ephemeral; the ledger keeps a closed-set classification. That
/// also discharges the one-lining, capping and deterministic-truncation questions — there is
/// nothing to truncate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Blocked {
    /// The runtime did not answer: DNS, connect, TLS, or a timeout.
    RuntimeUnreachable,
    /// The credential was refused.
    CredentialRejected,
    /// It answered something this build cannot use — a non-2xx status, unparseable JSON, or a
    /// required field that was absent. Deliberately one class: all three mean *the answer could
    /// not be used*, and splitting them finer would mean parsing the strings this type exists to
    /// discard.
    UnreadableAnswer,
    /// No credential was available to ask with.
    NoCredential,
    /// A reconciler never mentioned the authority. Silence is not absence.
    NotObserved,
    /// The stage was reached but no identifier was recorded.
    NoRecordedIdentity,
    /// Nothing was accounted for, so there was nothing to compare against.
    NothingToReconcile,
    /// No reconciler was supplied at all.
    NoReconciler,
}

impl Blocked {
    /// The stable spelling the ledger stores. Exhaustive, so a new variant fails to compile here.
    pub fn tag(self) -> &'static str {
        match self {
            Self::RuntimeUnreachable => "runtime_unreachable",
            Self::CredentialRejected => "credential_rejected",
            Self::UnreadableAnswer => "unreadable_answer",
            Self::NoCredential => "no_credential",
            Self::NotObserved => "not_observed",
            Self::NoRecordedIdentity => "no_recorded_identity",
            Self::NothingToReconcile => "nothing_to_reconcile",
            Self::NoReconciler => "no_reconciler",
        }
    }

    /// Every value, so a round-trip test cannot silently miss one.
    pub const ALL: [Blocked; 8] = [
        Self::RuntimeUnreachable,
        Self::CredentialRejected,
        Self::UnreadableAnswer,
        Self::NoCredential,
        Self::NotObserved,
        Self::NoRecordedIdentity,
        Self::NothingToReconcile,
        Self::NoReconciler,
    ];

    /// Strict: an unknown tag is a corrupt row, never a default.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.tag() == tag)
    }
}

/// Total, and lossy by design: every `String` payload is dropped rather than stored.
impl From<&BlockedReason> for Blocked {
    fn from(reason: &BlockedReason) -> Self {
        match reason {
            BlockedReason::Unreachable { .. } => Self::RuntimeUnreachable,
            BlockedReason::CredentialRejected => Self::CredentialRejected,
            BlockedReason::Malformed { .. } => Self::UnreadableAnswer,
            BlockedReason::NoCredential { .. } => Self::NoCredential,
            BlockedReason::NotObserved => Self::NotObserved,
            BlockedReason::NoRecordedIdentity => Self::NoRecordedIdentity,
            BlockedReason::NothingToReconcile => Self::NothingToReconcile,
            BlockedReason::NoReconciler => Self::NoReconciler,
        }
    }
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let words = match self {
            Self::RuntimeUnreachable => "the runtime did not answer",
            Self::CredentialRejected => "the credential was refused",
            Self::UnreadableAnswer => "the runtime's answer could not be used",
            Self::NoCredential => "no credential was available to ask with",
            Self::NotObserved => "nothing mentioned it",
            Self::NoRecordedIdentity => "no identifier was recorded",
            Self::NothingToReconcile => "there was nothing to reconcile against",
            Self::NoReconciler => "nothing was asked of anyone",
        };
        f.write_str(words)
    }
}

/// What one catalogue read established. The only durable catalogue facts.
///
/// Note what is **absent**: the member, the policy fingerprint and the timestamp. Each lives
/// exactly once, higher up the attempt, so no two stored copies can disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    catalogued: usize,
    reachable: BTreeSet<String>,
    auto_approve: bool,
    drift: Vec<Drift>,
}

impl Measurement {
    /// Run one assessment over one bound subject, policy, catalogue and approval value.
    ///
    /// **The only constructor**, and it takes raw inputs rather than a finished
    /// [`ConfinementFacts`] and `Vec<Drift>`. That is the difference between a value whose drift
    /// provably belongs to its facts and a value where a caller could pair a clean catalogue with
    /// somebody else's drift list.
    pub fn from_assessment(
        policy: &ConfinementPolicy,
        bound: &BoundSubject,
        observed: &BTreeMap<String, String>,
        locked: &BTreeSet<String>,
        auto_approve: bool,
        at: &ObservedAt,
    ) -> Self {
        let (facts, drift) =
            assess_parts(policy, bound.expected(), observed, locked, auto_approve, at.as_str());
        Self {
            catalogued: facts.catalogued,
            reachable: facts.reachable,
            auto_approve: facts.auto_approve,
            drift,
        }
    }

    pub fn catalogued(&self) -> usize {
        self.catalogued
    }
    pub fn reachable(&self) -> &BTreeSet<String> {
        &self.reachable
    }
    pub fn auto_approve(&self) -> bool {
        self.auto_approve
    }
    pub fn drift(&self) -> &[Drift] {
        &self.drift
    }
    /// Whether this measurement found nothing outside policy. Derived, never stored.
    pub fn is_clean(&self) -> bool {
        self.drift.is_empty()
    }

    /// Crate-private reconstruction from the ledger. The store validates every part first and
    /// refuses the row rather than calling this with something it could not check.
    pub(crate) fn from_stored(
        catalogued: usize,
        reachable: BTreeSet<String>,
        auto_approve: bool,
        drift: Vec<Drift>,
    ) -> Self {
        Self { catalogued, reachable, auto_approve, drift }
    }
}

/// What the catalogue read produced, once a subject was bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueResult {
    Measured(Measurement),
    Blocked { reason: Blocked },
}

/// How far one attempt got.
///
/// Each variant is a rung, and the payload of each is exactly what that rung established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    /// `/session` never answered usably, so no subject is known and none is recorded.
    BindingBlocked { reason: Blocked },
    /// `/session` answered, and it is not our member. Terminal: nothing after this touched the
    /// settings surface. It carries no policy, because nothing was measured against one and
    /// recording one would imply a comparison that did not happen.
    SubjectMismatch { authenticated: RuntimeSubject },
    /// The subject is ours. Everything below is about that member.
    Bound { bound: BoundSubject, policy: PolicyBinding, catalogue: CatalogueResult },
}

/// One appended row of the confinement ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementAttempt {
    expected: OpaqueId,
    at: ObservedAt,
    reached: Reached,
}

/// The stable spelling of a factual variant. **Not a judgment** — it says which shape the row has,
/// so the enum can be rebuilt from SQL, and nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptVariant {
    BindingBlocked,
    SubjectMismatch,
    Measured,
    CatalogueBlocked,
}

impl AttemptVariant {
    pub fn tag(self) -> &'static str {
        match self {
            Self::BindingBlocked => "binding_blocked",
            Self::SubjectMismatch => "subject_mismatch",
            Self::Measured => "measured",
            Self::CatalogueBlocked => "catalogue_blocked",
        }
    }
    pub const ALL: [AttemptVariant; 4] =
        [Self::BindingBlocked, Self::SubjectMismatch, Self::Measured, Self::CatalogueBlocked];

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.tag() == tag)
    }
}

impl ConfinementAttempt {
    /// `/session` could not be resolved. No subject is known, so none is recorded.
    pub fn binding_blocked(expected: OpaqueId, at: ObservedAt, reason: Blocked) -> Self {
        Self { expected, at, reached: Reached::BindingBlocked { reason } }
    }

    /// The credential authenticates as somebody else.
    ///
    /// Infallible, and it takes the [`SubjectMismatch`] that [`BoundSubject::bind`] returns as its
    /// error — so an attempt recorded as a mismatch is one where the comparison actually failed.
    pub fn subject_mismatch(mismatch: SubjectMismatch, at: ObservedAt) -> Self {
        let (expected, authenticated) =
            (mismatch.expected().clone(), mismatch.authenticated().clone());
        Self { expected, at, reached: Reached::SubjectMismatch { authenticated } }
    }

    /// The subject is ours, and the catalogue could not be read.
    ///
    /// Infallible: [`BoundSubject`] already discharged the identity invariant, so there is nothing
    /// left to check and no way to pass a subject that was never compared.
    pub fn catalogue_blocked(
        bound: BoundSubject,
        policy: PolicyBinding,
        at: ObservedAt,
        reason: Blocked,
    ) -> Self {
        let expected = bound.expected().clone();
        Self {
            expected,
            at,
            reached: Reached::Bound {
                bound,
                policy,
                catalogue: CatalogueResult::Blocked { reason },
            },
        }
    }

    /// The subject is ours, and this is what the catalogue said.
    pub fn measured(
        bound: BoundSubject,
        policy: PolicyBinding,
        at: ObservedAt,
        measurement: Measurement,
    ) -> Self {
        let expected = bound.expected().clone();
        Self {
            expected,
            at,
            reached: Reached::Bound {
                bound,
                policy,
                catalogue: CatalogueResult::Measured(measurement),
            },
        }
    }

    pub fn expected(&self) -> &OpaqueId {
        &self.expected
    }
    pub fn at(&self) -> &ObservedAt {
        &self.at
    }
    pub fn reached(&self) -> &Reached {
        &self.reached
    }

    pub fn variant(&self) -> AttemptVariant {
        match &self.reached {
            Reached::BindingBlocked { .. } => AttemptVariant::BindingBlocked,
            Reached::SubjectMismatch { .. } => AttemptVariant::SubjectMismatch,
            Reached::Bound { catalogue: CatalogueResult::Measured(_), .. } => {
                AttemptVariant::Measured
            }
            Reached::Bound { catalogue: CatalogueResult::Blocked { .. }, .. } => {
                AttemptVariant::CatalogueBlocked
            }
        }
    }

    /// The subject the runtime reported, where one was obtained.
    pub fn authenticated(&self) -> Option<&RuntimeSubject> {
        match &self.reached {
            Reached::BindingBlocked { .. } => None,
            Reached::SubjectMismatch { authenticated } => Some(authenticated),
            Reached::Bound { bound, .. } => Some(bound.authenticated()),
        }
    }

    /// The policy this attempt was taken against, where one applies.
    pub fn policy(&self) -> Option<&PolicyBinding> {
        match &self.reached {
            Reached::Bound { policy, .. } => Some(policy),
            _ => None,
        }
    }

    pub fn measurement(&self) -> Option<&Measurement> {
        match &self.reached {
            Reached::Bound { catalogue: CatalogueResult::Measured(m), .. } => Some(m),
            _ => None,
        }
    }

    /// Rebuild the operator-facing fact view from the single canonical source of each part.
    ///
    /// Not persisted as a unit, so the copies cannot disagree — there are no copies.
    pub fn facts(&self) -> Option<ConfinementFacts> {
        let (policy, measurement) = (self.policy()?, self.measurement()?);
        Some(ConfinementFacts {
            member: self.expected.clone(),
            policy_fingerprint: policy.fingerprint().clone(),
            catalogued: measurement.catalogued(),
            reachable: measurement.reachable().clone(),
            auto_approve: measurement.auto_approve(),
            observed_at: self.at.as_str().to_owned(),
        })
    }

    /// What this attempt supports, as a reading of one moment in the past.
    ///
    /// Derived every time. Nothing stores it, and no caller can set it.
    pub fn historical(&self) -> Historical {
        match &self.reached {
            Reached::BindingBlocked { reason } => Historical::Unmeasured { reason: *reason },
            Reached::SubjectMismatch { authenticated } => Historical::Misbound {
                expected: self.expected.clone(),
                authenticated: authenticated.clone(),
            },
            Reached::Bound { catalogue: CatalogueResult::Blocked { reason }, .. } => {
                Historical::Unmeasured { reason: *reason }
            }
            Reached::Bound { policy, catalogue: CatalogueResult::Measured(m), .. } => {
                if m.is_clean() {
                    Historical::CleanAt { policy: policy.clone() }
                } else {
                    Historical::DriftedAt { policy: policy.clone(), drift: m.drift().to_vec() }
                }
            }
        }
    }
}

/// What one past attempt supports. **Derived on read; never persisted.**
///
/// Every variant is explicitly about a moment: none of them is a claim about now. Turning one of
/// these into a current judgment needs a live observation, which is what
/// `lifecycle inspect --confinement` is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Historical {
    /// Bound, measured, nothing outside policy — at that time, against that policy.
    CleanAt { policy: PolicyBinding },
    /// Bound, measured, something outside policy.
    DriftedAt { policy: PolicyBinding, drift: Vec<Drift> },
    /// The credential authenticated as somebody else.
    Misbound { expected: OpaqueId, authenticated: RuntimeSubject },
    /// Either blocked rung: nothing about the member's authority was established.
    Unmeasured { reason: Blocked },
}

impl Historical {
    /// Whether this reading is a clean measurement taken against `current`.
    ///
    /// Note what it is not: a claim about now. A baseline is the durable half of a PASS, and the
    /// live half is a separate measurement that this cannot stand in for.
    ///
    /// The baseline test, in one place: a clean measurement under a policy that has since changed
    /// is not a baseline for the policy in force now.
    pub fn is_baseline_for(&self, current: &PolicyBinding) -> bool {
        matches!(self, Self::CleanAt { policy } if policy.fingerprint() == current.fingerprint())
    }
}

#[cfg(test)]
mod tests;
