//! What a member's settings catalogue reports, and what IronWorks can honestly promise about it.
//!
//! # The guarantee, with its limitation attached
//!
//! > IronWorks verifies that every mutable **settings-catalogued** tool outside the allowed set is
//! > effectively `Disabled` at the exact recorded member's scope, except the two rows the runtime
//! > locks at `ask_each_time`. Ordinary settings remain enforceable only while IronWorks retains
//! > **exclusive custody of the member credential**; the two locked rows are the measured exception.
//!
//! Every clause is measured at `4cb47cfaf3`, and the limitation is not a hedge:
//!
//! - The two `builtin.operator_config_set_*` rows read `locked: true`, `mutable: false`, with state,
//!   default state and effective source fixed at locked `ask_each_time`. Writes answer `400
//!   invalid_value` and read-back is unchanged. `Locked` is not a writable state; the row metadata
//!   is nevertheless enforced.
//! - `POST /settings/tools/{cap}` writes the **caller's own** scope. An operator cannot write a
//!   member's override through it, which is why confinement authenticates *as the member* — and
//!   why the member can undo it: measured, `disabled` → `always_allow` → 200.
//! - Two tools cannot be disabled at all: the runtime answers `400 invalid_value` to a `disabled`
//!   state for `builtin.operator_config_set_tool_permission` and
//!   `builtin.operator_config_set_auto_approve`, while accepting it for every other tool. They sit
//!   permanently at `ask_each_time`. Both are API-only and were absent from the host-captured
//!   capability surface and the fresh member's directly advertised definitions. Exhaustive
//!   fresh-member deferred reachability is BLOCKED, so no model approval/resume behavior is claimed
//!   for them here.
//! - A member-scope override does beat the global default: the runtime's own comment is *"the
//!   gate honours the explicit override"*, and it was measured flipping `effective_source` from
//!   `global` to `override`.
//!
//! So ordinary confinement is a **custody-backed configuration guarantee**, not immutable policy;
//! the two locked rows are the narrow exception. Neither statement proves anything about all
//! executable authority.
//!
//! # Catalog completeness
//!
//! The claim is about the settings catalogue — **not** "the model has no other executable
//! authority". At the pin the fresh-member catalogue holds 53 rows and a host-captured capability
//! surface held 53 ids — not the same 53, overlapping in 45, with 8 catalogue-only and 8
//! surface-only. Exhaustive fresh-member deferred reachability is BLOCKED.

use crate::OpaqueId;
use std::collections::{BTreeMap, BTreeSet};

/// A tool's effective state at one member's scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolState {
    /// Invocable with no approval.
    AlwaysAllow,
    /// Not denied, but invocable only after approval: a matching one-shot lease admits the resumed
    /// invocation. Global `agent.auto_approve_tools` does not satisfy an explicit or hard-floor
    /// `ask_each_time` gate.
    AskEachTime,
    /// Refused at dispatch.
    Disabled,
    /// A state this build does not understand. Never read as `Disabled`.
    Unrecognized(String),
}

impl ToolState {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "always_allow" => Self::AlwaysAllow,
            "ask_each_time" => Self::AskEachTime,
            "disabled" => Self::Disabled,
            other => Self::Unrecognized(other.to_owned()),
        }
    }
    /// Whether this settings row is not denied. This is deliberately a catalogue-state predicate,
    /// not proof that the capability is present on a model surface. For a model-visible capability,
    /// `AskEachTime` is an approval gate rather than a denial.
    pub fn is_reachable(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// The wire spelling, so a state can be written to the ledger and read back as itself.
    ///
    /// [`Self::parse`] and this are inverses, including for [`Self::Unrecognized`] — a state this
    /// build cannot interpret is stored verbatim rather than normalised away, because normalising
    /// it would lose the one fact worth keeping about it.
    pub fn wire(&self) -> &str {
        match self {
            Self::AlwaysAllow => "always_allow",
            Self::AskEachTime => "ask_each_time",
            Self::Disabled => "disabled",
            Self::Unrecognized(raw) => raw,
        }
    }
}

