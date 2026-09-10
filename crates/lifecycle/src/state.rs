//! Desired configuration, and how far provisioning got. Two separate things.
//!
//! Keeping intent apart from progress is not tidiness. The reference implementation merged them
//! into stage names, and the merge is what made its two worst crash windows invisible: a stage
//! called `org_registered` reads as an assertion that a credential exists, when all it records is
//! that this tool believed it created one. Those come apart the moment an org is registered any
//! other supported way — and when they do, provisioning mints a *second* live credential and
//! leaves the first authenticating.
//!
//! So: [`InstanceSpec`] is what an operator asked for. [`ProvisionProgress`] is what IronWorks
//! reached. Neither says what exists — that is [`crate::authority`], and it is the only one
//! entitled to.

use ironworks_domain::{ChannelRef, OrgId, ServiceId, Slug};
use std::fmt;

/// What an operator asked for. **No credentials, ever** — the crate docs say why this type has
/// nowhere to put one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceSpec {
    pub slug: Slug,
    pub org: OrgId,
    pub service: ServiceId,
    pub channel: ChannelRef,
}

/// The stages provisioning passes through, in order.
///
/// `Ord` is derived from declaration order, so "has this run got past X?" is a comparison the
/// compiler checks rather than an index lookup into a list that two files must agree about. The
/// reference keeps the order in one Python list precisely because it is the contract between the
/// provisioning script and the journal; here the contract is the type.
///
/// **A `Stage::Activated` is deliberately absent.** Activation requires
/// [`SmokeEvidence`], so it is a variant of [`ProvisionProgress`] rather than a stage — which
/// makes "activated without a passing smoke" unrepresentable instead of merely disallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Every check that can be made *before* creating authority has passed.
    PreflightPassed,
    /// An Account-Service org credential exists and resolves.
    OrgRegistered,
    /// Data initialization was considered and settled.
    ///
    /// **This does not assert that rows exist.** The reference calls it `data_seeded`, and its own
    /// `else` branch sets it when there was nothing to seed: *"the org starts empty (valid: org
    /// existence is row scoping)"*. A stage records that a step was settled; whether a tenant's
    /// book holds anything is an observation, and observations live in [`crate::authority`].
    DataInitializationSettled,
    /// A sealed runtime member exists.
    MemberMinted,
    /// That member's tool surface is confined and certified.
    MemberConfined,
    /// The registry entry is written to the staging path. **Not yet servable.**
    Staged,
    /// Isolation and reachability passed *against the real credentials*.
    SmokePassed,
}

impl Stage {
    /// Every stage, in order. The single source of the sequence.
    pub const ALL: [Stage; 7] = [
        Self::PreflightPassed,
        Self::OrgRegistered,
        Self::DataInitializationSettled,
        Self::MemberMinted,
        Self::MemberConfined,
        Self::Staged,
        Self::SmokePassed,
    ];

    /// The stage after this one, or `None` at the end of the sequence — where activation, not
    /// another stage, is what comes next.
    pub fn next(self) -> Option<Stage> {
        Self::ALL.iter().copied().find(|s| *s > self)
    }

    /// The stable spelling used on disk and in operator output. One place decides it, so a stage
    /// cannot be written under one name and read under another.
    pub fn tag(self) -> &'static str {
        match self {
            Self::PreflightPassed => "preflight_passed",
            Self::OrgRegistered => "org_registered",
            Self::DataInitializationSettled => "data_initialization_settled",
            Self::MemberMinted => "member_minted",
            Self::MemberConfined => "member_confined",
            Self::Staged => "staged",
            Self::SmokePassed => "smoke_passed",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Stage> {
        Self::ALL.iter().copied().find(|s| s.tag() == tag)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// What one smoke run measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmokeOutcome {
    /// Both legs passed against the real credentials.
    Passed { isolation: String, reachability: String },
    /// A leg failed. Named, so an operator is not left to guess which.
    Failed { leg: String },
    /// The smoke could not run. **Not a failure and emphatically not a pass** — see
    /// [`crate::BlockedReason`].
    Blocked(crate::BlockedReason),
}

/// Proof that a smoke run passed.
///
/// Constructible only from [`SmokeOutcome::Passed`], and required by
/// [`ProvisionProgress::Activated`] — so an instance cannot be recorded activated without
/// something having measured that it works. Same shape as `ScopeEvidence` in `ironworks-domain`,
/// and for the same reason: evidence, never a boolean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeEvidence {
    isolation: String,
    reachability: String,
    at: String,
}

impl SmokeEvidence {
    /// `Some` only for a passing outcome. There is no other constructor.
    pub fn from_outcome(outcome: &SmokeOutcome, at: impl Into<String>) -> Option<Self> {
        match outcome {
            SmokeOutcome::Passed { isolation, reachability } => Some(Self {
                isolation: isolation.clone(),
                reachability: reachability.clone(),
                at: at.into(),
            }),
            SmokeOutcome::Failed { .. } | SmokeOutcome::Blocked(_) => None,
        }
    }
    pub fn isolation(&self) -> &str {
        &self.isolation
    }
    pub fn reachability(&self) -> &str {
        &self.reachability
    }
    pub fn at(&self) -> &str {
        &self.at
    }
}

/// How far provisioning reached. **Not** what exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionProgress {
    /// Nothing has been journalled. Says nothing whatever about the outside world — which is the
    /// entire point of [`crate::authority`] existing.
    NotStarted,
    Provisioning {
        reached: Stage,
    },
    /// The registry entry is live, so the instance is **routable**.
    ///
    /// Routable is not serving. Whether a process has loaded it is
    /// [`ServingObservation`], a separate fact on a separate axis, and it may be `Unknown`.
    Activated {
        smoke: SmokeEvidence,
    },
}

impl ProvisionProgress {
    /// The furthest stage reached, if any. `Activated` reports `SmokePassed`, because activation
    /// is past the last stage rather than one of them.
    pub fn reached(&self) -> Option<Stage> {
        match self {
            Self::NotStarted => None,
            Self::Provisioning { reached } => Some(*reached),
            Self::Activated { .. } => Some(Stage::SmokePassed),
        }
    }

    /// Whether provisioning has got past `stage`.
    pub fn has_reached(&self, stage: Stage) -> bool {
        self.reached().is_some_and(|r| r >= stage)
    }
}

/// Whether a serving process has this instance loaded.
///
/// A separate axis from [`ProvisionProgress`] on purpose. The reference activates by renaming a
/// file and then tells the operator *"restart telegram_bridge.py to pick it up (the registry is
/// read at startup)"* — so activation is a promise about the next restart, and nothing measures
/// the difference. `Unknown` is the honest default until something asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServingObservation {
    Serving {
        observed_at: String,
    },
    NotServing {
        observed_at: String,
    },
    /// Nothing has asked, or the answer could not be obtained. Never read as `NotServing`.
    Unknown,
}

impl ServingObservation {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Serving { .. } => "serving",
            Self::NotServing { .. } => "not_serving",
            Self::Unknown => "unknown",
        }
    }
}
