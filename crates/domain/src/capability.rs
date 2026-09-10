//! What a service is permitted to do, and how an instance may only ever narrow it.

/// How much authority a capability grants.
///
/// **There is deliberately no `Write` variant.** Model-directed effect execution is not
/// implemented, and an unimplemented capability that already has a name in the type system is one
/// somebody will eventually pass. Propose-and-confirm is deliberately different: a person's
/// deterministic approval path performs the append, while the model still has no write authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    None,
    ReadOnly,
}

/// Which of the runtime's own built-in tools a service's turns may invoke.
///
/// Separate from [`Access`] because it is not a read/write question. IronClaw runs its own agent
/// loop over a built-in capability catalogue; this axis says which slice of it a service is
/// entitled to, and `ironworks_lifecycle::ConfinementPolicy` is what carries the decision to the
/// runtime. Ordered, so [`CapabilityProfile::attenuate`] narrows it like every other axis.
///
/// **There is deliberately no `Full` variant.** The catalogue is 53 rows on a fresh member and
/// `docs/INVARIANTS.md` §L-2 derives that a record-only turn needs none of them; a name meaning
/// "all of it" is one somebody eventually selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolAccess {
    /// No built-in is reachable. The empty allow-set.
    None,
    /// `builtin.shell` in the per-user sandbox worker, and nothing else (§L-10).
    Sandbox,
}

/// A profile that claims two things which cannot both be true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Incoherence {
    /// `docs/INVARIANTS.md` §L-10: IronClaw 1.4.0's sandbox shell always carries its managed
    /// allowlist for package registries and GitHub, and the operator setting can only *add*
    /// domains — there is no way to empty it. So a service cannot enable the sandbox and also
    /// tell a tenant it has no egress.
    #[error(
        "runtime_tools: sandbox with egress: none — the 1.4.0 sandbox shell always carries a \
         managed egress allowlist that cannot be emptied (INVARIANTS §L-10), so this profile \
         would publish a guarantee the runtime does not honour. Declare egress: read-only, or \
         runtime_tools: none."
    )]
    SandboxWithoutEgress,
}

/// The capabilities of a service, or of one instance of it.
///
/// A **service definition** carries the ceiling. An **instance** may only ever hold the same or
/// less, obtained by [`CapabilityProfile::attenuate`]. There is no operation that widens one,
/// which is what stops a provisioning mistake granting a read-only service more than its
/// definition allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityProfile {
    pub records: Access,
    pub writes: Access,
    pub egress: Access,
    pub outreach: Access,
    pub runtime_tools: ToolAccess,
}

/// Authority exercised only by an explicit human/operator command.
///
/// This is intentionally not an axis on [`CapabilityProfile`]: that profile enters model-turn
/// construction, while these permissions must remain unreachable from a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HumanAuthorityProfile {
    pub promote: bool,
    pub correct: bool,
    pub retract: bool,
    pub export: bool,
    pub delete: bool,
}

impl HumanAuthorityProfile {
    pub const NONE: Self =
        Self { promote: false, correct: false, retract: false, export: false, delete: false };

    pub const ACCOUNT_CUSTODIAN: Self =
        Self { promote: true, correct: true, retract: true, export: true, delete: false };

    pub const DEPROVISIONER: Self =
        Self { promote: false, correct: false, retract: false, export: true, delete: true };
}

impl CapabilityProfile {
    /// The frozen profile every current service declares: read the tenant's records, do nothing
    /// else.
    pub const READ_ONLY_RECORDS: Self = Self {
        records: Access::ReadOnly,
        writes: Access::None,
        egress: Access::None,
        outreach: Access::None,
        runtime_tools: ToolAccess::None,
    };

    /// A service whose turns get a sandboxed shell over a persistent `/workspace`.
    ///
    /// `egress` is `ReadOnly` rather than `None` and that is the whole point of the constant
    /// existing: §L-10's managed allowlist is reachable from the sandbox and cannot be emptied, so
    /// the honest declaration is a narrow egress rather than none. Records stay read-only and
    /// `writes`/`outreach` stay absent — a workspace is somewhere to compute, not authority to act.
    pub const SANDBOX_WORKSPACE: Self = Self {
        records: Access::ReadOnly,
        writes: Access::None,
        egress: Access::ReadOnly,
        outreach: Access::None,
        runtime_tools: ToolAccess::Sandbox,
    };

    /// No authority at all.
    pub const NONE: Self = Self {
        records: Access::None,
        writes: Access::None,
        egress: Access::None,
        outreach: Access::None,
        runtime_tools: ToolAccess::None,
    };

