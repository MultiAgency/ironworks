//! The operator-facing question, answered without changing anything.
//!
//! > **What did IronWorks intend, what did it record doing, what does the outside world currently
//! > report, and where do those disagree?**
//!
//! Four axes, kept apart on the way in and on the way out:
//!
//! | Axis | Answers | Source |
//! |---|---|---|
//! | [`InstanceSpec`] | what was asked for | the operator |
//! | [`ProvisionProgress`] | what IronWorks recorded reaching | our own store |
//! | [`AuthorityInventory`] | what exists | the authoritative systems |
//! | [`ServingObservation`] | whether anything has loaded it | a serving process |
//!
//! # The governing invariant
//!
//! > Lifecycle inspection distinguishes intent, recorded progress, and observed authority;
//! > disagreement is **surfaced**, never normalized away.
//!
//! Normalizing is the easy failure and it has two shapes, both of which this refuses. Inferring
//! *managed* from *exists* hides authority nobody accounted for — the reference found an instance
//! holding 43 members, 41 unknown to its registry, accumulated over weeks with nothing reporting
//! it because nothing was looking. Inferring *absent* from *our records say nothing* is worse: it
//! is how a tool mints a second live credential and leaves the first authenticating.

use crate::attempt::{Blocked, ConfinementAttempt, Historical};
use crate::authority::{Authority, Custody, Holding};
use crate::confinement::{Confinement, PolicyBinding};
use crate::runtime::RuntimeSubject;
use crate::state::{InstanceSpec, ProvisionProgress, ServingObservation, Stage};
use crate::{AuthorityInventory, BlockedReason, OpaqueId, holdings};
use ironworks_domain::NonEmptyReason;
use std::fmt;

/// Whether the store this history came from can hold observations at all.
///
/// A v1 store predates the ledger; a v2 store with no rows can hold one and holds none. Both are
/// BLOCKED, and they owe an operator different sentences — telling someone "no observation" about a
/// store that has nowhere to put one sends them looking for the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    PredatesLedger,
    Present,
}

/// What a live observation found, if one was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveConfinement {
    /// The credential authenticated as the recorded member, and this is what its catalogue said.
    Bound { subject: RuntimeSubject, confinement: Confinement },
    /// The credential authenticated as somebody else. Measured, and therefore determinate.
    Mismatch { expected: OpaqueId, authenticated: RuntimeSubject },
    /// Nothing was established.
    Blocked(Blocked),
}

/// The confinement axis of an inspection.
///
/// `latest` is the latest attempt about the **currently recorded** member, never simply the newest
/// row. A previous member's history stays visible in `confinement_history` and has no bearing on
/// the current verdict: replacement over time is legitimate, not a contradiction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfinementAxis {
    /// The command did not ask about confinement, so nothing was read and nothing is claimed.
    ///
    /// **It contributes nothing to the verdict**, and that is deliberate rather than lenient: an
    /// inspection that reported BLOCKED for a question nobody asked would degrade every existing
    /// invocation for a reason about our instrumentation. The axis still renders a line saying it
    /// was not asked, so a reader is never left to infer confinement from silence.
    NotRequested,
    /// History was read; no live observation was made. Historical evidence alone.
    Recorded { ledger: Ledger, latest: Option<ConfinementAttempt> },
    /// History, plus a live observation to compare it with.
    ///
    /// `live` is boxed so this variant does not dwarf the others: `LiveConfinement` carries a whole
    /// [`Confinement`], and an axis that is usually `NotRequested` should not pay for the case that
    /// measured.
    Measured {
        ledger: Ledger,
        latest: Option<ConfinementAttempt>,
        live: Box<LiveConfinement>,
        current: PolicyBinding,
    },
}

impl ConfinementAxis {
    fn ledger(&self) -> Option<Ledger> {
        match self {
            Self::NotRequested => None,
            Self::Recorded { ledger, .. } | Self::Measured { ledger, .. } => Some(*ledger),
        }
    }
    fn latest(&self) -> Option<&ConfinementAttempt> {
        match self {
            Self::NotRequested => None,
            Self::Recorded { latest, .. } | Self::Measured { latest, .. } => latest.as_ref(),
        }
    }
}

