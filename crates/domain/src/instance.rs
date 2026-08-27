//! A service instance, and the turn only a verified one can build.
//!
//! # Why this is a typestate and not a boolean
//!
//! An instance's configuration *claims* an org; the record store *resolves* one from a
//! credential, server-side. The current implementation carries an `organization_verified` flag
//! and refuses to load thread state when it is false — a runtime check, in one place, that a new
//! call site can forget.
//!
//! Here they are different types. [`ServiceInstance<Unverified>`] has no `org()` at all, because
//! it does not have one: what it holds is a claim. Only [`ServiceInstance<Verified>`] can produce
//! a [`ServingIdentity`], a [`ThreadScope`], or a [`ModelTurn`]. The ordering hazard is removed
//! rather than tested for.
//!
//! # Controls fail closed; data sources degrade
//!
//! Verification is a **control**: an unresolvable scope yields no instance, so nothing downstream
//! can be reached. Guidance is a **control**: an instance cannot be constructed without validated
//! guidance, so there is no unguided composition to fall back to.
//!
//! Records are a **data source**: [`RecordsState::Degraded`] and [`RecordsState::Empty`] still
//! build a turn, because a store that is unavailable has established exactly one thing — that it
//! is unavailable — which the model can be told honestly and still be useful.

use crate::capability::CapabilityProfile;
use crate::composition::{Alloy, Guidance, MeltError, PartSource, melt};
use crate::ids::{CredentialPurpose, CredentialRef, OrgId, ResponseId, RuntimeIdentity, Slug};
use crate::service::ServiceDefinition;

mod sealed {
    pub trait Sealed {}
}

/// An instance's verification state. Sealed: there is no third state.
pub trait InstanceState: sealed::Sealed {}

/// Configured but not yet checked against the record store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unverified {
    claimed_org: Option<OrgId>,
}

/// The store resolved this instance's credential to an org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    evidence: ScopeEvidence,
    claim_matched: bool,
}

impl sealed::Sealed for Unverified {}
impl sealed::Sealed for Verified {}
impl InstanceState for Unverified {}
impl InstanceState for Verified {}

/// Which records an instance may read. An authority path in its own right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBinding {
    credential: CredentialRef,
}

impl ContextBinding {
    /// Refuses anything but a record-scope credential — a runtime member credential here would be
    /// a cross-wiring, and the two must never be the same.
    pub fn new(credential: CredentialRef) -> Result<Self, InstanceError> {
        if credential.purpose() != CredentialPurpose::RecordScope {
            return Err(InstanceError::WrongCredentialPurpose {
                field: "context binding",
                want: CredentialPurpose::RecordScope,
                got: credential.purpose(),
            });
        }
        Ok(Self { credential })
    }

    pub fn credential(&self) -> &CredentialRef {
        &self.credential
    }
}

/// The model a turn runs on.
///
/// Constructible **only** from a pin. There is no `ModelSelection::new`, so "a service cannot
/// choose its own model" is a property of the type. Model selection decides which third party
/// sees a tenant's records, which is why it is an authority path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection(String);

impl ModelSelection {
    pub fn from_pin(pin: &crate::pins::ModelPin) -> Self {
        Self(pin.as_str().to_owned())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstanceError {
    #[error("{field} needs a {want:?} credential, got {got:?}")]
    WrongCredentialPurpose { field: &'static str, want: CredentialPurpose, got: CredentialPurpose },
    #[error(
        "the runtime and record credentials are the same handle — cross-wired credentials mean \
         one identity where there must be two"
    )]
    CrossWiredCredentials,
    #[error("service {service}: the corpus does not hold {missing} part(s) this composition names")]
    MissingParts { service: String, missing: usize },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    #[error("instance {slug}: refusing to serve — {source}")]
    Unresolved {
        slug: Slug,
        #[source]
        source: ScopeFailure,
    },
}

/// How a scope was established.
///
/// Recorded so a verified value can say *how* it became verified. A boolean cannot be audited;
/// this can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeSource {
    /// The record store resolved the credential server-side and named this endpoint.
    RecordStore { endpoint: String },
}

/// Evidence that a scope was resolved by something entitled to resolve it.
///
/// Constructed only through a named source, so fabricating one is a visible act rather than a
/// convenient `true`. [`Verified`] carries it, which is what makes a verified instance
/// *auditable* rather than merely flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeEvidence {
    org: OrgId,
    source: ScopeSource,
}