/// A settings row describing an operation that can widen or modify authority.
///
/// These are ranked ahead of every other drift, and they carry an ordering rule:
///
/// > No catalogued authority-changing row should be left until after ordinary denials.
///
/// Deny these first. Five entries are API-only and absent from the measured captured/direct model
/// surfaces, but exhaustive fresh-member deferred reachability is BLOCKED. Keeping them in the
/// first group costs nothing and avoids weakening the ordering on an incomplete measurement.
/// The two tools IronClaw 1.4.0 **refuses to disable**.
///
/// Measured: `POST /settings/tools/builtin.operator_config_set_tool_permission
/// {"state":"disabled"}` answers `400 validation invalid_value` on `state`, as does its
/// auto-approve sibling. `builtin.echo` answers `200` to the identical request, so this is a rule
/// about these capabilities rather than a broken request.
///
/// They therefore remain at locked `ask_each_time`. That is not drift — it is the best settings
/// state the runtime permits. This says nothing about model reachability: both rows are API-only,
/// and exhaustive fresh-member deferred reachability is BLOCKED.
///
/// # This list is the expectation, not the source of truth
///
/// The catalogue reports `locked` per row, and [`plan`] and [`assess`] take that measured set
/// rather than this constant — see [`undisableable`]. The two agreed exactly when measured
/// (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 2), and they agree by
/// construction at this revision: `locked` is computed upstream as `default_permission == Deny ||
/// effects ∩ {Financial, ModifyApproval, ModifyBudget} ≠ ∅`, and both rows below declare
/// `ModifyApproval`.
///
/// They are kept apart because a capability that *later* gains a hard-floor effect would join the
/// runtime's locked set without joining this list, and the plan would then queue a write the
/// runtime answers `400` — which stops the whole plan, because [`Applied`] stops at the first
/// failure by design. Reading the measured set makes that bump a no-op; keeping this constant
/// beside it means a disagreement is still visible rather than absorbed.
pub const UNDISABLEABLE: &[&str] =
    &["builtin.operator_config_set_tool_permission", "builtin.operator_config_set_auto_approve"];

/// The rows this build must not attempt to write, given what the catalogue reported.
///
/// The union of the measured `locked` set and [`UNDISABLEABLE`]. A union rather than a
/// substitution: a catalogue that omits `locked` (an older runtime, a truncated read) must not
/// silently lose the two rows we have measured a `400` from, and a runtime that locks a row this
/// build has never heard of must not have it queued.
fn undisableable(locked: &BTreeSet<String>) -> BTreeSet<&str> {
    UNDISABLEABLE.iter().copied().chain(locked.iter().map(String::as_str)).collect()
}

pub const AUTHORITY_CHANGING: &[&str] = &[
    // Direct: sets tool permissions and the approval setting.
    "builtin.operator_config_set_tool_permission",
    "builtin.operator_config_set_auto_approve",
    // Replaces tenant administrator configuration groups.
    "builtin.admin_configuration_replace",
    // Installing an extension, MCP endpoint, or skill ADDS tools — new authority by definition.
    "builtin.extension_install",
    "builtin.extension_activate",
    "builtin.extension_register_hosted_mcp",
    "builtin.ironhub_install",
    "builtin.skill_install",
    "builtin.skill_update",
    "builtin.skill_auto_activate_set",
    "builtin.skill_auto_activate_learned_set",
    // Authorizes another actor to run.
    "builtin.spawn_subagent",
    // Chooses where the runtime may reach out to.
    "builtin.notification_channels_set",
    // Mint credentials and login links.
    "builtin.trace_commons.onboard",
    "builtin.trace_commons.profile_token",
    "builtin.trace_commons.account_login_link",
];

/// What the model is permitted to invoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementPolicy {
    allowed: BTreeSet<String>,
    auto_approve: bool,
}

