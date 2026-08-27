//! The organizational and service semantics of IronWorks, as types.
//!
//! Pure: no IO, no clock, no credential values. Foreign systems are named only through traits an
//! adapter implements, so the whole domain can be exercised without one.
//!
//! # The rule this crate exists to enforce
//!
//! > Every **IronWorks-controlled** model input path and every **IronWorks-controlled** authority
//! > path must be explicit in the type model. Runtime-supplied inputs must be **named and bounded
//! > as foreign dependencies**, not falsely enumerated.
//!
//! See [`ModelInput`] and [`AuthorityPath`]. The foreign share is real and measured: a one-word
//! question on a bare pinned instance consumed 13,171 input tokens before any of our material,
//! and history compaction is the runtime's decision. [`ModelInput::RuntimeSupplied`] names that
//! rather than pretending to enumerate it.
//!
//! # Controls fail closed; data sources degrade
//!
//! Verification and guidance are **controls**: neither yields a value at all when it cannot
//! establish what it exists to establish. Records are a **data source**:
//! [`RecordsState::Degraded`] still builds a turn, because "the store is unavailable" is
//! something the model can be told honestly and still be useful with.
//!
//! # The acceptance test
//!
//! **Only a verified service instance can build a model turn**, and the compiler enforces it.
//! The positive control comes first, because a `compile_fail` example that fails for a typo
//! passes vacuously — the same reason every isolation check needs a positive control.
//!
//! ```
//! use ironworks_domain::*;
//! use std::collections::BTreeMap;
//!
//! struct Parts(BTreeMap<String, String>);
//! impl PartSource for Parts {
//!     fn read(&self, p: &PartRef) -> Option<&str> { self.0.get(p.as_str()).map(String::as_str) }
//! }
//! struct Store;
//! impl ScopeResolver for Store {
//!     fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
//!         Ok(ScopeEvidence::from_record_store(
//!             OrgId::parse("acme-sales").unwrap(), "http://store.example",
//!         ))
//!     }
//! }
//!
//! let parts = Parts(BTreeMap::from([
//!     ("personas/A.md".into(), "# A\n\nrole".into()),
//!     ("personas/_tail.md".into(), "## Safety\n\nrules".into()),
//! ]));
//! let definition = ServiceDefinition::new(
//!     ServiceId::parse("account-analysis").unwrap(),
//!     1,
//!     "reads a tenant's own records",
//!     Composition::new(
//!         vec![PartRef::new("personas/A.md")],
//!         "# ORGANIZATION GUIDANCE",
//!         PartRef::new("personas/_tail.md"),
//!     ).unwrap(),
//!     CapabilityProfile::READ_ONLY_RECORDS,
//!     None,
//! ).unwrap();
//! let guidance = Guidance::parse(
//!     &format!(
//!         "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\n{}",
//!         "content. ".repeat(60),
//!     ),
//!     "acme", "account-analysis",
//! ).unwrap();
//!
//! let instance = ServiceInstance::new(
//!     Slug::parse("acme").unwrap(),
//!     definition,
//!     guidance,
//!     RuntimeIdentity::parse("member-1").unwrap(),
//!     CredentialRef::new(CredentialPurpose::RuntimeMember, "keyring:member"),
//!     ContextBinding::new(
//!         CredentialRef::new(CredentialPurpose::RecordScope, "keyring:records"),
//!     ).unwrap(),
//!     ModelSelection::from_pin(&ModelPin::parse("m # c").unwrap()),
//!     CapabilityProfile::READ_ONLY_RECORDS,
//!     None,
//!     &parts,
//! ).unwrap();
//!
//! // Verified first, then served.
//! let verified = instance.verify(&Store).unwrap();
//! let at = record::Timestamp::parse("2026-08-27T12:00:00+00:00").unwrap();
//! let turn = verified.build_turn(
//!     &parts,
//!     &Ask {
//!         question: "what is outstanding?",
//!         records: &[],
//!         records_state: &RecordsState::Empty,
//!         retrieved_at: &at,
//!         speaker: None,
//!         fact_fields: None,
//!     },
//!     None,
//! ).unwrap();
//! assert_eq!(turn.identity().org().as_str(), "acme-sales");
//! assert!(turn.instructions().as_str().ends_with("## Safety\n\nrules"));
//! // The envelope was rendered by the instance. With no records it takes its minimal branch and
//! // carries only the book status the model must know — which the instance decided, not the
//! // caller, because `Ask` has no field for it.
//! assert!(turn.input().as_str().contains("ACCOUNT RECORDS STATUS: empty"));
//! ```
//!
//! An **unverified** instance cannot build a turn — `build_turn` exists only on
//! `ServiceInstance<Verified>`:
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! # fn unverified() -> ServiceInstance<Unverified> { unimplemented!() }
//! # fn parts() -> &'static dyn PartSource { unimplemented!() }
//! # fn ask() -> &'static Ask<'static> { unimplemented!() }
//! let _ = unverified().build_turn(parts(), ask(), None);
//! ```
//!
//! Nor produce a serving identity or touch thread state:
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! # fn unverified() -> ServiceInstance<Unverified> { unimplemented!() }
//! let _ = unverified().serving_identity();
//! ```
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! # fn unverified() -> ServiceInstance<Unverified> { unimplemented!() }
//! let _ = unverified().thread_scope();
//! ```
//!
//! And a turn cannot be assembled around the instance at all — [`ModelTurn`] has no public
//! constructor, so there is no back door:
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! # fn identity() -> ServingIdentity { unimplemented!() }
//! # fn alloy() -> Alloy { unimplemented!() }
//! # fn input() -> TurnInput { unimplemented!() }
//! let _ = ModelTurn {
//!     identity: identity(),
//!     alloy: alloy(),
//!     input: input(),
//!     previous: None,
//! };
//! ```
//!
//! # The other half of the surface
//!
//! The instructions were always gated. The **envelope** — the half carrying a tenant's business
//! records — was not: it was rendered by callers and injected as finished text through a public
//! `TurnInput::new`. Two halves of one model-visible surface, two enforcement strengths, and the
//! weaker one carried the records.
//!
//! Pre-rendered context can no longer be constructed:
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! let _ = TurnInput::new("ORGANIZATION: someone-else\n\nfabricated records", RecordsState::Complete);
//! ```
//!
//! Nor supplied, because [`Ask`] carries materials and has no field for rendered text — or for an
//! org, which comes from the verified scope:
//!
//! ```compile_fail
//! # use ironworks_domain::*;
//! # fn at() -> record::Timestamp { unimplemented!() }
//! let _ = Ask {
//!     question: "q",
//!     records: &[],
//!     records_state: &RecordsState::Complete,
//!     retrieved_at: &at(),
//!     speaker: None,
//!     fact_fields: None,
//!     org: "someone-else",
//! };
//! ```
//!
//! > **Every IronWorks-controlled model-visible byte is constructible only through a verified
//! > service instance.**
//!
//! This says nothing about the runtime's own contributions. Conversation replay and workspace
//! memory are named by [`ModelInput::RuntimeSupplied`] and remain outside the gate, because they
//! are outside our control — and an enumeration that claimed otherwise would be the more
//! dangerous falsehood.
//!
//! # Not implemented, deliberately
//!
//! Approvals, workflows, effect execution, and storage compatibility. [`Access`] has no `Write`
//! variant and [`AuthorityPath::Approvals`] reports [`AuthorityPath::implemented`] as `false`: an
//! unimplemented capability that already has a name is one somebody eventually passes.