/// Whether confinement is a question worth asking about this instance yet.
///
/// A pre-provisioning inspection must not be degraded merely because a member does not exist: there
/// is nothing to confine, and reporting BLOCKED would make every early instance look unhealthy for
/// a reason about our sequencing rather than about the tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Applicability {
    /// No member recorded and `member_minted` not reached. **No effect on the verdict.**
    NotYet,
    /// The stage was reached and no identifier was recorded, so there is nothing to bind against.
    NoRecordedMember,
    Applicable(OpaqueId),
}

fn applicability(recorded: &crate::Recorded) -> Applicability {
    match recorded.accounted_id(Authority::Member) {
        Some(member) => Applicability::Applicable(member.clone()),
        None if recorded.progress.has_reached(Stage::MemberMinted) => {
            Applicability::NoRecordedMember
        }
        None => Applicability::NotYet,
    }
}

/// What an inspection concluded.
///
/// Four outcomes, and the point of having four is that the last two are not the first two. A
/// check that could not run is never a PASS (§F-14), and authority nobody has accounted for is
/// never silently a PASS either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Everything IronWorks recorded is accounted for, nothing unmanaged, nothing unmeasured.
    Pass,
    /// A determinate contradiction: the world and our records disagree about something that
    /// cannot be true both ways.
    Fail,
    /// Promotion is blocked. Something could not be measured, or something exists that nobody
    /// accounted for.
    ///
    /// This matches the reference's own mapping: outstanding authority exits **3**, not 2 —
    /// nothing is broken, but the state cannot be certified clean.
    Blocked,
    /// An operator has explicitly classified a known risk, **with a recorded reason**.
    ///
    /// The reason lives in the variant, so this cannot be produced by anything that has not
    /// written one down — *a waiver nobody can account for later is the same as no waiver*.
    ///
    /// **Nothing in this slice constructs it.** The residual-authority ledger is where operator
    /// classifications live and it is not built yet. It is defined here so the shape of a verdict
    /// is settled before something needs it, and specifically so that unmanaged authority is
    /// never normalized into it — that would be exactly the "make a gate green by forgetting"
    /// move §F-9 forbids.
    AcceptedRisk(NonEmptyReason),
}

impl Verdict {
    /// Whether this may promote. `Blocked` and `Fail` may not; an explicitly accepted risk may,
    /// and stays visibly different from `Pass` in every report.
    pub fn promotable(&self) -> bool {
        matches!(self, Self::Pass | Self::AcceptedRisk(_))
    }

    /// The operator convention already used by `dev/prove.sh`: 0 / 2 / 3.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Pass | Self::AcceptedRisk(_) => 0,
            Self::Fail => 2,
            Self::Blocked => 3,
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Blocked => "BLOCKED",
            Self::AcceptedRisk(_) => "ACCEPTED_RISK",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AcceptedRisk(reason) => write!(f, "ACCEPTED_RISK ({reason})"),
            other => f.write_str(other.tag()),
        }
    }
}

/// One thing worth an operator's attention, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub authority: Option<Authority>,
    pub note: String,
}

/// A complete answer, with every axis still separate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub spec: InstanceSpec,
    pub progress: ProvisionProgress,
    pub holdings: Vec<Holding>,
    pub serving: ServingObservation,
    /// What confinement reported, rendered separately from the authority inventory so a clean
    /// inventory cannot mask a blocked confinement or the reverse.
    pub confinement: ConfinementReport,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

/// The confinement axis as an operator reads it: what history says, what the live state is, and
/// which of the two produced the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementReport {
    /// `None` when confinement is not yet applicable.
    pub outcome: Option<ConfinementOutcome>,
    pub lines: Vec<String>,
}

/// What the confinement axis contributed. Derived; nothing stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfinementOutcome {
    /// Live, bound, drift-free, **and** a durable baseline for the policy in force.
    Pass,
    /// A measured contradiction: the wrong subject, or reachable authority outside policy.
    Fail,
    /// Anything unmeasured, and every history shape that is not a current-policy baseline.
    Blocked,
}