impl ConfinementPolicy {
    /// The policy for a read-only analytical service: **nothing**.
    ///
    /// Derived from the product, not from what the reference happened to keep:
    ///
    /// - IronWorks' request body is `{model, instructions, input, previous_response_id}`. It sends
    ///   no `tools`, so it registers no caller-hosted function tools. **That is a choice, not a
    ///   limit of the surface**: `/v1/responses` accepts `tools` and `tool_choice` at the pin, and
    ///   the probe has the host ports wired
    ///   (`docs/experiments/0031-the-external-tool-lane-is-open.md`). Those are a separate lane —
    ///   the host parks such a call for the caller to execute — and this policy governs IronClaw's
    ///   built-in catalogue, which is runtime-supplied and must still be confined explicitly.
    /// - Records reach the model through [`crate::InstanceSpec`]-scoped, verified model input —
    ///   not by the model fetching them. Generic HTTP or filesystem reach adds no capability the
    ///   product needs and removes the property that every model-visible byte was gated.
    /// - The service performs no outreach, no writes, no effects, no installation and no
    ///   self-modification, so every tool granting those is authority without a purpose.
    ///
    /// An empty allow-set is therefore the *correct* answer rather than an aggressive one, and
    /// the burden is on any future tool to justify itself against a turn that needs none.
    pub fn read_only_analyst() -> Self {
        Self { allowed: BTreeSet::new(), auto_approve: false }
    }

    /// The narrow policy for a sandbox-backed client workspace.
    ///
    /// `builtin.shell` is the one IronClaw 1.4.0 capability whose production implementation
    /// crosses the user-sandbox process port. Its commands run in the persistent per-user worker,
    /// with `/workspace` writable, the system filesystem read-only, scrubbed credentials, and
    /// network access mediated by IronClaw's managed allowlist proxy. File tools alone use the
    /// virtual filesystem lane and therefore do not prove or exercise the process sandbox.
    ///
    /// Everything else remains disabled and automatic approval remains off. This is deliberately
    /// opt-in: a record-only analyst should still use [`Self::read_only_analyst`].
    pub fn sandbox_workspace() -> Self {
        Self::allowing(["builtin.shell".to_owned()], false)
    }

    /// The policy a service's own declared ceiling implies.
    ///
    /// # Why this exists rather than an operator flag
    ///
    /// `lifecycle confine --workspace` used to pick the policy directly, which made the runtime's
    /// tool authority a second, independent answer to a question the service definition already
    /// answers with `capabilities.runtime_tools`. Two answers to one question is one answer and
    /// one thing to keep in sync — and the failure is silent in the worst direction: a service
    /// telling a tenant `egress: none` while an operator confined its member with a shell that
    /// reaches §L-10's managed allowlist.
    ///
    /// So the definition is the authority and this is a total function from it. An operator
    /// choosing a wider policy than the service declares has to change the service, in a committed
    /// file, where [`ironworks_domain::CapabilityProfile::incoherence`] is waiting.
    pub fn for_profile(profile: &ironworks_domain::CapabilityProfile) -> Self {
        match profile.runtime_tools {
            ironworks_domain::ToolAccess::None => Self::read_only_analyst(),
            ironworks_domain::ToolAccess::Sandbox => Self::sandbox_workspace(),
        }
    }

    /// A policy naming specific tools. Every entry needs a reason recorded where it is
    /// constructed; there is no default that quietly allows something.
    pub fn allowing(tools: impl IntoIterator<Item = String>, auto_approve: bool) -> Self {
        Self { allowed: tools.into_iter().collect(), auto_approve }
    }

    pub fn allows(&self, tool: &str) -> bool {
        self.allowed.contains(tool)
    }
    pub fn allowed(&self) -> &BTreeSet<String> {
        &self.allowed
    }
    pub fn auto_approve(&self) -> bool {
        self.auto_approve
    }

    /// A stable identity for this policy, so evidence records *which* policy was verified.
    ///
    /// Without it a stored observation cannot be re-read after the policy changes: "confined"
    /// would silently come to mean something else. It is the only producer of a
    /// [`PolicyFingerprint`], so a stored fingerprint can only ever have come from a policy.
    pub fn fingerprint(&self) -> PolicyFingerprint {
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        for tool in &self.allowed {
            sha2::Digest::update(&mut hasher, tool.as_bytes());
            sha2::Digest::update(&mut hasher, b"\n");
        }
        sha2::Digest::update(&mut hasher, if self.auto_approve { b"auto:1" } else { b"auto:0" });
        let out = sha2::Digest::finalize(hasher);
        PolicyFingerprint(out.iter().take(8).map(|b| format!("{b:02x}")).collect())
    }
}

/// Which policy an observation was taken against.
///
/// A newtype with a private field, produced **only** by [`ConfinementPolicy::fingerprint`]. The
/// alloy digest is the same shape — sixteen hex characters — and the two must never be compared or
/// assigned to one another by accident, which is the whole reason this is not a `String`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PolicyFingerprint(String);