    /// Whether this profile claims two things that cannot both hold.
    ///
    /// Checked where a [`crate::ServiceDefinition`] is constructed, so an incoherent ceiling
    /// cannot be compiled into the binary. It is a *declaration* check, not a measurement: it
    /// catches a definition promising what the runtime will not honour, and says nothing about
    /// what any live member is confined to.
    pub fn incoherence(&self) -> Option<Incoherence> {
        (self.runtime_tools == ToolAccess::Sandbox && self.egress == Access::None)
            .then_some(Incoherence::SandboxWithoutEgress)
    }

    /// This profile narrowed to a ceiling — the field-wise **minimum**, never a union.
    ///
    /// Attenuation is the only way an instance profile is built, so "no instance can exceed its
    /// service" is a property of the construction rather than a rule someone checks.
    #[must_use]
    pub fn attenuate(&self, ceiling: &Self) -> Self {
        Self {
            records: self.records.min(ceiling.records),
            writes: self.writes.min(ceiling.writes),
            egress: self.egress.min(ceiling.egress),
            outreach: self.outreach.min(ceiling.outreach),
            runtime_tools: self.runtime_tools.min(ceiling.runtime_tools),
        }
    }

    /// Whether this profile is within `ceiling` on every axis.
    pub fn within(&self, ceiling: &Self) -> bool {
        self.attenuate(ceiling) == *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attenuation_narrows_and_never_widens() {
        let ceiling = CapabilityProfile::READ_ONLY_RECORDS;
        let greedy = CapabilityProfile {
            records: Access::ReadOnly,
            writes: Access::ReadOnly,
            egress: Access::ReadOnly,
            outreach: Access::ReadOnly,
            runtime_tools: ToolAccess::Sandbox,
        };
        // An instance asking for more than its service allows gets the service's answer.
        assert_eq!(greedy.attenuate(&ceiling), ceiling);
        assert!(!greedy.within(&ceiling));
    }

    #[test]
    fn attenuation_keeps_a_narrower_instance_narrow() {
        // The other direction matters just as much: a ceiling must not *grant* anything.
        let narrowed = CapabilityProfile::NONE.attenuate(&CapabilityProfile::READ_ONLY_RECORDS);
        assert_eq!(narrowed, CapabilityProfile::NONE);
        assert!(narrowed.within(&CapabilityProfile::READ_ONLY_RECORDS));
    }

    #[test]
    fn attenuation_is_idempotent() {
        let c = CapabilityProfile::READ_ONLY_RECORDS;
        assert_eq!(c.attenuate(&c).attenuate(&c), c);
    }

    #[test]
    fn a_sandbox_ceiling_narrows_a_tool_axis_it_does_not_grant_one() {
        // The ceiling must not hand `Sandbox` to an instance that asked for nothing, and an
        // instance asking for `Sandbox` under a tool-less service gets the service's answer.
        let none = CapabilityProfile::READ_ONLY_RECORDS;
        let sandbox = CapabilityProfile::SANDBOX_WORKSPACE;
        assert_eq!(none.attenuate(&sandbox).runtime_tools, ToolAccess::None);
        assert_eq!(sandbox.attenuate(&none).runtime_tools, ToolAccess::None);
        assert_eq!(sandbox.attenuate(&sandbox).runtime_tools, ToolAccess::Sandbox);
    }

    #[test]
    fn the_sandbox_cannot_be_declared_alongside_no_egress() {
        // §L-10: the 1.4.0 sandbox shell's managed allowlist cannot be emptied, so this pair is a
        // guarantee the runtime does not honour. The shipped constant must be the coherent one.
        assert_eq!(CapabilityProfile::SANDBOX_WORKSPACE.incoherence(), None);
        assert_eq!(CapabilityProfile::READ_ONLY_RECORDS.incoherence(), None);
        let lying =
            CapabilityProfile { egress: Access::None, ..CapabilityProfile::SANDBOX_WORKSPACE };
        assert_eq!(lying.incoherence(), Some(Incoherence::SandboxWithoutEgress));
    }

    #[test]
    fn every_current_service_is_read_only() {
        let c = CapabilityProfile::READ_ONLY_RECORDS;
        assert_eq!(c.writes, Access::None);
        assert_eq!(c.egress, Access::None, "egress is removed at provisioning, not granted here");
        assert_eq!(c.outreach, Access::None);
        assert_eq!(c.runtime_tools, ToolAccess::None, "a record-only turn needs no built-in");
    }
}