/// Inspect one instance. **Pure** — it reads four values and returns a conclusion, so it cannot
/// mutate anything, reconcile anything, or adopt anything by construction.
/// Decide what the confinement axis contributes, and say why in words.
///
/// # The two halves of a PASS
///
/// A live, bound, drift-free measurement is necessary and **not sufficient**. The durable half must
/// also be there: the latest attempt about this member must be a clean measurement taken against
/// the policy in force now. `inspect` is read-only and cannot append the measurement it just made,
/// so certifying on the live half alone would be a judgment with nothing durable behind it.
///
/// The remedy is one idempotent command — `lifecycle confine` — which appends a clean measurement
/// when the settings already match, so this never leaves an operator without a next step.
fn assess_confinement(axis: &ConfinementAxis, applicability: &Applicability) -> ConfinementReport {
    if matches!(axis, ConfinementAxis::NotRequested) {
        return ConfinementReport {
            outcome: None,
            lines: vec![
                "not asked — this inspection made no confinement claim. Re-run with \
                 --confinement to measure the member's catalogue and compare it with history."
                    .into(),
            ],
        };
    }
    let member = match applicability {
        Applicability::NotYet => {
            return ConfinementReport {
                outcome: None,
                lines: vec![
                    "not yet applicable — no runtime member is recorded, so there is nothing to \
                     confine. This does not lower the verdict."
                        .into(),
                ],
            };
        }
        Applicability::NoRecordedMember => {
            return ConfinementReport {
                outcome: Some(ConfinementOutcome::Blocked),
                lines: vec![
                    "BLOCKED — member_minted was reached and no member id was recorded, so there \
                     is nothing to bind a measurement against."
                        .into(),
                ],
            };
        }
        Applicability::Applicable(member) => member,
    };

    let mut lines = Vec::new();
    let historical = axis.latest().map(ConfinementAttempt::historical);

    // ── what history says ───────────────────────────────────────────────────────────
    match (&historical, axis.ledger()) {
        (None, Some(Ledger::PredatesLedger)) => lines.push(
            "history: this store predates the confinement ledger, so no observation could have \
             been recorded — which is not the same as none having been made."
                .into(),
        ),
        (None, _) => {
            lines.push(format!("history: no confinement observation for member {member}."))
        }
        (Some(Historical::CleanAt { policy }), _) => lines.push(format!(
            "history: clean measurement against {policy}, at {}.",
            axis.latest().expect("a reading implies an attempt").at()
        )),
        (Some(Historical::DriftedAt { policy, drift }), _) => lines.push(format!(
            "history: {} drift(s) against {policy}, at {}.",
            drift.len(),
            axis.latest().expect("a reading implies an attempt").at()
        )),
        (Some(Historical::Misbound { authenticated, .. }), _) => lines.push(format!(
            "history: an attempt at {} authenticated as {authenticated}, not this member. That is \
             a past failure, not proof about the credential in use now.",
            axis.latest().expect("a reading implies an attempt").at()
        )),
        (Some(Historical::Unmeasured { reason }), _) => lines.push(format!(
            "history: the last attempt for this member measured nothing — {reason}."
        )),
    }

    // ── what is true now ────────────────────────────────────────────────────────────
    let ConfinementAxis::Measured { live, current, .. } = axis else {
        lines.push(
            "BLOCKED — no live observation was made. Stored evidence is historical and cannot \
             establish current confinement; re-run with --confinement."
                .into(),
        );
        return ConfinementReport { outcome: Some(ConfinementOutcome::Blocked), lines };
    };

    match live.as_ref() {
        // FAIL outranks everything: the measurement is about the wrong member, which invalidates
        // whatever else could be said about it.
        LiveConfinement::Mismatch { expected, authenticated } => {
            lines.push(format!(
                "FAIL — the member credential authenticates as {authenticated}; lifecycle records \
                 member {expected}. This measurement is about the wrong subject."
            ));
            ConfinementReport { outcome: Some(ConfinementOutcome::Fail), lines }
        }
        LiveConfinement::Blocked(reason) => {
            lines.push(format!("BLOCKED — nothing could be measured now: {reason}."));
            ConfinementReport { outcome: Some(ConfinementOutcome::Blocked), lines }
        }
        LiveConfinement::Bound { subject, confinement } => {
            lines.push(format!("live: bound as {subject}."));
            match confinement {
                Confinement::Drifted { facts, drift } => {
                    lines.push(format!(
                        "FAIL — {} reachable of {} catalogued, outside policy now:",
                        facts.reachable.len(),
                        facts.catalogued
                    ));
                    lines.extend(drift.iter().map(|d| format!("  {d}")));
                    ConfinementReport { outcome: Some(ConfinementOutcome::Fail), lines }
                }
                Confinement::Unverifiable(reason) => {
                    lines.push(format!("BLOCKED — the catalogue could not be read: {reason}."));
                    ConfinementReport { outcome: Some(ConfinementOutcome::Blocked), lines }
                }
                Confinement::Verified(facts) => {
                    lines.push(format!(
                        "live: {} catalogued, {} reachable, approval {} — nothing outside policy.",
                        facts.catalogued,
                        facts.reachable.len(),
                        if facts.auto_approve { "ON" } else { "off" }
                    ));
                    // The durable half. Every shape that is not a current-policy baseline blocks,
                    // and each says the same one-command remedy rather than leaving a dead end.
                    let baseline = historical.as_ref().is_some_and(|h| h.is_baseline_for(current));
                    if baseline {
                        lines.push(format!(
                            "  SCOPE: about the {} catalogued settings rows, verified live against \
                             a recorded baseline. NOT a claim that the model has no other \
                             executable authority.",
                            facts.catalogued
                        ));
                        ConfinementReport { outcome: Some(ConfinementOutcome::Pass), lines }
                    } else {
                        lines.push(format!(
                            "BLOCKED — the live state matches {current}, and no durable \
                             observation records it. Run `ironworks lifecycle confine`: it is \
                             idempotent and appends a clean measurement when settings already \
                             match."
                        ));
                        ConfinementReport { outcome: Some(ConfinementOutcome::Blocked), lines }
                    }
                }
            }
        }
    }
}