impl PolicyFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rebuild one read back out of the ledger.
    ///
    /// Crate-private and strict: the store is the only caller, and a value that is not sixteen
    /// lowercase hex characters is a corrupt row rather than a fingerprint. Returning `None` here
    /// is what makes the reader able to say `Corrupt` instead of quietly comparing nonsense.
    pub(crate) fn from_stored(raw: &str) -> Option<Self> {
        (raw.len() == 16 && raw.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .then(|| Self(raw.to_owned()))
    }
}

impl std::fmt::Display for PolicyFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which service's declared ceiling produced the policy an observation was taken against.
///
/// The three values travel together because a fingerprint alone cannot distinguish *the policy was
/// edited* from *the service was replaced*, and those need different operator sentences. Built from
/// the definition and the policy in one call, so a fingerprint cannot be paired with a service it
/// did not come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyBinding {
    service: ironworks_domain::ServiceId,
    version: u32,
    fingerprint: PolicyFingerprint,
}

impl PolicyBinding {
    pub fn of(
        definition: &ironworks_domain::ServiceDefinition,
        policy: &ConfinementPolicy,
    ) -> Self {
        Self {
            service: definition.id().clone(),
            version: definition.version(),
            fingerprint: policy.fingerprint(),
        }
    }
    pub fn service(&self) -> &ironworks_domain::ServiceId {
        &self.service
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn fingerprint(&self) -> &PolicyFingerprint {
        &self.fingerprint
    }

    /// Crate-private reconstruction from the ledger; the store validates every part first.
    pub(crate) fn from_stored(
        service: ironworks_domain::ServiceId,
        version: u32,
        fingerprint: PolicyFingerprint,
    ) -> Self {
        Self { service, version, fingerprint }
    }
}

impl std::fmt::Display for PolicyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{} policy {}", self.service.as_str(), self.version, self.fingerprint)
    }
}

/// One way an observed member departs from policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// A tool the policy does not allow is reachable. The ordinary case.
    Reachable { tool: String, state: ToolState },
    /// **Ranked first.** A catalogued authority-changing row is not disabled. This classification
    /// is conservatively ordered and is not proof that the row is model-visible.
    AuthorityChangingReachable { tool: String, state: ToolState },
    /// Approval is wider than policy asks.
    AutoApprove { observed: bool, expected: bool },
    /// The policy allows a tool the catalogue does not offer — a policy written against a
    /// different build. Not a security hole, but it means the policy was never fully applied.
    AllowedToolAbsent { tool: String },
    /// A state this build cannot interpret. Never counted as denied.
    Unrecognized { tool: String, raw: String },
}

impl std::fmt::Display for Drift {
    /// Written for an operator reading a terminal, so each line says what is wrong and, where it
    /// matters, why it outranks the others.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthorityChangingReachable { tool, state } => write!(
                f,
                "{tool} is {state:?} and can widen the model's own authority — no other denial \
                 on this member can be relied upon while it is reachable"
            ),
            Self::Reachable { tool, state } => write!(f, "{tool} is {state:?}, not disabled"),
            Self::AutoApprove { observed, expected } => write!(
                f,
                "automatic approval is {} where policy expects {} — it does not bypass \
                 `ask_each_time`; while on, it makes eligible globally governed tools report \
                 `always_allow`",
                if *observed { "ON" } else { "off" },
                if *expected { "ON" } else { "off" }
            ),
            Self::AllowedToolAbsent { tool } => write!(
                f,
                "policy allows {tool}, which this catalogue does not offer — the policy was \
                 written against a different build"
            ),
            Self::Unrecognized { tool, raw } => {
                write!(f, "{tool} reports {raw:?}, a state this build cannot read as denied")
            }
        }
    }
}

impl Drift {
    /// Sort key: authority-changing first, then reachable tools, then settings.
    pub fn severity(&self) -> u8 {
        match self {
            Self::AuthorityChangingReachable { .. } => 0,
            Self::Reachable { .. } => 1,
            Self::Unrecognized { .. } => 2,
            Self::AutoApprove { .. } => 3,
            Self::AllowedToolAbsent { .. } => 4,
        }
    }