impl ScopeEvidence {
    /// The record store is the authority on which org a credential authorises.
    pub fn from_record_store(org: OrgId, endpoint: impl Into<String>) -> Self {
        Self { org, source: ScopeSource::RecordStore { endpoint: endpoint.into() } }
    }
    pub fn org(&self) -> &OrgId {
        &self.org
    }
    pub fn source(&self) -> &ScopeSource {
        &self.source
    }
}

/// Why a scope could not be established.
///
/// Every variant fails closed. A control that cannot verify has established nothing, and an
/// unverified identity does not serve — the cost is real and accepted: a store outage becomes a
/// serving outage for that instance.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ScopeFailure {
    #[error("the record store could not be reached: {detail}")]
    Unreachable { detail: String },
    #[error("the record store refused this credential")]
    Unauthorized,
    #[error("the record store answered in a shape this cannot read: {detail}")]
    Malformed { detail: String },
}

/// Resolves a record-scope credential to the org it authorises, server-side.
pub trait ScopeResolver {
    fn resolve(&self, credential: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure>;
}

/// One organization's instance of one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstance<S: InstanceState> {
    slug: Slug,
    definition: ServiceDefinition,
    guidance: Guidance,
    runtime_identity: RuntimeIdentity,
    runtime_credential: CredentialRef,
    context_binding: ContextBinding,
    model: ModelSelection,
    capabilities: CapabilityProfile,
    state: S,
}

impl<S: InstanceState> ServiceInstance<S> {
    pub fn slug(&self) -> &Slug {
        &self.slug
    }
    pub fn definition(&self) -> &ServiceDefinition {
        &self.definition
    }
    /// Attenuated against the definition's ceiling at construction, so this can never exceed it.
    pub fn capabilities(&self) -> &CapabilityProfile {
        &self.capabilities
    }
    pub fn context_binding(&self) -> &ContextBinding {
        &self.context_binding
    }
    pub fn model(&self) -> &ModelSelection {
        &self.model
    }
    /// The fact keys this instance's tenant declared, in declared order.
    ///
    /// Read from validated [`Guidance`], which is the only place a tenant's own policy is allowed
    /// to enter — so both serving paths obtain it from the object that was already bound to this
    /// slug and this service, and neither needs a registry field, an environment handle or a
    /// parser of its own.
    pub fn fact_fields(&self) -> &[String] {
        self.guidance.fact_fields()
    }
}

impl ServiceInstance<Unverified> {
    /// Build an instance from configuration.
    ///
    /// Every input the model or an authority path depends on is explicit here: the definition
    /// (and through it the composition), validated guidance, the runtime identity and its
    /// credential, the context binding, the model selection, and the requested capabilities.
    ///
    /// `guidance` is a validated [`Guidance`] and not an `Option` — guidance is mandatory and
    /// there is no unguided composition to fall back to.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        slug: Slug,
        definition: ServiceDefinition,
        guidance: Guidance,
        runtime_identity: RuntimeIdentity,
        runtime_credential: CredentialRef,
        context_binding: ContextBinding,
        model: ModelSelection,
        requested_capabilities: CapabilityProfile,
        claimed_org: Option<OrgId>,
        parts: &dyn PartSource,
    ) -> Result<Self, InstanceError> {
        if runtime_credential.purpose() != CredentialPurpose::RuntimeMember {
            return Err(InstanceError::WrongCredentialPurpose {
                field: "runtime identity",
                want: CredentialPurpose::RuntimeMember,
                got: runtime_credential.purpose(),
            });
        }
        // The append handle has no business here at all. `ContextBinding::new` already refuses a
        // wrong purpose on the records slot and the check above covers the runtime slot, so this
        // is belt-and-braces — but it is the one place a future field could quietly admit it, and
        // an append credential reachable from a `ServiceInstance` is an append credential one
        // refactor away from a model input.
        if context_binding.credential().purpose() == CredentialPurpose::RecordAppend {
            return Err(InstanceError::WrongCredentialPurpose {
                field: "context binding",
                want: CredentialPurpose::RecordScope,
                got: CredentialPurpose::RecordAppend,
            });
        }
        if runtime_credential.handle() == context_binding.credential().handle() {
            return Err(InstanceError::CrossWiredCredentials);
        }
        // A composition naming a part the corpus does not hold fails at construction, not
        // mid-turn: the whole instance refuses rather than one turn.
        let missing = definition.composition().missing_in(parts).len();
        if missing > 0 {
            return Err(InstanceError::MissingParts { service: definition.service_id(), missing });
        }
        let capabilities = requested_capabilities.attenuate(definition.capability_ceiling());
        Ok(Self {
            slug,
            definition,
            guidance,
            runtime_identity,
            runtime_credential,
            context_binding,
            model,
            capabilities,
            state: Unverified { claimed_org },
        })
    }

    /// What the configuration *claims*. Metadata: nothing may be scoped by it.
    pub fn claimed_org(&self) -> Option<&OrgId> {
        self.state.claimed_org.as_ref()
    }

    /// Exchange the claim for an authoritative scope. Consumes the unverified instance, so the
    /// unchecked value cannot linger beside the checked one and be passed by mistake.
    pub fn verify(
        self,
        resolver: &dyn ScopeResolver,
    ) -> Result<ServiceInstance<Verified>, VerifyError> {
        let evidence = resolver
            .resolve(self.context_binding.credential())
            .map_err(|source| VerifyError::Unresolved { slug: self.slug.clone(), source })?;
        let claim_matched = self.state.claimed_org.as_ref() == Some(evidence.org());
        Ok(ServiceInstance {
            slug: self.slug,
            definition: self.definition,
            guidance: self.guidance,
            runtime_identity: self.runtime_identity,
            runtime_credential: self.runtime_credential,
            context_binding: self.context_binding,
            model: self.model,
            capabilities: self.capabilities,
            state: Verified { evidence, claim_matched },
        })
    }
}