pub mod address;
pub mod capability;
pub mod composition;
pub mod continuation;
pub mod custody;
pub mod delivery;
pub mod envelope;
pub mod ids;
pub mod instance;
pub mod pins;
pub mod progress;
pub mod record;
pub mod registry;
pub mod route;
pub mod service;
pub mod share;
pub mod supply;

pub use address::Addressing;
pub use capability::{Access, CapabilityProfile, Incoherence, ToolAccess};
pub use composition::{
    Alloy, Composition, CompositionError, Guidance, GuidanceError, GuidanceMarker, MeltError,
    PartRef, PartSource, melt, strip_frontmatter,
};
pub use continuation::{Continuation, FreshBecause, PriorThread, ThreadLimit, decide};
pub use custody::restrict_to_owner;
pub use delivery::{AttemptOutcome, DeliveryState, Repair};
pub use ids::{
    ActorRef, ContributorId, CredentialPurpose, CredentialRef, GuidanceRef, IdError,
    IdempotencyKey, InvalidId, InvalidKey, OrgId, ResponseId, RuntimeIdentity, ServiceId, Slug,
};
pub use instance::{
    Ask, ContextBinding, Degradation, InstanceError, InstanceState, ModelSelection, ModelTurn,
    RecordScope, RecordsState, ScopeEvidence, ScopeFailure, ScopeResolver, ScopeSource,
    ServiceInstance, ServingIdentity, ThreadPointer, ThreadScope, TurnError, TurnInput, Unverified,
    Verified, VerifyError,
};
pub use pins::ModelPin;
pub use progress::{BlankReason, NonEmptyReason, Openness, TurnProgress};
pub use registry::{RegistryError, TenantEntry, TenantRegistry};
pub use route::{ChannelRef, ChatKind, RouteAmbiguity, RouteState, resolve};
pub use service::{ServiceDefinition, ServiceDefinitionError};
pub use share::{Intent, Refusal, ShareRequest, is_command, render_echo};
pub use supply::{SuppliedMark, Supply};