pub fn inspect(
    spec: &InstanceSpec,
    recorded: &crate::Recorded,
    inventory: &AuthorityInventory,
    serving: &ServingObservation,
    confinement: &ConfinementAxis,
) -> Inspection {
    let progress = &recorded.progress;
    let held = holdings(recorded, inventory);
    let mut findings = Vec::new();
    let mut blocked = false;
    let mut failed = false;

    for holding in &held {
        match &holding.custody {
            Custody::Managed | Custody::Adopted => {}
            Custody::KnownUnmanaged => {
                blocked = true;
                findings.push(Finding {
                    authority: Some(holding.authority),
                    note: format!(
                        "{} exists and IronWorks has no record of creating it. Reported, not \
                         adopted and not removed — deciding what it is belongs to an operator.",
                        holding.authority.tag()
                    ),
                });
            }
            Custody::Unverifiable(reason) => {
                blocked = true;
                findings.push(Finding {
                    authority: Some(holding.authority),
                    note: format!(
                        "{} could not be measured: {reason}. This is not absence — acting on it \
                         as absence is how a second credential gets created.",
                        holding.authority.tag()
                    ),
                });
            }
            Custody::Absent => {
                // Measured absent. Only a contradiction if we recorded creating it.
                if progress.has_reached(holding.authority.created_at()) {
                    failed = true;
                    findings.push(Finding {
                        authority: Some(holding.authority),
                        note: format!(
                            "IronWorks recorded reaching {}, but {} is measured absent. Our \
                             records and the world disagree.",
                            holding.authority.created_at(),
                            holding.authority.tag()
                        ),
                    });
                }
            }
        }
    }

    // Serving is reported, never blocking. Activation's guarantee is that an instance is
    // ROUTABLE; whether a process has loaded it is a separate fact with no producer yet, and
    // blocking on it would make PASS unreachable for a reason about our instrumentation rather
    // than about the tenant.
    if matches!(progress, ProvisionProgress::Activated { .. })
        && matches!(serving, ServingObservation::Unknown)
    {
        findings.push(Finding {
            authority: None,
            note: "activated (routable) — but nothing has observed a serving process loading it. \
                   Routable is not serving, and Unknown is not 'no'."
                .into(),
        });
    }

    // Confinement is a separate axis, folded in at the same precedence as everything else. It is
    // kept apart on the way in and on the way out so a clean authority inventory cannot mask a
    // blocked confinement, or the reverse.
    let report = assess_confinement(confinement, &applicability(recorded));
    match report.outcome {
        Some(ConfinementOutcome::Fail) => failed = true,
        Some(ConfinementOutcome::Blocked) => blocked = true,
        // `None` is *not yet applicable*, and it deliberately changes nothing.
        Some(ConfinementOutcome::Pass) | None => {}
    }

    // FAIL outranks BLOCKED: a measured contradiction is worse news than an unmeasured one, and
    // the operator should see the sharper fact first.
    let verdict = if failed {
        Verdict::Fail
    } else if blocked {
        Verdict::Blocked
    } else {
        Verdict::Pass
    };

    Inspection {
        spec: spec.clone(),
        progress: progress.clone(),
        holdings: held,
        serving: serving.clone(),
        confinement: report,
        verdict,
        findings,
    }
}