impl ServiceInstance<Verified> {
    /// The authoritative scope, resolved server-side.
    pub fn org(&self) -> &OrgId {
        self.state.evidence.org()
    }

    /// *How* this instance's scope was established. A verified value that cannot say how it was
    /// verified is a boolean wearing a better name.
    pub fn scope_evidence(&self) -> &ScopeEvidence {
        &self.state.evidence
    }

    /// The value a record fetch requires.
    ///
    /// Every retrieval takes one of these and **none takes an organization identifier**, so a
    /// caller-supplied org cannot influence scope: there is no parameter for it to arrive in.
    pub fn record_scope(&self) -> RecordScope {
        RecordScope {
            evidence: self.state.evidence.clone(),
            credential: self.context_binding.credential().clone(),
        }
    }

    /// Whether the configuration's claimed org agreed with the resolved one. A mismatch is not
    /// fatal — the claim is explicitly metadata — but it means a config file describes an
    /// instance it no longer scopes, which an operator should hear about.
    pub fn claim_matched(&self) -> bool {
        self.state.claim_matched
    }

    pub fn serving_identity(&self) -> ServingIdentity {
        ServingIdentity {
            org: self.org().clone(),
            runtime_identity: self.runtime_identity.clone(),
            runtime_credential: self.runtime_credential.clone(),
            model: self.model.clone(),
            capabilities: self.capabilities,
        }
    }

    /// Scopes durable thread state to this instance.
    pub fn thread_scope(&self) -> ThreadScope {
        ThreadScope { slug: self.slug.clone(), org: self.org().clone() }
    }

    /// Melt this instance's **own** composition and guidance into an alloy.
    ///
    /// Private on purpose: an alloy is only ever reachable through [`Self::build_turn`], so one
    /// can never be built for one service and sent under another's identity.
    fn alloy(&self, parts: &dyn PartSource) -> Result<Alloy, MeltError> {
        melt(self.definition.composition(), &self.guidance, parts)
    }

    /// What this instance would compose, fingerprinted — without composing it into a turn.
    ///
    /// The continuation decision needs the digest *before* the turn is built, because whether to
    /// chain is an input to building it. Melting twice per turn is a few microseconds over ~25 KB;
    /// the alternative is handing the alloy itself to a caller that has no business holding it,
    /// which is the gate `alloy` stays private to protect.
    pub fn alloy_digest(&self, parts: &dyn PartSource) -> Result<String, MeltError> {
        Ok(self.alloy(parts)?.digest())
    }