// ── the taxonomies ───────────────────────────────────────────────────────────────────────

/// Who decides the content of a model input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controller {
    IronWorks,
    IronClaw,
}

/// Prompt material the runtime supplies. Named, never enumerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInputKind {
    /// Prior turns, replayed and compacted server-side by a policy we do not control.
    ConversationReplay,
    /// The runtime's own workspace identity files.
    WorkspaceMemory,
    /// Anything else the runtime prepends. Present so the taxonomy stays honest as it grows.
    Other,
}

/// A fingerprint of an alloy. The bytes live elsewhere; this identifies them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionRef(String);

/// A fingerprint of the records rendered into one envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordsRef(String);

/// A tool whose output reached the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolId(String);

macro_rules! ref_newtype {
    ($t:ident) => {
        impl $t {
            pub fn new(v: impl Into<String>) -> Self {
                Self(v.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
ref_newtype!(CompositionRef);
ref_newtype!(RecordsRef);
ref_newtype!(ToolId);

/// Every path by which material reaches the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelInput {
    Persona(CompositionRef),
    Guidance(GuidanceRef),
    Records(RecordsRef),
    ToolOutput(ToolId),
    RuntimeSupplied(RuntimeInputKind),
}

impl ModelInput {
    pub fn controller(&self) -> Controller {
        match self {
            Self::Persona(_) | Self::Guidance(_) | Self::Records(_) | Self::ToolOutput(_) => {
                Controller::IronWorks
            }
            Self::RuntimeSupplied(_) => Controller::IronClaw,
        }
    }
}

/// Every path by which authority is exercised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityPath {
    /// What the sealed member may do on the runtime.
    RuntimeCapabilities,
    /// Whether the runtime can reach anything but its model provider.
    ExternalNetwork,
    /// Which credentials exist and who holds them.
    Credentials,
    /// **Which third party sees a tenant's records.** A pin is a privacy decision wearing a
    /// version number.
    ModelSelection,
    /// **Which records an instance may read.** Granted at provisioning, enforced server-side.
    ContextBinding,
    /// A human gate on an effect. Not implemented.
    Approvals,
    /// Provision, suspend, rotate, deprovision.
    Lifecycle,
    /// Sending something to a person.
    Delivery,
}

impl AuthorityPath {
    pub const ALL: [Self; 8] = [
        Self::RuntimeCapabilities,
        Self::ExternalNetwork,
        Self::Credentials,
        Self::ModelSelection,
        Self::ContextBinding,
        Self::Approvals,
        Self::Lifecycle,
        Self::Delivery,
    ];

    /// Whether this path exists in the implementation yet.
    ///
    /// `Approvals` is a **named absence**: nothing in the extracted corpus constrains an
    /// approval, because current capabilities are frozen at no-writes. When propose-and-confirm
    /// arrives its invariants must be written rather than extracted.
    pub fn implemented(&self) -> bool {
        !matches!(self, Self::Approvals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_controllers_are_split_correctly() {
        assert_eq!(
            ModelInput::Persona(CompositionRef::new("d")).controller(),
            Controller::IronWorks
        );
        assert_eq!(
            ModelInput::RuntimeSupplied(RuntimeInputKind::ConversationReplay).controller(),
            Controller::IronClaw
        );
    }

    /// The rule is only honest if the foreign share has somewhere to live. If this becomes zero,
    /// someone has tidied away the variant that stops us claiming full enumeration.
    #[test]
    fn the_taxonomy_admits_inputs_we_do_not_control() {
        let foreign: Vec<ModelInput> = [
            RuntimeInputKind::ConversationReplay,
            RuntimeInputKind::WorkspaceMemory,
            RuntimeInputKind::Other,
        ]
        .into_iter()
        .map(ModelInput::RuntimeSupplied)
        .collect();
        assert_eq!(foreign.len(), 3);
        assert!(foreign.iter().all(|i| i.controller() == Controller::IronClaw));
    }

    #[test]
    fn model_selection_and_context_binding_are_authority_paths() {
        assert!(AuthorityPath::ALL.contains(&AuthorityPath::ModelSelection));
        assert!(AuthorityPath::ALL.contains(&AuthorityPath::ContextBinding));
    }

    #[test]
    fn approvals_is_the_only_unimplemented_authority_path() {
        let unimplemented: Vec<_> =
            AuthorityPath::ALL.iter().filter(|a| !a.implemented()).collect();
        assert_eq!(unimplemented, vec![&AuthorityPath::Approvals]);
    }
}