impl Inspection {
    /// The compact operator report. Identifiers only — the types it reads from have nowhere to
    /// put a credential, so there is nothing here to redact.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("instance   {}\n", self.spec.slug));
        out.push_str(&format!(
            "intent     org {} · service {} · channel {}\n",
            self.spec.org, self.spec.service, self.spec.channel
        ));
        out.push_str(&format!("recorded   {}\n", describe_progress(&self.progress)));
        out.push_str(&format!("serving    {}\n", describe_serving(&self.serving)));
        out.push_str("authority  (what the authoritative systems report)\n");
        for holding in &self.holdings {
            out.push_str(&format!(
                "  {:<20} {}\n",
                holding.authority.tag(),
                describe_custody(&holding.custody, &holding.presence)
            ));
        }
        // Rendered as its own block. Stored evidence and live state are separate lines on purpose:
        // a reader must be able to see which of the two produced the outcome.
        out.push_str("confinement\n");
        for line in &self.confinement.lines {
            out.push_str(&format!("  {line}\n"));
        }
        out.push_str(&format!("\nverdict    {}\n", self.verdict));
        if !self.findings.is_empty() {
            out.push('\n');
            for finding in &self.findings {
                out.push_str(&format!("  - {}\n", finding.note));
            }
        }
        out
    }
}

fn describe_progress(progress: &ProvisionProgress) -> String {
    match progress {
        ProvisionProgress::NotStarted => {
            "nothing reached — which says nothing about what exists".into()
        }
        ProvisionProgress::Provisioning { reached } => format!("reached {reached}"),
        ProvisionProgress::Activated { smoke } => {
            format!("activated (routable) — smoke passed at {}", smoke.at())
        }
    }
}

fn describe_serving(serving: &ServingObservation) -> String {
    match serving {
        ServingObservation::Serving { observed_at } => format!("observed serving at {observed_at}"),
        ServingObservation::NotServing { observed_at } => {
            format!("observed NOT serving at {observed_at}")
        }
        ServingObservation::Unknown => "unknown — nothing has observed it".into(),
    }
}

fn describe_custody(custody: &Custody, presence: &crate::Presence) -> String {
    let id = match presence {
        crate::Presence::Present { id } => format!(" [{id}]"),
        _ => String::new(),
    };
    match custody {
        Custody::Managed => format!("MANAGED{id} — exists, and we recorded creating it"),
        Custody::Adopted => {
            format!("ADOPTED{id} — exists, and we recorded observing rather than creating it")
        }
        Custody::KnownUnmanaged => {
            format!("UNMANAGED{id} — exists, and we have no record of creating it")
        }
        Custody::Absent => "absent — measured, nothing there".into(),
        Custody::Unverifiable(reason) => format!("UNVERIFIABLE — {reason}"),
    }
}

/// A reconciler that asks nobody, because nobody was supplied.
///
/// Not a convenience: it is what makes "no way to observe the world" produce `BLOCKED` instead of
/// a report that silently certifies an instance nothing checked.
pub struct NoReconciler;

impl crate::Reconcile for NoReconciler {
    fn observe(&self, _spec: &InstanceSpec) -> AuthorityInventory {
        AuthorityInventory::Blocked(BlockedReason::NoReconciler)
    }
}

#[cfg(test)]
mod confinement_tests;