    /// Build the turn.
    ///
    /// # Every model-visible byte is rendered here
    ///
    /// Both halves are **composed, never accepted**. The alloy is melted from this instance's own
    /// composition and guidance, and the envelope is rendered from an [`Ask`] — raw records, a
    /// question, a timestamp — rather than handed in as finished text.
    ///
    /// That symmetry is the whole point, and it did not used to hold. The alloy was gated and the
    /// envelope was not: a caller rendered its own and injected it through a public
    /// `TurnInput::new`, so the half carrying the tenant's business records was the half anyone
    /// could write. The governing invariant is now true rather than aspirational:
    ///
    /// > Every IronWorks-controlled model-visible byte is constructible only through a verified
    /// > service instance.
    ///
    /// What this does **not** claim is anything about the runtime's own contributions —
    /// conversation replay and workspace memory. Those are named as
    /// [`crate::ModelInput::RuntimeSupplied`] and remain outside this gate, because they are
    /// outside our control and pretending to enumerate them would be the more dangerous lie.
    ///
    /// A thread pointer from another instance is refused.
    pub fn build_turn(
        &self,
        parts: &dyn PartSource,
        ask: &Ask<'_>,
        previous: Option<ThreadPointer>,
    ) -> Result<ModelTurn, TurnError> {
        if let Some(p) = &previous
            && p.instance != self.slug
        {
            return Err(TurnError::ForeignThread {
                instance: self.slug.clone(),
                pointer_instance: p.instance.clone(),
            });
        }
        // The org is taken from the verified scope, never from the caller. An `Ask` has no field
        // for one, so a turn cannot be built that tells the model it belongs to another tenant.
        let note = ask.records_state.note();
        let rendered = crate::envelope::build(&crate::envelope::Turn {
            user_text: ask.question,
            org: self.org(),
            retrieved_at: ask.retrieved_at,
            speaker: ask.speaker,
            note: note.as_deref(),
            records: ask.records,
            fact_fields: ask.fact_fields,
        });
        Ok(ModelTurn {
            identity: self.serving_identity(),
            alloy: self.alloy(parts).map_err(TurnError::Melt)?,
            input: TurnInput::new(rendered.as_str(), ask.records_state.clone()),
            previous,
        })
    }
}

/// What a turn is built **from**: materials, never rendered text.
///
/// There is no field for the org and no field for a finished envelope. Both are supplied by the
/// verified instance doing the rendering, which is what makes caller-supplied organization
/// context unrepresentable rather than merely discouraged.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// The person's message. Attacker-controlled: sanitized during rendering, and never used to
    /// decide which records a turn sees.
    pub question: &'a str,
    pub records: &'a [crate::record::AccountRecord],
    /// What the store established. Decides the note the model is shown — see
    /// [`RecordsState::note`].
    pub records_state: &'a RecordsState,
    /// When the records were fetched. Passed in and journalled rather than read from a clock, so
    /// the envelope can be rebuilt byte-identically after a crash (ADR 0005).
    pub retrieved_at: &'a crate::record::Timestamp,
    /// The human who sent the message, if the channel names one.
    pub speaker: Option<&'a str>,
    /// Which fact keys this tenant declared it expects. `None` asserts no gaps at all.
    pub fact_fields: Option<&'a [String]>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TurnError {
    #[error(transparent)]
    Melt(MeltError),
    #[error(
        "instance {instance} was handed a thread pointer belonging to {pointer_instance} — \
         upstream does not refuse a cross-wired continuation, so this must"
    )]
    ForeignThread { instance: Slug, pointer_instance: Slug },
}

/// Everything needed to invoke the runtime for one instance.
///
/// **No public constructor.** The only way to obtain one is
/// [`ServiceInstance::<Verified>::serving_identity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingIdentity {
    org: OrgId,
    runtime_identity: RuntimeIdentity,
    runtime_credential: CredentialRef,
    model: ModelSelection,
    capabilities: CapabilityProfile,
}

impl ServingIdentity {
    pub fn org(&self) -> &OrgId {
        &self.org
    }
    pub fn runtime_identity(&self) -> &RuntimeIdentity {
        &self.runtime_identity
    }
    /// Where the runtime credential lives — the adapter resolves it at the moment of use.
    pub fn runtime_credential(&self) -> &CredentialRef {
        &self.runtime_credential
    }
    pub fn model(&self) -> &ModelSelection {
        &self.model
    }
    pub fn capabilities(&self) -> &CapabilityProfile {
        &self.capabilities
    }
}

/// Scopes durable thread state to one verified instance. No public constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadScope {
    slug: Slug,
    org: OrgId,
}

#[cfg(test)]
impl ThreadScope {
    /// A scope without an instance behind it, for tests in sibling modules.
    ///
    /// `#[cfg(test)]` and nothing more: outside tests the only way to a `ThreadScope` stays
    /// [`ServiceInstance::thread_scope`], so a pointer still cannot exist without a verified
    /// instance having produced it. A test in `continuation` needs a pointer to decide about and
    /// has no business building a whole verified instance to get one.
    pub(crate) fn for_test(slug: Slug, org: OrgId) -> Self {
        Self { slug, org }
    }
}

impl ThreadScope {
    pub fn slug(&self) -> &Slug {
        &self.slug
    }
    pub fn org(&self) -> &OrgId {
        &self.org
    }