    /// The stable spelling the ledger stores, so a drift cannot be written under one name and read
    /// under another. One place decides it, the way [`crate::Stage::tag`] does.
    ///
    /// Exhaustively matched, so adding a variant fails to compile here rather than silently
    /// becoming unstorable.
    pub fn kind_tag(&self) -> &'static str {
        match self {
            Self::AuthorityChangingReachable { .. } => "authority_changing_reachable",
            Self::Reachable { .. } => "reachable",
            Self::Unrecognized { .. } => "unrecognized",
            Self::AutoApprove { .. } => "auto_approve",
            Self::AllowedToolAbsent { .. } => "allowed_tool_absent",
        }
    }

    /// Every tag this build can write, for the SQL `CHECK` and for a round-trip test.
    pub const KIND_TAGS: [&'static str; 5] = [
        "authority_changing_reachable",
        "reachable",
        "unrecognized",
        "auto_approve",
        "allowed_tool_absent",
    ];
}

/// What was observed, kept so a verdict can be explained months later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementFacts {
    /// The member this was measured against — the recorded one, never a guess.
    pub member: OpaqueId,
    pub policy_fingerprint: PolicyFingerprint,
    pub catalogued: usize,
    pub reachable: BTreeSet<String>,
    pub auto_approve: bool,
    pub observed_at: String,
}

/// The conclusion. Three-valued, for the same reason [`crate::Presence`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    /// Every catalogued tool outside the allowed set is `Disabled`, and approval matches policy.
    ///
    /// **Custody-backed.** It says nothing about what a holder of the member credential could
    /// change a moment later, and nothing about executable authority outside the catalogue.
    Verified(ConfinementFacts),
    /// Measured departure from policy. A determinate contradiction, not an unmeasured one.
    Drifted { facts: ConfinementFacts, drift: Vec<Drift> },
    /// The effective authority could not be established. Never read as either of the others.
    Unverifiable(crate::BlockedReason),
}

impl Confinement {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified(_))
    }
}

/// One write confinement must perform, in the order it must be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Turn automatic approval off. **First, always.**
    ///
    /// While it is on, eligible tools governed by the global setting are effectively
    /// `always_allow`; turning it off moves them to `ask_each_time` before individual denials are
    /// written
    /// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/reborn_services.rs:1906-1910`).
    /// The two locked API-only rows remain at their settings-catalogue `ask_each_time` floor in
    /// either global state; this enum does not claim they are model-reachable.
    ApprovalOff,
    /// Disable one tool.
    Disable { tool: String },
}

/// What must be written to bring a member to policy, ordered so the work cannot be undone by the
/// thing being confined while it is in progress.
///
/// # The ordering is the whole point
///
/// §L-3: sixteen settings rows describe authority-changing operations. Five are API-only, but the
/// exhaustive fresh-member deferred surface is BLOCKED. Keep the cost-free conservative order:
/// approval off, then the authority-changing set, then everything else.
///
/// The two undisableable tools are **not** in the plan. The runtime answers `400 invalid_value`
/// to a `disabled` state for them, so attempting it would manufacture a failure that says nothing.
/// Approval-off goes first for the separate reason that it narrows tools governed by the global
/// setting before the per-tool writes begin. No model invocation semantics are inferred for the
/// two API-only locked rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    steps: Vec<Step>,
}

impl Plan {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
    pub fn len(&self) -> usize {
        self.steps.len()
    }
}

/// Build the plan from an observed catalogue.
///
/// `observed` is tool id to raw effective state, as the catalogue reported it, and `locked` is the
/// set of rows that same read reported as `locked` — the same inputs [`assess`] takes, so a plan
/// and a verdict are always built from one measurement rather than two.
///
/// Only what is not already at policy appears: applying a plan twice is a no-op, and a member
/// already confined produces an empty one.
pub fn plan(
    policy: &ConfinementPolicy,
    observed: &BTreeMap<String, String>,
    locked: &BTreeSet<String>,
    auto_approve: bool,
) -> Plan {
    let mut steps = Vec::new();
    if auto_approve != policy.auto_approve() {
        steps.push(Step::ApprovalOff);
    }

    let undisableable = undisableable(locked);
    let mut queued: BTreeSet<&str> = BTreeSet::new();

    // Authority-changing first, in the order §L-3 lists them.
    for tool in AUTHORITY_CHANGING {
        if undisableable.contains(tool) || policy.allows(tool) {
            continue;
        }
        if observed.get(*tool).is_some_and(|s| ToolState::parse(s).is_reachable()) {
            steps.push(Step::Disable { tool: (*tool).to_owned() });
            queued.insert(tool);
        }
    }

    // Then everything else the catalogue reports as reachable.
    for (tool, state) in observed {
        if undisableable.contains(tool.as_str())
            || policy.allows(tool)
            || queued.contains(tool.as_str())
            || !ToolState::parse(state).is_reachable()
        {
            continue;
        }
        steps.push(Step::Disable { tool: tool.clone() });
    }
    Plan { steps }
}