    /// Mint a pointer to a completed turn, scoped to this instance.
    ///
    /// A pointer can only come from a scope, and a scope only from a verified instance — so a
    /// pointer always knows whose it is. Upstream treats a cross-wired continuation as undefined
    /// behaviour rather than refusing it, which makes this the only place it can be caught.
    pub fn pointer(&self, response: ResponseId) -> ThreadPointer {
        ThreadPointer { instance: self.slug.clone(), response }
    }
}

/// Authorises reading one org's records. No public constructor.
///
/// Carries the *evidence* that the scope was resolved, and the credential the adapter presents.
/// The org is read from the evidence, never accepted from a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordScope {
    evidence: ScopeEvidence,
    credential: CredentialRef,
}

impl RecordScope {
    pub fn org(&self) -> &OrgId {
        self.evidence.org()
    }
    pub fn evidence(&self) -> &ScopeEvidence {
        &self.evidence
    }
    /// Where the record-scope credential lives — resolved by the adapter at the moment of use.
    pub fn credential(&self) -> &CredentialRef {
        &self.credential
    }
}

/// A continuation handle that carries its instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPointer {
    instance: Slug,
    response: ResponseId,
}

impl ThreadPointer {
    pub fn instance(&self) -> &Slug {
        &self.instance
    }
    pub fn response(&self) -> &ResponseId {
        &self.response
    }
}

/// What the record store established. A **data source**, so every variant still serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordsState {
    /// The book was read in full.
    Complete,
    /// The org has no records yet. Declared to the model, never implied by silence.
    Empty,
    /// The store could not be read, or could not be read fully. The model is told.
    Degraded(Degradation),
    /// The records are not in this envelope because this conversation was already shown them and
    /// the book has not moved since.
    ///
    /// A fourth state rather than reusing `Empty`, and the distinction is the whole point: a
    /// model told "empty" would answer that the organization has no records, having been handed
    /// forty of them one message earlier. The count travels because "the 40 records you were
    /// shown earlier" is something a model can reason about and "you have them" is not.
    CarriedByThread { records: usize },
}

/// Why a book is incomplete. A cause, never a sentence.
///
/// The variant used to be `Degraded { reason: String }`, and the string was authored by whichever
/// caller happened to build the state — so an adapter or a CLI could put arbitrary text in front
/// of a tenant's model through a public field. That is the same hole `TurnInput::new` was, one
/// level down: the envelope's *structure* was gated while a sentence inside it was not.
///
/// Adapters now report a cause and `domain` chooses the words. What a tenant's model is told about
/// its own records is a product decision, and it lives with the other product decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Degradation {
    /// The store could not be read at all.
    Unavailable,
    /// Some records could not be read. `unreadable` is how many are missing from the envelope —
    /// a count, because the model can act on "three are missing" and cannot act on "some".
    Partial { unreadable: usize },
}

impl RecordsState {
    /// Whether **this envelope** carries the tenant's records.
    ///
    /// `CarriedByThread` is false, and the wording of the question matters: the conversation has
    /// the records, this envelope does not. Anything asking "are the records here" — a proof
    /// counting bytes, a check that a degraded read produced no book — means this envelope.
    pub fn has_records(&self) -> bool {
        matches!(self, Self::Complete)
    }

    /// What the model is told about the state of the book, if anything.
    ///
    /// This is model-visible text, so it lives behind the same gate as the rest of it. It was
    /// authored in the CLI, which meant a binary could decide what a tenant's model is told about
    /// its own records — a hole in the surface that no type made visible, because the string
    /// arrived as an already-rendered `&str`.
    ///
    /// `Complete` says nothing on purpose: a note explaining that everything is fine is a
    /// sentence the model has to spend attention discounting.
    /// What the model is told about the state of the book, if anything.
    ///
    /// This is model-visible text, so it lives behind the same gate as the rest of it. It was
    /// authored in the CLI, which meant a binary could decide what a tenant's model is told about
    /// its own records — a hole no type made visible, because the string arrived already
    /// rendered.
    ///
    /// `Complete` says nothing on purpose: a note explaining that everything is fine is a
    /// sentence the model has to spend attention discounting.
    pub(crate) fn note(&self) -> Option<String> {
        match self {
            Self::Complete => None,
            Self::Empty => Some(
                "empty — no account records have been loaded for this organization yet \
                 (see your empty-book instructions)"
                    .to_owned(),
            ),
            Self::Degraded(Degradation::Unavailable) => Some(
                "temporarily unavailable — the records store could not be reached just now; \
                 answer from conversation history and say records are briefly unavailable if the \
                 question needs them"
                    .to_owned(),
            ),
            Self::Degraded(Degradation::Partial { unreadable }) => Some(format!(
                "partial — {unreadable} record(s) in your book could not be read just now and are \
                 not included below; answer from what is here and say the book is incomplete if \
                 the question needs them"
            )),
            Self::CarriedByThread { records } => Some(format!(
                "already supplied — the {records} record(s) in your book were included earlier in \
                 this same conversation and have not changed since; use them, and do not treat \
                 their absence below as an empty book"
            )),
        }
    }
}

/// The model-visible input for one turn: the rendered envelope and what the store established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnInput {
    text: String,
    records: RecordsState,
}

impl TurnInput {
    /// **Crate-private.** The only way to obtain a `TurnInput` is for
    /// [`ServiceInstance::<Verified>::build_turn`] to render one from an [`Ask`].
    ///
    /// It was public, and that was the hole this type now closes: instructions were gated by the
    /// typestate while the *input* — the half carrying the tenant's business records — could be
    /// rendered by any caller and injected as a finished string. Two halves of one model-visible
    /// surface with two different enforcement strengths, and the weaker one carried the records.
    fn new(text: impl Into<String>, records: RecordsState) -> Self {
        Self { text: text.into(), records }
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
    pub fn records(&self) -> &RecordsState {
        &self.records
    }
}

/// One turn, ready to send.
///
/// **No public constructor.** The only way to obtain one is
/// [`ServiceInstance::<Verified>::build_turn`], so a turn cannot exist for an instance whose
/// scope was never resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTurn {
    identity: ServingIdentity,
    alloy: Alloy,
    input: TurnInput,
    previous: Option<ThreadPointer>,
}

impl ModelTurn {
    pub fn identity(&self) -> &ServingIdentity {
        &self.identity
    }
    pub fn instructions(&self) -> &Alloy {
        &self.alloy
    }
    pub fn input(&self) -> &TurnInput {
        &self.input
    }
    pub fn previous(&self) -> Option<&ThreadPointer> {
        self.previous.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::{Composition, PartRef};
    use std::collections::BTreeMap;

    struct Parts(BTreeMap<String, String>);
    impl PartSource for Parts {
        fn read(&self, p: &PartRef) -> Option<&str> {
            self.0.get(p.as_str()).map(String::as_str)
        }
    }
    fn parts() -> Parts {
        Parts(BTreeMap::from([
            ("personas/A.md".into(), "# A\n\nrole".into()),
            ("personas/_tail.md".into(), "## Safety\n\nrules".into()),
        ]))
    }

    struct Store(&'static str);
    impl ScopeResolver for Store {
        fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
            Ok(ScopeEvidence::from_record_store(
                OrgId::parse(self.0).unwrap(),
                "http://store.example",
            ))
        }
    }
    struct Down;
    impl ScopeResolver for Down {
        fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
            Err(ScopeFailure::Unreachable { detail: "connection refused".into() })
        }
    }

    fn definition() -> ServiceDefinition {
        ServiceDefinition::new(
            crate::ids::ServiceId::parse("account-analysis").unwrap(),
            1,
            "reads a tenant's own records",
            Composition::new(
                vec![PartRef::new("personas/A.md")],
                "# ORGANIZATION GUIDANCE",
                PartRef::new("personas/_tail.md"),
            )
            .unwrap(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
        )
        .unwrap()
    }

    fn guidance() -> Guidance {
        let text = format!(
            "<!-- tenant-guidance v1 slug: acme service: account-analysis -->\n\n\
             # Company\n\n{}\n",
            "Real content about the organization. ".repeat(20)
        );
        Guidance::parse(&text, "acme", "account-analysis").unwrap()
    }

    fn model() -> ModelSelection {
        ModelSelection::from_pin(&crate::pins::ModelPin::parse("m # c").unwrap())
    }

    fn instance(slug: &str) -> ServiceInstance<Unverified> {
        ServiceInstance::new(
            Slug::parse(slug).unwrap(),
            definition(),
            guidance(),
            RuntimeIdentity::parse("member-1").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "keyring:member"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "keyring:recs"))
                .unwrap(),
            model(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
            &parts(),
        )
        .unwrap()
    }

    fn verified() -> ServiceInstance<Verified> {
        instance("acme").verify(&Store("acme-sales")).unwrap()
    }

    // ── controls fail closed ────────────────────────────────────────────────────────