/// Compare an observed catalogue against a policy.
///
/// Pure, so every case is exercisable without a runtime. `observed` maps tool id to its raw
/// effective state string, exactly as the catalogue reported it.
pub fn assess(
    policy: &ConfinementPolicy,
    member: &OpaqueId,
    observed: &BTreeMap<String, String>,
    locked: &BTreeSet<String>,
    auto_approve: bool,
    observed_at: &str,
) -> Confinement {
    let (facts, drift) = assess_parts(policy, member, observed, locked, auto_approve, observed_at);
    if drift.is_empty() {
        Confinement::Verified(facts)
    } else {
        Confinement::Drifted { facts, drift }
    }
}

/// The same comparison, before it is folded into a two-variant conclusion.
///
/// [`assess`] is the caller-facing form and its behaviour is unchanged. This exists because the
/// durable ledger stores *facts and drift*, never a conclusion — and reading them back out of a
/// [`Confinement`] would need a match arm for `Unverifiable`, a variant this function cannot
/// produce. A dead arm on a value that decides whether a member is confined is exactly the shape
/// worth removing rather than commenting.
pub(crate) fn assess_parts(
    policy: &ConfinementPolicy,
    member: &OpaqueId,
    observed: &BTreeMap<String, String>,
    locked: &BTreeSet<String>,
    auto_approve: bool,
    observed_at: &str,
) -> (ConfinementFacts, Vec<Drift>) {
    let mut drift = Vec::new();
    let mut reachable = BTreeSet::new();
    let authority: BTreeSet<&str> = AUTHORITY_CHANGING.iter().copied().collect();
    let undisableable = undisableable(locked);

    for (tool, raw) in observed {
        let state = ToolState::parse(raw);
        if let ToolState::Unrecognized(raw) = &state {
            // Not denied, and not silently tolerated: a state we cannot read is a state we cannot
            // certify.
            drift.push(Drift::Unrecognized { tool: tool.clone(), raw: raw.clone() });
            reachable.insert(tool.clone());
            continue;
        }
        if !state.is_reachable() {
            continue;
        }
        reachable.insert(tool.clone());
        if policy.allows(tool) {
            continue;
        }
        // The settings rows the runtime will not let anyone disable — the measured `locked` set,
        // unioned with the two this build has a `400` on record for. `ask_each_time` is their
        // catalogue floor. Policy requires global approval off independently; accepting the locked
        // floor is not a claim that any of these capabilities is model-reachable.
        if undisableable.contains(tool.as_str()) && state == ToolState::AskEachTime && !auto_approve
        {
            continue;
        }
        drift.push(if authority.contains(tool.as_str()) {
            Drift::AuthorityChangingReachable { tool: tool.clone(), state }
        } else {
            Drift::Reachable { tool: tool.clone(), state }
        });
    }

    for tool in policy.allowed() {
        if !observed.contains_key(tool) {
            drift.push(Drift::AllowedToolAbsent { tool: tool.clone() });
        }
    }
    if auto_approve != policy.auto_approve() {
        drift.push(Drift::AutoApprove { observed: auto_approve, expected: policy.auto_approve() });
    }

    // Sorted unconditionally: an empty list sorts to itself, and doing it here rather than in one
    // branch keeps the order a property of the assessment instead of a property of the verdict.
    drift.sort_by_key(Drift::severity);
    let facts = ConfinementFacts {
        member: member.clone(),
        policy_fingerprint: policy.fingerprint(),
        catalogued: observed.len(),
        reachable,
        auto_approve,
        observed_at: observed_at.to_owned(),
    };
    (facts, drift)
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    /// No row reported `locked`. The default for a fixture not exercising the locked set:
    /// [`undisableable`] unions it with [`UNDISABLEABLE`], so the two rows the runtime refuses are
    /// still skipped and these tests keep asserting exactly what they asserted before.
    fn unlocked() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn locked(tools: &[&str]) -> BTreeSet<String> {
        tools.iter().map(|t| (*t).to_owned()).collect()
    }

    fn open_catalogue() -> BTreeMap<String, String> {
        let mut c = BTreeMap::new();
        // A fresh member: everything reachable, including the two that cannot be disabled.
        for tool in AUTHORITY_CHANGING {
            c.insert((*tool).to_owned(), "always_allow".to_owned());
        }
        for tool in ["builtin.http", "builtin.write_file", "ironclaw.memory.write", "builtin.time"]
        {
            c.insert(tool.to_owned(), "always_allow".to_owned());
        }
        c
    }

    #[test]
    fn approval_goes_off_before_anything_else() {
        // Global automatic approval makes eligible globally governed tools `always_allow`.
        // Narrow that ambient set to `ask_each_time` before writing individual denials.
        let p = plan(&ConfinementPolicy::read_only_analyst(), &open_catalogue(), &unlocked(), true);
        assert_eq!(p.steps().first(), Some(&Step::ApprovalOff));
    }

    #[test]
    fn the_authority_changing_tools_are_denied_before_the_rest() {
        // §L-3: keep every catalogued authority-changing row ahead of ordinary denials. Five are
        // API-only, but exhaustive fresh-member deferred reachability is BLOCKED.
        let p = plan(&ConfinementPolicy::read_only_analyst(), &open_catalogue(), &unlocked(), true);
        let order: Vec<&str> = p
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Disable { tool } => Some(tool.as_str()),
                Step::ApprovalOff => None,
            })
            .collect();
        let authority: BTreeSet<&str> =
            AUTHORITY_CHANGING.iter().copied().filter(|t| !UNDISABLEABLE.contains(t)).collect();
        let last_authority = order.iter().rposition(|t| authority.contains(t)).expect("some");
        let first_other = order.iter().position(|t| !authority.contains(t)).expect("some");
        assert!(
            last_authority < first_other,
            "every authority-widening tool must be denied before any ordinary one: {order:?}"
        );
    }

    #[test]
    fn the_two_undisableable_tools_are_never_attempted() {
        // The runtime answers 400 to disabling them. Attempting it would manufacture a failure
        // that says nothing; locked `ask_each_time` is their achievable catalogue state.
        let p = plan(&ConfinementPolicy::read_only_analyst(), &open_catalogue(), &unlocked(), true);
        for step in p.steps() {
            if let Step::Disable { tool } = step {
                assert!(!UNDISABLEABLE.contains(&tool.as_str()), "attempted {tool}");
            }
        }
    }

    #[test]
    fn a_member_already_at_policy_needs_no_plan() {
        // Applying twice is a no-op, which is what makes this safe to run on a schedule.
        let mut confined: BTreeMap<String, String> = open_catalogue()
            .into_keys()
            .map(|t| {
                let state =
                    if UNDISABLEABLE.contains(&t.as_str()) { "ask_each_time" } else { "disabled" };
                (t, state.to_owned())
            })
            .collect();
        confined.insert("builtin.time".into(), "disabled".into());
        let p = plan(&ConfinementPolicy::read_only_analyst(), &confined, &unlocked(), false);
        assert!(p.is_empty(), "{:?}", p.steps());
    }

    #[test]
    fn a_tool_the_policy_allows_is_left_alone() {
        // The positive control. Without it a plan that disabled everything would pass every test
        // above, and the allowed set would be decoration.
        let policy = ConfinementPolicy::allowing(["builtin.time".to_owned()], false);
        let p = plan(&policy, &open_catalogue(), &unlocked(), false);
        assert!(
            !p.steps().contains(&Step::Disable { tool: "builtin.time".into() }),
            "an allowed tool must not be denied"
        );
        assert!(
            p.steps().contains(&Step::Disable { tool: "builtin.http".into() }),
            "and the rest must still be"
        );
    }

    #[test]
    fn sandbox_workspace_allows_only_the_sandbox_process_capability() {
        let policy = ConfinementPolicy::sandbox_workspace();
        assert_eq!(policy.allowed(), &BTreeSet::from(["builtin.shell".to_owned()]));
        assert!(!policy.auto_approve());
    }

    #[test]
    fn a_state_this_build_does_not_understand_is_treated_as_reachable() {
        // `Unrecognized` is never read as `Disabled`. A catalogue that grew a new state must
        // produce a denial, not a silent pass.
        let mut c = BTreeMap::new();
        c.insert("builtin.http".to_owned(), "some_new_state".to_owned());
        let p = plan(&ConfinementPolicy::read_only_analyst(), &c, &unlocked(), false);
        assert_eq!(p.steps(), [Step::Disable { tool: "builtin.http".into() }]);
    }

    // ── the measured `locked` set, and the control that makes it a measurement ──────────────
    //
    // The runtime computes `locked` from a capability's declared effects, so a capability that
    // gains a hard-floor effect in a later revision joins that set without joining
    // `UNDISABLEABLE`. Planning a write against such a row draws a `400`, and `Applied` stops at
    // the first failure — so one unknown locked row would stall the whole plan. These assert the
    // skip, and each carries the opposite case, because a predicate that skipped everything and a
    // predicate that skipped nothing would both satisfy a one-sided test (§K-8).

    #[test]
    fn a_row_the_catalogue_reports_locked_is_never_queued() {
        let mut c = BTreeMap::new();
        c.insert("builtin.some_future_financial_tool".to_owned(), "always_allow".to_owned());
        let p = plan(
            &ConfinementPolicy::read_only_analyst(),
            &c,
            &locked(&["builtin.some_future_financial_tool"]),
            false,
        );
        assert!(p.is_empty(), "a locked row must not be queued: {:?}", p.steps());
    }

    #[test]
    fn the_same_row_is_queued_when_the_catalogue_does_not_report_it_locked() {
        // The positive control for the test above. Without it, a `plan` that queued nothing at all
        // would pass.
        let mut c = BTreeMap::new();
        c.insert("builtin.some_future_financial_tool".to_owned(), "always_allow".to_owned());
        let p = plan(&ConfinementPolicy::read_only_analyst(), &c, &unlocked(), false);
        assert_eq!(
            p.steps(),
            [Step::Disable { tool: "builtin.some_future_financial_tool".into() }]
        );
    }

    #[test]
    fn a_measured_locked_row_at_its_floor_is_not_drift() {
        // §L-8: `ask_each_time` with approval off is the achievable floor for a row nothing can
        // disable. That must hold for a locked row this build has never heard of, not only for the
        // two in `UNDISABLEABLE`.
        let mut c = BTreeMap::new();
        c.insert("builtin.some_future_financial_tool".to_owned(), "ask_each_time".to_owned());
        let out = assess(
            &ConfinementPolicy::read_only_analyst(),
            &OpaqueId::new("member"),
            &c,
            &locked(&["builtin.some_future_financial_tool"]),
            false,
            "2026-09-02T00:00:00+00:00",
        );
        assert!(matches!(out, Confinement::Verified(_)), "{out:?}");
    }

    #[test]
    fn the_same_row_is_drift_when_it_is_not_reported_locked() {
        // The control for the test above: an unlocked row at `ask_each_time` is still reachable
        // and still drift, so `Verified` above is the locked set doing the work.
        let mut c = BTreeMap::new();
        c.insert("builtin.some_future_financial_tool".to_owned(), "ask_each_time".to_owned());
        let out = assess(
            &ConfinementPolicy::read_only_analyst(),
            &OpaqueId::new("member"),
            &c,
            &unlocked(),
            false,
            "2026-09-02T00:00:00+00:00",
        );
        assert!(matches!(out, Confinement::Drifted { .. }), "{out:?}");
    }

    #[test]
    fn an_absent_locked_field_still_skips_the_two_rows_measured_at_the_pin() {
        // `undisableable` is a union, not a substitution. A runtime that stopped reporting
        // `locked` — or a truncated read — must not cause a write we have a measured `400` from.
        let mut c = BTreeMap::new();
        for tool in UNDISABLEABLE {
            c.insert((*tool).to_owned(), "ask_each_time".to_owned());
        }
        let p = plan(&ConfinementPolicy::read_only_analyst(), &c, &unlocked(), false);
        assert!(p.is_empty(), "{:?}", p.steps());
    }
}