    #[test]
    fn an_unresolvable_scope_produces_no_instance_at_all() {
        assert!(matches!(instance("acme").verify(&Down), Err(VerifyError::Unresolved { .. })));
    }

    #[test]
    fn a_composition_naming_a_missing_part_refuses_the_whole_instance() {
        let err = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition(),
            guidance(),
            RuntimeIdentity::parse("m").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "a"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "b")).unwrap(),
            model(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
            &Parts(BTreeMap::new()),
        )
        .unwrap_err();
        assert!(matches!(err, InstanceError::MissingParts { missing: 2, .. }));
    }

    #[test]
    fn cross_wired_credentials_are_refused() {
        let err = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition(),
            guidance(),
            RuntimeIdentity::parse("m").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "same"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "same"))
                .unwrap(),
            model(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
            &parts(),
        )
        .unwrap_err();
        assert_eq!(err, InstanceError::CrossWiredCredentials);
    }

    // ── data sources degrade ────────────────────────────────────────────────────────

    fn at() -> crate::record::Timestamp {
        crate::record::Timestamp::parse("2026-08-27T12:00:00+00:00").unwrap()
    }

    /// A question with no records. The `Ask` carries materials only — there is no way to hand it
    /// a rendered envelope, which is the property under test everywhere below.
    fn ask<'a>(
        question: &'a str,
        state: &'a RecordsState,
        at: &'a crate::record::Timestamp,
    ) -> Ask<'a> {
        Ask {
            question,
            records: &[],
            records_state: state,
            retrieved_at: at,
            speaker: None,
            fact_fields: None,
        }
    }

    /// The book's state is declared to the model, never implied by silence — and `Complete` says
    /// nothing, because a sentence explaining that everything is fine is one the model has to
    /// spend attention discounting. These words used to be authored in the CLI.
    #[test]
    fn an_empty_book_is_declared_never_implied() {
        assert!(RecordsState::Empty.note().expect("the model must be told").contains("empty"));
        assert!(RecordsState::Complete.note().is_none());
        // A cause renders to words core chose. A caller cannot supply the sentence.
        let unavailable = RecordsState::Degraded(Degradation::Unavailable);
        assert!(unavailable.note().unwrap().starts_with("temporarily unavailable"));
        let partial = RecordsState::Degraded(Degradation::Partial { unreadable: 3 });
        assert!(partial.note().unwrap().contains("3 record(s)"), "the count reaches the model");
    }

    #[test]
    fn a_degraded_store_still_builds_a_turn() {
        let v = verified();
        let state = RecordsState::Degraded(Degradation::Unavailable);
        let (t, s) = (at(), state);
        let turn = v.build_turn(&parts(), &ask("what is outstanding?", &s, &t), None).unwrap();
        assert!(!turn.input().records().has_records());
        assert!(!turn.instructions().is_empty(), "the alloy is still composed");
        // The degradation reason reaches the model, rendered here rather than by the caller.
        assert!(turn.input().as_str().contains("temporarily unavailable"));
    }

    #[test]
    fn an_empty_book_still_builds_a_turn_and_says_so() {
        let v = verified();
        let turn =
            v.build_turn(&parts(), &ask("hello", &RecordsState::Empty, &at()), None).unwrap();
        assert_eq!(*turn.input().records(), RecordsState::Empty);
    }

    // ── the turn itself ─────────────────────────────────────────────────────────────

    #[test]
    fn a_verified_instance_builds_a_turn_from_its_own_composition() {
        let v = verified();
        let turn = v.build_turn(&parts(), &ask("q", &RecordsState::Complete, &at()), None).unwrap();
        assert_eq!(turn.identity().org().as_str(), "acme-sales");
        assert_eq!(turn.identity().model().as_str(), "m");
        // Composed here, not accepted: the instructions necessarily belong to this instance.
        assert!(turn.instructions().as_str().starts_with("# A"));
        assert!(turn.instructions().as_str().ends_with("## Safety\n\nrules"));
    }

    /// The wire this patch exists to close: marker -> validated [`Guidance`] -> instance ->
    /// [`Ask`] -> the one renderer.
    ///
    /// Both serving paths read the policy from the verified instance, so neither can render a
    /// tenant's records under a declaration that tenant did not write, and the two cannot drift
    /// apart into a second, looser behaviour.
    #[test]
    fn a_verified_instance_carries_its_tenants_declared_facts_into_the_turn() {
        let text = format!(
            "<!-- tenant-guidance v1 slug: acme service: account-analysis \
             facts: [\"work_order\",\"commitment\"] -->\n\n# Company\n\n{}\n",
            "Real content about the organization. ".repeat(20)
        );
        let declared = Guidance::parse(&text, "acme", "account-analysis").unwrap();
        let v = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition(),
            declared,
            RuntimeIdentity::parse("member-1").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "keyring:member"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "keyring:recs"))
                .unwrap(),
            model(),
            CapabilityProfile::READ_ONLY_RECORDS,
            None,
            &parts(),
        )
        .unwrap()
        .verify(&Store("acme-sales"))
        .unwrap();

        assert_eq!(v.fact_fields(), ["work_order", "commitment"]);

        // `commitment` is recorded, `work_order` is declared and absent, `zebra` is neither.
        let record = crate::record::AccountRecord::from_json(
            r#"{"record_id":"AC-1","account":{"name":"Acme",
                 "facts":{"zebra":"z","commitment":"monthly summary"}},
               "contacts":[],"activities":[]}"#,
        )
        .unwrap();
        let turn = v
            .build_turn(
                &parts(),
                &Ask {
                    question: "q",
                    records: std::slice::from_ref(&record),
                    records_state: &RecordsState::Complete,
                    retrieved_at: &at(),
                    speaker: None,
                    fact_fields: Some(v.fact_fields()),
                },
                None,
            )
            .unwrap();
        let rendered = turn.input().as_str();

        assert!(rendered.contains("commitment: monthly summary"));
        assert!(
            rendered.contains("zebra: z"),
            "an undeclared key is still recorded, so it renders"
        );
        assert!(
            rendered.find("commitment:").unwrap() < rendered.find("zebra:").unwrap(),
            "declared keys render before undeclared ones, in declared order"
        );
        // Only the declared-and-absent key is a gap. `zebra` is present, and nothing invents the
        // sales-shaped columns this implementation deliberately has no fallback to.
        assert!(rendered.contains("missing fields (genuinely unknown): work_order"));
        for invented in ["budget", "timeline", "decision_process", "economic_buyer"] {
            assert!(!rendered.contains(invented), "{invented} was never declared or recorded");
        }
    }

    #[test]
    fn a_thread_pointer_from_another_instance_is_refused() {
        let a = verified();
        let b = instance("globex").verify(&Store("globex-sales")).unwrap();
        let foreign = b.thread_scope().pointer(ResponseId::parse("resp_1").unwrap());
        let err = a
            .build_turn(&parts(), &ask("q", &RecordsState::Complete, &at()), Some(foreign))
            .unwrap_err();
        assert!(matches!(err, TurnError::ForeignThread { .. }));
    }

    #[test]
    fn its_own_thread_pointer_is_accepted() {
        let v = verified();
        let own = v.thread_scope().pointer(ResponseId::parse("resp_1").unwrap());
        let turn =
            v.build_turn(&parts(), &ask("q", &RecordsState::Complete, &at()), Some(own)).unwrap();
        assert_eq!(turn.previous().unwrap().response().as_str(), "resp_1");
    }

    #[test]
    fn verification_replaces_the_claim_and_surfaces_disagreement() {
        let claimed = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition(),
            guidance(),
            RuntimeIdentity::parse("m").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "a"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "b")).unwrap(),
            model(),
            CapabilityProfile::READ_ONLY_RECORDS,
            Some(OrgId::parse("stale-claim").unwrap()),
            &parts(),
        )
        .unwrap();
        let v = claimed.verify(&Store("acme-sales")).unwrap();
        assert_eq!(v.org().as_str(), "acme-sales", "the store's answer wins");
        assert!(!v.claim_matched());
    }

    #[test]
    fn instance_capabilities_are_attenuated_to_the_definition_ceiling() {
        let greedy = CapabilityProfile {
            records: crate::capability::Access::ReadOnly,
            writes: crate::capability::Access::ReadOnly,
            egress: crate::capability::Access::ReadOnly,
            outreach: crate::capability::Access::ReadOnly,
            runtime_tools: crate::capability::ToolAccess::Sandbox,
        };
        let i = ServiceInstance::new(
            Slug::parse("acme").unwrap(),
            definition(),
            guidance(),
            RuntimeIdentity::parse("m").unwrap(),
            CredentialRef::new(CredentialPurpose::RuntimeMember, "a"),
            ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "b")).unwrap(),
            model(),
            greedy,
            None,
            &parts(),
        )
        .unwrap();
        assert_eq!(*i.capabilities(), CapabilityProfile::READ_ONLY_RECORDS);
    }
}
