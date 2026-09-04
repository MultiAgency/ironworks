//! Operator-side lifecycle state: intent, progress, and observed authority.
//!
//! # The question this answers
//!
//! > **What authority currently exists for this service instance, what did IronWorks create, and
//! > what remains uncertain?**
//!
//! That is a bigger question than "did provisioning finish", and it is the one the reference
//! implementation learned to ask the hard way. Provisioning is not primarily a sequence of API
//! calls; it is reconciliation between local intent and authority that lives in other people's
//! systems.
//!
//! # Three concepts, kept apart
//!
//! | | Answers | Lives in |
//! |---|---|---|
//! | [`InstanceSpec`] | what an operator asked for | [`state`] |
//! | [`ProvisionProgress`] | how far IronWorks got | [`state`] |
//! | [`AuthorityInventory`] | what the authoritative systems say exists | [`authority`] |
//!
//! Merging any two of them is how the reference's worst crash windows became invisible. A stage
//! named `org_registered` reads as an assertion about the world; it is a note about ourselves.
//!
//! # Why this is not `ironworks-journal`
//!
//! The turn journal owns durability on the **serving path** — what became of one turn, and what
//! may safely be retried or delivered. This owns **operator** state, is read by no serving code,
//! and outlives any individual turn by months. Same durability discipline, different database:
//! explicit schema version, fail closed on an unknown one, atomic writes, and read-only
//! inspection that cannot mutate.
//!
//! # What it must never hold
//!
//! No credential values, ever. It records stage names, opaque identifiers, timestamps and
//! classifications — every one of which is already non-secret. A token here would be a second
//! copy of every client credential in a file whose whole purpose is to outlive a crash.
//!
//! The reference enforces that with a field-name blacklist, because its CLI takes `k=v` pairs
//! from shell and a future caller passing `token=$TOK` would otherwise write one. **This crate
//! has no such surface** — every write goes through a typed method whose parameters are
//! [`Slug`](ironworks_domain::Slug), [`OpaqueId`], [`Stage`] and timestamps, so there is nowhere to
//! put a secret rather than a rule against putting one there. The proof is a test that reads the
//! database file's bytes and looks for sentinels.
//!
//! # Not in this slice
//!
//! Deprovisioning, revocation probes, the residual-authority ledger, suspension, live
//! reconciliation adapters, and serving-process reload. [`Reconcile`] is a trait with fixture
//! implementations only — the point is to make the crash boundaries representable and testable
//! *before* wiring foreign systems, so the adapters arrive into a shape whose semantics are
//! already proven.
//!
//! Suspension is absent deliberately, not pending. The upstream `admin/users/<id>/status`
//! endpoint exists, but at the pinned revision the session store consults only the signature,
//! expiry, a process-local revoked set and the tenant — it never reads the user directory. So
//! suspending a member does not remove product-surface authority from an already-issued session,
//! and modelling it as an authority-removal state would be a lie in the type system.

use ironworks_domain::{ChannelRef, ScopeEvidence, ServiceId, Slug};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

pub mod attempt;
pub mod authority;
pub mod confinement;
pub mod inspect;
pub mod runtime;
pub mod state;

pub use attempt::{
    AttemptVariant, Blocked, CatalogueResult, ConfinementAttempt, ConfinementEvidence, Historical,
    Measurement, Reached,
};
pub use authority::{
    Authority, AuthorityFact, AuthorityInventory, Custody, Holding, NextAction, Presence,
    Reconcile, holdings, plan,
};
pub use confinement::{
    AUTHORITY_CHANGING, Confinement, ConfinementFacts, ConfinementPolicy, Drift, Plan,
    PolicyBinding, PolicyFingerprint, Step, ToolState, assess,
};
pub use inspect::{
    ConfinementAxis, ConfinementOutcome, ConfinementReport, Finding, Inspection, Ledger,
    LiveConfinement, NoReconciler, Verdict, inspect,
};
pub use runtime::{
    BoundSubject, ObservedAt, RuntimeIdError, RuntimeSubject, RuntimeTenantId, RuntimeUserId,
    SubjectMismatch,
};
pub use state::{
    InstanceSpec, ProvisionProgress, ServingObservation, SmokeEvidence, SmokeOutcome, Stage,
};

/// How an authority identifier entered the lifecycle record.
///
/// This distinction controls compensation: IronWorks may reverse authority it created, but an
/// adopted identifier is an observation about somebody else's prior action and is never ours to
/// remove merely because this deployment later fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityProvenance {
    Created,
    Adopted,
}

impl IdentityProvenance {
    fn tag(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Adopted => "adopted",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "created" => Some(Self::Created),
            "adopted" => Some(Self::Adopted),
            _ => None,
        }
    }
}

/// One foreign authority identifier, with the provenance lifecycle must not erase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityIdentity {
    id: OpaqueId,
    provenance: IdentityProvenance,
    runtime_subject: Option<RuntimeSubject>,
}

impl AuthorityIdentity {
    pub fn created(id: OpaqueId) -> Self {
        Self { id, provenance: IdentityProvenance::Created, runtime_subject: None }
    }

    pub fn adopted(id: OpaqueId) -> Self {
        Self { id, provenance: IdentityProvenance::Adopted, runtime_subject: None }
    }

    pub fn id(&self) -> &OpaqueId {
        &self.id
    }

    pub fn provenance(&self) -> IdentityProvenance {
        self.provenance
    }

    /// The full subject measured from `/session`, when this identity was adopted that way.
    ///
    /// Older created identities predate this observation and correctly return `None`; absence is
    /// not rewritten into a guessed tenant.
    pub fn runtime_subject(&self) -> Option<&RuntimeSubject> {
        self.runtime_subject.as_ref()
    }
}

/// What IronWorks durably recorded about its own actions and observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub progress: ProvisionProgress,
    identities: std::collections::BTreeMap<Authority, AuthorityIdentity>,
}

impl Recorded {
    /// Just the progress, for a caller with no interest in identifiers.
    pub fn from_progress(progress: ProvisionProgress) -> Self {
        Self { progress, identities: std::collections::BTreeMap::new() }
    }

    /// Construct a record of authority IronWorks created.
    pub fn from_created(
        progress: ProvisionProgress,
        ids: impl IntoIterator<Item = (Authority, OpaqueId)>,
    ) -> Self {
        Self {
            progress,
            identities: ids
                .into_iter()
                .map(|(authority, id)| (authority, AuthorityIdentity::created(id)))
                .collect(),
        }
    }

    pub fn identity(&self, authority: Authority) -> Option<&AuthorityIdentity> {
        self.identities.get(&authority)
    }

    pub fn accounted_id(&self, authority: Authority) -> Option<&OpaqueId> {
        self.identity(authority).map(AuthorityIdentity::id)
    }

    pub fn created_id(&self, authority: Authority) -> Option<&OpaqueId> {
        self.identity(authority)
            .filter(|identity| identity.provenance == IdentityProvenance::Created)
            .map(AuthorityIdentity::id)
    }
}

/// The two authoritative answers required to adopt an existing instance.
///
/// The organization is copied from [`ScopeEvidence`], not accepted as a string, and the member is
/// the complete subject returned by the runtime. This value creates no foreign authority; it is
/// consumed only by [`LifecycleStore::adopt`] to record what those systems already reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionEvidence {
    org: ironworks_domain::OrgId,
    subject: RuntimeSubject,
}

impl AdoptionEvidence {
    pub fn observed(scope: &ScopeEvidence, subject: RuntimeSubject) -> Self {
        Self { org: scope.org().clone(), subject }
    }

    pub fn org(&self) -> &ironworks_domain::OrgId {
        &self.org
    }

    pub fn subject(&self) -> &RuntimeSubject {
        &self.subject
    }
}

/// An identifier a foreign system uses for something it holds. **Never a credential.**
///
/// A newtype rather than `String` so that "the org id" and "the sealed user id" cannot be swapped
/// by position, and so that the one place a value enters lifecycle state is visible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpaqueId(String);

impl OpaqueId {
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OpaqueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why something could not be measured.
///
/// A reason is required rather than optional: "could not check" with no account of why is the
/// same as no answer at all, and it is the state an operator has to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedReason {
    /// The authoritative system did not answer.
    Unreachable { system: String },
    /// It answered something unusable.
    Malformed { detail: String },
    /// No credential was available to ask with.
    NoCredential { system: String },
    /// The reconciler returned an inventory that never mentioned this authority. Silence is not
    /// absence.
    NotObserved,
    /// The credential we hold was refused. Evidence about the credential, never about whether
    /// the authority it would have opened exists — a different question, needing a different
    /// asker.
    CredentialRejected,
    /// Something exists at an authority whose stage we reached, but we never recorded which
    /// identifier we created — so we cannot say whether the thing out there is ours.
    NoRecordedIdentity,
    /// The lifecycle store accounts for no instances, so there is nothing to reconcile against.
    ///
    /// A set difference against an empty set is not an answer. "Every member on the instance is
    /// accounted for" is trivially true of a store that accounts for nothing — and identically
    /// true of a `--store` pointing at the wrong path, which is the likelier cause. The reference
    /// takes the same stance and exits rather than certifying: *"no client envs in CLIENTS_DIR —
    /// nothing to reconcile (fail closed)"*.
    NothingToReconcile,
    /// No reconciler was supplied at all, so nothing was asked of anyone.
    ///
    /// The honest default for an inspection with no way to observe the world. It yields
    /// `BLOCKED`, never `PASS` — a report that certified an instance clean without asking any
    /// authoritative system would be the exact failure this crate exists to prevent.
    NoReconciler,
}

impl std::fmt::Display for BlockedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable { system } => write!(f, "{system} did not answer"),
            Self::Malformed { detail } => write!(f, "unusable answer: {detail}"),
            Self::NoCredential { system } => write!(f, "no credential to ask {system} with"),
            Self::NotObserved => f.write_str("reconciliation never mentioned it"),
            Self::CredentialRejected => f.write_str(
                "the credential was refused, which says nothing about whether the \
                             authority exists",
            ),
            Self::NoRecordedIdentity => f.write_str(
                "the stage was reached but no identifier was recorded, so this cannot \
                             be matched to anything we created",
            ),
            Self::NothingToReconcile => f.write_str(
                "the lifecycle store accounts for no instances, so a set difference against it \
                 proves nothing",
            ),
            Self::NoReconciler => f.write_str("no reconciler was supplied, so nothing was asked"),
        }
    }
}

/// The four nullable columns progress is read from: stage, then the three smoke fields that are
/// present together or not at all.
type StoredProgress = (Option<String>, Option<String>, Option<String>, Option<String>);

/// What `open` initialises and migrates to.
///
/// `open_readonly` accepts every known version. Older stores hold valid lifecycle state; refusing
/// them would make inspection impossible until somebody ran a writing command.
const SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS lifecycle_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS instances (
    slug         TEXT PRIMARY KEY,
    -- Desired configuration. What an operator asked for.
    org          TEXT NOT NULL,
    service      TEXT NOT NULL,
    channel      TEXT NOT NULL,
    -- Progress. NULL means nothing has been reached, which says nothing about the world.
    stage        TEXT,
    -- Present exactly when activated. Activation cannot be recorded without them, which is how
    -- `activated` stays impossible to claim without a passing smoke.
    smoke_isolation    TEXT,
    smoke_reachability TEXT,
    smoke_at           TEXT,
    -- A separate axis. 'unknown' until something observes it; never inferred from `stage`.
    serving      TEXT NOT NULL DEFAULT 'unknown',
    serving_at   TEXT,
    started_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS stage_history (
    slug  TEXT NOT NULL,
    stage TEXT NOT NULL,
    at    TEXT NOT NULL,
    -- The opaque identifier the foreign system gave whatever this stage created, when it created
    -- something. Without it, custody can only ask whether we reached the stage -- and a stray
    -- member at a stage we did reach then reads as one we minted, which is precisely the case
    -- the unmanaged state exists to catch.
    created_id TEXT,
    -- Whether this stage was reached by DOING the work or by finding the authority already there.
    -- An operator asking 'did we create this?' months later cannot reconstruct it otherwise.
    adopted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS stage_history_slug ON stage_history(slug, at);
";

/// The confinement ledger, added at schema 2.
///
/// # Append-only, and why the parent carries variant-gated nullable columns
///
/// A `confinement_bound` child table would give `NOT NULL` columns, and would *lose* the
/// invariant: SQLite cannot express "a `measured` parent has exactly one row in that child". The
/// paired `CHECK`s below express the variant-to-column correspondence directly and are enforced on
/// every insert, so the nullable columns are the stronger design here rather than the lazier one.
///
/// The two child tables exist for **cardinality** alone: a measurement names 0..n reachable ids and
/// 0..n drift entries, and neither can be a column.
///
/// `CREATE TABLE` is written **without** `IF NOT EXISTS` on purpose. Inside a transaction that
/// makes the migration idempotent-by-rollback rather than idempotent-by-ignoring: an interrupted
/// run leaves nothing behind and retries cleanly, while a pre-existing object of the wrong shape
/// fails loudly and repeatably instead of being adopted.
const SCHEMA_V2: &str = "
CREATE TABLE confinement_attempts (
    -- rowid: monotone append order, and the tiebreak when two attempts share a timestamp.
    -- `now_iso` has one-second granularity, and one `confine` run appends its initial and final
    -- attempts inside the same second routinely, so this is load-bearing rather than defensive.
    id       INTEGER PRIMARY KEY,
    slug     TEXT NOT NULL REFERENCES instances(slug) ON DELETE RESTRICT,
    variant  TEXT NOT NULL CHECK (variant IN
                 ('binding_blocked','subject_mismatch','measured','catalogue_blocked')),
    expected TEXT NOT NULL CHECK (length(expected) > 0),
    at       TEXT NOT NULL CHECK (length(at) >= 20),

    subject_tenant     TEXT,      -- iff variant <> 'binding_blocked'
    subject_user       TEXT,
    service            TEXT,      -- iff variant IN ('measured','catalogue_blocked')
    service_version    INTEGER,
    policy_fingerprint TEXT,
    catalogued         INTEGER,   -- iff variant = 'measured'
    auto_approve       INTEGER,
    blocked_kind       TEXT,      -- iff variant IN ('binding_blocked','catalogue_blocked')

    -- The variant tag and the columns it implies, in both directions. `subject_mismatch` is
    -- therefore forced to carry a subject and NOTHING else: no service, no policy, no catalogue,
    -- no blocked kind -- which is the shape of an attempt that made no settings call.
    CHECK ((variant =  'binding_blocked')                       = (subject_tenant IS NULL)),
    CHECK ((subject_tenant IS NULL)                             = (subject_user IS NULL)),
    CHECK ((variant IN ('measured','catalogue_blocked'))        = (service IS NOT NULL)),
    CHECK ((service IS NULL)                                    = (service_version IS NULL)),
    CHECK ((service IS NULL)                                    = (policy_fingerprint IS NULL)),
    CHECK ((variant =  'measured')                              = (catalogued IS NOT NULL)),
    CHECK ((catalogued IS NULL)                                 = (auto_approve IS NULL)),
    CHECK ((variant IN ('binding_blocked','catalogue_blocked')) = (blocked_kind IS NOT NULL)),

    CHECK (auto_approve       IS NULL OR auto_approve IN (0,1)),
    CHECK (catalogued         IS NULL OR catalogued >= 0),
    CHECK (service_version    IS NULL OR service_version > 0),
    CHECK (subject_user       IS NULL OR length(subject_user) > 0),
    CHECK (subject_tenant     IS NULL OR length(subject_tenant) > 0),
    CHECK (policy_fingerprint IS NULL OR length(policy_fingerprint) = 16),
    CHECK (blocked_kind       IS NULL OR length(blocked_kind) > 0)
);
CREATE INDEX confinement_attempts_latest ON confinement_attempts(slug, at DESC, id DESC);
-- The applicable-history lookup: latest attempt for the CURRENTLY recorded member. Filtering by
-- member is what stops a previous member's history deciding the current verdict.
CREATE INDEX confinement_attempts_member
    ON confinement_attempts(slug, expected, at DESC, id DESC);

CREATE TABLE confinement_reachable (
    attempt_id INTEGER NOT NULL REFERENCES confinement_attempts(id) ON DELETE CASCADE,
    tool       TEXT NOT NULL CHECK (length(tool) > 0),
    PRIMARY KEY (attempt_id, tool)
) WITHOUT ROWID;

CREATE TABLE confinement_drift (
    attempt_id INTEGER NOT NULL REFERENCES confinement_attempts(id) ON DELETE CASCADE,
    ordinal    INTEGER NOT NULL CHECK (ordinal >= 0),
    kind       TEXT NOT NULL CHECK (kind IN
                   ('authority_changing_reachable','reachable','unrecognized',
                    'auto_approve','allowed_tool_absent')),
    tool       TEXT,
    state      TEXT,
    observed   INTEGER,
    expected   INTEGER,
    PRIMARY KEY (attempt_id, ordinal),
    -- Exact per-kind field requirements, not merely the auto_approve distinction.
    --
    -- Every presence test is spelled `IS NOT NULL AND <test>` rather than relying on the test
    -- alone. SQLite evaluates `NULL IN (0,1)` to NULL, and a CHECK whose expression is NULL
    -- PASSES -- so `observed IN (0,1)` would have admitted an auto_approve row with no booleans
    -- at all. Three-valued logic makes the obvious spelling of this constraint vacuous, and it
    -- was measured admitting exactly that row before this was corrected.
    CHECK (
      (kind = 'auto_approve'
         AND tool IS NULL AND state IS NULL
         AND observed IS NOT NULL AND observed IN (0,1)
         AND expected IS NOT NULL AND expected IN (0,1))
      OR
      (kind = 'allowed_tool_absent'
         AND tool IS NOT NULL AND length(tool) > 0 AND state IS NULL
         AND observed IS NULL AND expected IS NULL)
      OR
      (kind IN ('authority_changing_reachable','reachable','unrecognized')
         AND tool IS NOT NULL AND length(tool) > 0
         AND state IS NOT NULL AND length(state) > 0
         AND observed IS NULL AND expected IS NULL)
    )
) WITHOUT ROWID;
";

/// Every v2 object, so `validate_v2` checks a list rather than a memory.
const V2_OBJECTS: [&str; 5] = [
    "confinement_attempts",
    "confinement_attempts_latest",
    "confinement_attempts_member",
    "confinement_reachable",
    "confinement_drift",
];

/// Provenance-aware authority identities, added at schema 3.
///
/// `stage_history.created_id` is retained as historical storage for v1/v2 readers. This table is
/// the authoritative identity view from v3 onward: an adopted id is accounted for without being
/// relabelled as something IronWorks created. A runtime subject is present only when `/session`
/// supplied the member identity; older rows remain honest by carrying no inferred tenant.
const SCHEMA_V3: &str = "
CREATE TABLE authority_identities (
    slug       TEXT NOT NULL REFERENCES instances(slug) ON DELETE RESTRICT,
    authority  TEXT NOT NULL CHECK (authority IN
                   ('org_registration','tenant_data','member','member_confinement','registry_entry')),
    opaque_id  TEXT NOT NULL CHECK (length(opaque_id) > 0),
    provenance TEXT NOT NULL CHECK (provenance IN ('created','adopted')),
    observed_at TEXT NOT NULL CHECK (length(observed_at) >= 20),
    subject_tenant TEXT,
    subject_user   TEXT,
    PRIMARY KEY (slug, authority),
    CHECK ((subject_tenant IS NULL) = (subject_user IS NULL)),
    CHECK (subject_tenant IS NULL OR authority = 'member'),
    CHECK (subject_user IS NULL OR subject_user = opaque_id),
    CHECK (subject_tenant IS NULL OR length(subject_tenant) > 0),
    CHECK (subject_user IS NULL OR length(subject_user) > 0)
) WITHOUT ROWID;

INSERT INTO authority_identities(
    slug, authority, opaque_id, provenance, observed_at, subject_tenant, subject_user
)
SELECT slug,
       CASE stage
         WHEN 'org_registered' THEN 'org_registration'
         WHEN 'data_initialization_settled' THEN 'tenant_data'
         WHEN 'member_minted' THEN 'member'
         WHEN 'member_confined' THEN 'member_confinement'
         WHEN 'staged' THEN 'registry_entry'
       END,
       created_id,
       CASE adopted WHEN 1 THEN 'adopted' ELSE 'created' END,
       at,
       NULL,
       NULL
FROM stage_history
WHERE created_id IS NOT NULL
  AND stage IN ('org_registered','data_initialization_settled','member_minted',
                'member_confined','staged');
";

/// What a file turned out to be, decided by reads alone.
///
/// Discovery happens **before** any pragma or DDL, because `PRAGMA journal_mode = WAL` rewrites the
/// database header and creates sidecar files — so a store at an unknown version would have been
/// mutated by the very command that then refused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// No objects at all. The only thing safe to initialise over.
    Empty,
    /// Objects, but no usable lifecycle metadata. Never adopted: it may be another product's file,
    /// or a store whose creation was interrupted, and initialising over either would destroy it.
    Foreign,
    V1,
    V2,
    V3,
    Unknown(i64),
}

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("the lifecycle store at {path} could not be opened: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "the lifecycle store at {path} is not a readable database. Refusing to run — and do NOT delete it \
         blind: it is the record of which tenants exist and what a half-finished provisioning run left behind. Move it aside and keep it."
    )]
    NotADatabase { path: String },
    #[error(
        "the lifecycle store at {path} was created but could not be restricted to its owner: {source}. \
         Refusing to run: the file names every organization served and every turn outstanding, \
         and continuing would leave that readable while reporting success."
    )]
    Exposed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the lifecycle store is schema version {found}, this implementation writes {expected}. \
         Refusing to run: an unknown version means a downgrade or an unfinished write, and \
         guessing which would risk reporting authority that exists as absent."
    )]
    Version { found: i64, expected: i64 },
    #[error(
        "the lifecycle store at {path} holds no lifecycle schema. Refusing to read it — and do \
         NOT delete it: a file with objects but no lifecycle metadata is either another program's \
         database or a store whose creation was interrupted, and initialising over either would \
         destroy it. Open it with a writing command if it is meant to be a fresh store."
    )]
    Uninitialized { path: String },
    #[error("no lifecycle record for {slug}")]
    NoSuchInstance { slug: String },
    #[error(
        "instance {slug} already exists; adoption is a one-time provenance record and never an upsert"
    )]
    AlreadyExists { slug: String },
    #[error(
        "instance {slug} is recorded at stage {recorded}, which is at or past {attempted} — a \
         stage is never walked backwards, because a later run finding less than an earlier one \
         is evidence about the observer, not about the world"
    )]
    StageWouldRegress { slug: String, recorded: String, attempted: String },
    #[error(
        "instance {slug} cannot reach {attempted}: {predecessor} was never reached. A stage \
         records that the work before it is done, so recording one whose predecessor is missing \
         would let a member be staged before it was confined"
    )]
    StageSkipped { slug: String, attempted: String, predecessor: String },
    #[error(
        "instance {slug} cannot reach member_confined through generic stage progress; a clean, \
         current-policy observation must be supplied to certify_confinement"
    )]
    ConfinementEvidenceRequired { slug: String },
    #[error(
        "confinement evidence is for member {observed}, but instance {slug} records member \
         {recorded}; refusing to certify another subject"
    )]
    ConfinementMemberMismatch { slug: String, recorded: String, observed: String },
    #[error(
        "confinement evidence is for service {observed}, but instance {slug} declares service \
         {recorded}; refusing to certify another service's policy"
    )]
    ConfinementServiceMismatch { slug: String, recorded: String, observed: String },
    #[error(
        "instance {slug} cannot certify confinement because no runtime member identity is recorded"
    )]
    ConfinementMemberMissing { slug: String },
    #[error(
        "instance {slug} cannot be activated from stage {stage}: activation requires every stage \
         through {last} and a passing smoke"
    )]
    NotReadyToActivate { slug: String, stage: String, last: String },
    #[error("this lifecycle store is open read-only; inspection has no side effects")]
    ReadOnly,
    #[error("the lifecycle store holds an unusable {what}")]
    Corrupt { what: &'static str },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// What `open` must do once it has decided the file is safe to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prepare {
    Initialise,
    MigrateV1,
    MigrateV2,
    Validate,
}

/// Read the file's shape without changing it.
///
/// `sqlite_master` is queried rather than `lifecycle_meta` directly, so a brand-new file is
/// `Empty` instead of a "no such table" error — and so nothing has to INSERT a version row to find
/// out what version it is, which the previous implementation did and which cannot work on a
/// read-only connection.
fn discover(path: &Path, conn: &Connection) -> Result<Shape, LifecycleError> {
    let objects: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .map_err(|e| opened(path, e))?;
    if objects == 0 {
        return Ok(Shape::Empty);
    }
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='lifecycle_meta'",
        [],
        |r| r.get(0),
    )?;
    if has_meta == 0 {
        return Ok(Shape::Foreign);
    }
    let stored: Option<String> = conn
        .query_row("SELECT value FROM lifecycle_meta WHERE key='schema_version'", [], |r| r.get(0))
        .optional()?;
    // A `lifecycle_meta` with no version row is not a fresh store. Something created the table and
    // stopped, and initialising over it would destroy whatever else it created.
    let Some(stored) = stored else { return Ok(Shape::Foreign) };
    let Ok(found) = stored.parse::<i64>() else {
        return Err(LifecycleError::Corrupt { what: "schema version" });
    };
    Ok(match found {
        1 => Shape::V1,
        2 => Shape::V2,
        3 => Shape::V3,
        other => Shape::Unknown(other),
    })
}

/// A brand-new store is born at the current schema, in one transaction.
fn init_current(conn: &Connection) -> Result<(), LifecycleError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA)?;
    tx.execute_batch(SCHEMA_V2)?;
    tx.execute_batch(SCHEMA_V3)?;
    tx.execute(
        "INSERT INTO lifecycle_meta(key,value) VALUES('schema_version',?1)",
        params![SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Add provenance-aware identities to a valid v2 store, in one transaction.
fn migrate_v2_to_v3(conn: &Connection) -> Result<(), LifecycleError> {
    validate_v2(conn)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_V3)?;
    tx.execute(
        "UPDATE lifecycle_meta SET value=?1 WHERE key='schema_version'",
        params![SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Migrate a v1 store through both additive schemas without exposing an intermediate commit.
fn migrate_v1_to_v3(conn: &Connection) -> Result<(), LifecycleError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA_V2)?;
    tx.execute_batch(SCHEMA_V3)?;
    tx.execute(
        "UPDATE lifecycle_meta SET value=?1 WHERE key='schema_version'",
        params![SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

/// A store claiming v2 must actually have the v2 objects.
///
/// A version row is a claim; this is the check. Without it, a truncated or half-restored file would
/// report version 2 and then fail on first use with a bare SQLite error.
fn validate_v2(conn: &Connection) -> Result<(), LifecycleError> {
    for object in V2_OBJECTS {
        let present: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name=?1",
            params![object],
            |r| r.get(0),
        )?;
        if present == 0 {
            return Err(LifecycleError::Corrupt { what: "the v2 confinement ledger schema" });
        }
    }
    Ok(())
}

fn validate_v3(conn: &Connection) -> Result<(), LifecycleError> {
    validate_v2(conn)?;
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='authority_identities'",
        [],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Err(LifecycleError::Corrupt { what: "the v3 authority identity schema" });
    }
    Ok(())
}

/// One drift entry's columns. The set is variant-exact, and the SQL `CHECK` enforces which of them
/// may be present for each kind.
struct DriftColumns {
    kind: &'static str,
    tool: Option<String>,
    state: Option<String>,
    observed: Option<i64>,
    expected: Option<i64>,
}

fn drift_columns(drift: &Drift) -> DriftColumns {
    let kind = drift.kind_tag();
    match drift {
        Drift::AuthorityChangingReachable { tool, state } | Drift::Reachable { tool, state } => {
            DriftColumns {
                kind,
                tool: Some(tool.clone()),
                state: Some(state.wire().to_owned()),
                observed: None,
                expected: None,
            }
        }
        Drift::Unrecognized { tool, raw } => DriftColumns {
            kind,
            tool: Some(tool.clone()),
            state: Some(raw.clone()),
            observed: None,
            expected: None,
        },
        Drift::AllowedToolAbsent { tool } => DriftColumns {
            kind,
            tool: Some(tool.clone()),
            state: None,
            observed: None,
            expected: None,
        },
        Drift::AutoApprove { observed, expected } => DriftColumns {
            kind,
            tool: None,
            state: None,
            observed: Some(i64::from(*observed)),
            expected: Some(i64::from(*expected)),
        },
    }
}

/// Rebuild one drift entry, refusing anything this build cannot interpret.
///
/// A `reachable` row whose state parses to [`ToolState::Unrecognized`] is corrupt rather than
/// tolerated: `assess` emits `Drift::Unrecognized` for an unknown state and `Drift::Reachable` only
/// for a known one, so that pairing is impossible to have written and is therefore damage.
fn drift_from_columns(
    kind: &str,
    tool: Option<String>,
    state: Option<String>,
    observed: Option<i64>,
    expected: Option<i64>,
) -> Result<Drift, LifecycleError> {
    let corrupt = || LifecycleError::Corrupt { what: "a confinement drift row" };
    let known_state = |raw: Option<String>| -> Result<ToolState, LifecycleError> {
        let parsed = ToolState::parse(&raw.ok_or_else(corrupt)?);
        if matches!(parsed, ToolState::Unrecognized(_)) { Err(corrupt()) } else { Ok(parsed) }
    };
    match kind {
        "authority_changing_reachable" => Ok(Drift::AuthorityChangingReachable {
            tool: tool.ok_or_else(corrupt)?,
            state: known_state(state)?,
        }),
        "reachable" => {
            Ok(Drift::Reachable { tool: tool.ok_or_else(corrupt)?, state: known_state(state)? })
        }
        "unrecognized" => Ok(Drift::Unrecognized {
            tool: tool.ok_or_else(corrupt)?,
            raw: state.ok_or_else(corrupt)?,
        }),
        "allowed_tool_absent" => Ok(Drift::AllowedToolAbsent { tool: tool.ok_or_else(corrupt)? }),
        "auto_approve" => Ok(Drift::AutoApprove {
            observed: observed.ok_or_else(corrupt)? != 0,
            expected: expected.ok_or_else(corrupt)? != 0,
        }),
        _ => Err(corrupt()),
    }
}

/// Rebuild one attempt, re-applying every invariant its constructors discharged.
///
/// # It fails closed, and never into absence
///
/// Every refusal here is [`LifecycleError::Corrupt`]. A row this cannot interpret is damage, and
/// reporting it as "no observation" would turn a broken ledger into a clean-looking one — the exact
/// substitution §F-14 exists to refuse.
///
/// Two of the checks are free, and they are the interesting ones: reconstruction routes through
/// [`BoundSubject::bind`], so a stored `measured` row whose subject is not its expected member, or a
/// stored `subject_mismatch` row whose subject *is*, cannot be rebuilt at all.
fn read_attempt(conn: &Connection, id: i64) -> Result<attempt::ConfinementAttempt, LifecycleError> {
    use attempt::{AttemptVariant, Blocked, ConfinementAttempt, Measurement};

    let corrupt = |what: &'static str| LifecycleError::Corrupt { what };
    type Row = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    );
    let row: Row = conn.query_row(
        "SELECT variant, expected, at, subject_tenant, subject_user, service, service_version,
                policy_fingerprint, catalogued, auto_approve, blocked_kind
         FROM confinement_attempts WHERE id=?1",
        params![id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        },
    )?;
    let (
        variant,
        expected,
        at,
        subject_tenant,
        subject_user,
        service,
        service_version,
        fingerprint,
        catalogued,
        auto_approve,
        blocked_kind,
    ) = row;

    let variant = AttemptVariant::from_tag(&variant)
        .ok_or_else(|| corrupt("a confinement attempt variant"))?;
    let expected = OpaqueId::new(expected);
    let at =
        runtime::ObservedAt::parse(&at).map_err(|_| corrupt("a confinement attempt timestamp"))?;

    // Children belong to a measurement and nothing else. Their presence under any other variant
    // makes the WHOLE attempt corrupt: SQLite cannot express this — a child-table CHECK cannot see
    // the parent's variant — so it is a read-side invariant by necessity, not by preference.
    let mut reachable = std::collections::BTreeSet::new();
    {
        let mut stmt =
            conn.prepare("SELECT tool FROM confinement_reachable WHERE attempt_id=?1")?;
        for tool in stmt.query_map(params![id], |r| r.get::<_, String>(0))? {
            reachable.insert(tool?);
        }
    }
    let mut drift = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT kind, tool, state, observed, expected FROM confinement_drift
             WHERE attempt_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        for row in rows {
            let (kind, tool, state, observed, expected) = row?;
            drift.push(drift_from_columns(&kind, tool, state, observed, expected)?);
        }
    }
    if variant != AttemptVariant::Measured && !(reachable.is_empty() && drift.is_empty()) {
        return Err(corrupt("a confinement attempt carrying catalogue rows it never measured"));
    }

    let subject = match (subject_tenant, subject_user) {
        (Some(tenant), Some(user)) => Some(runtime::RuntimeSubject::new(
            runtime::RuntimeTenantId::parse(&tenant)
                .map_err(|_| corrupt("a stored runtime tenant id"))?,
            runtime::RuntimeUserId::parse(&user)
                .map_err(|_| corrupt("a stored runtime user id"))?,
        )),
        (None, None) => None,
        _ => return Err(corrupt("a half-stored runtime subject")),
    };
    let binding = match (service, service_version, fingerprint) {
        (Some(service), Some(version), Some(fingerprint)) => {
            let service = ironworks_domain::ServiceId::parse(service)
                .map_err(|_| corrupt("a stored service id"))?;
            let version =
                u32::try_from(version).map_err(|_| corrupt("a stored service version"))?;
            let fingerprint = confinement::PolicyFingerprint::from_stored(&fingerprint)
                .ok_or_else(|| corrupt("a stored policy fingerprint"))?;
            Some(confinement::PolicyBinding::from_stored(service, version, fingerprint))
        }
        (None, None, None) => None,
        _ => return Err(corrupt("a half-stored policy binding")),
    };

    match variant {
        AttemptVariant::BindingBlocked => {
            let reason = Blocked::from_tag(&blocked_kind.ok_or_else(|| corrupt("a blocked kind"))?)
                .ok_or_else(|| corrupt("a blocked kind"))?;
            Ok(ConfinementAttempt::binding_blocked(expected, at, reason))
        }
        AttemptVariant::SubjectMismatch => {
            let subject = subject.ok_or_else(|| corrupt("a subject-mismatch attempt's subject"))?;
            // `bind` must REFUSE here. If it binds, the stored row claims a mismatch between two
            // ids that are equal, which is not a state this build could have written.
            match runtime::BoundSubject::bind(expected, subject) {
                Err(mismatch) => Ok(ConfinementAttempt::subject_mismatch(mismatch, at)),
                Ok(_) => Err(corrupt("a subject-mismatch attempt whose subject matches")),
            }
        }
        AttemptVariant::Measured | AttemptVariant::CatalogueBlocked => {
            let subject = subject.ok_or_else(|| corrupt("a bound attempt's subject"))?;
            let binding = binding.ok_or_else(|| corrupt("a bound attempt's policy"))?;
            // And here `bind` must SUCCEED, for the mirror-image reason.
            let bound = runtime::BoundSubject::bind(expected, subject)
                .map_err(|_| corrupt("a bound attempt whose subject is not its expected member"))?;
            if variant == AttemptVariant::CatalogueBlocked {
                let reason =
                    Blocked::from_tag(&blocked_kind.ok_or_else(|| corrupt("a blocked kind"))?)
                        .ok_or_else(|| corrupt("a blocked kind"))?;
                return Ok(ConfinementAttempt::catalogue_blocked(bound, binding, at, reason));
            }
            let catalogued = catalogued.ok_or_else(|| corrupt("a measured catalogue count"))?;
            let catalogued =
                usize::try_from(catalogued).map_err(|_| corrupt("a measured catalogue count"))?;
            // A catalogue cannot have reported fewer rows than the number it reported as
            // reachable. SQLite cannot check this — it is a cross-table aggregate.
            if catalogued < reachable.len() {
                return Err(corrupt("a measurement reachable in more tools than it catalogued"));
            }
            let auto_approve = auto_approve.ok_or_else(|| corrupt("a measured approval state"))?;
            Ok(ConfinementAttempt::measured(
                bound,
                binding,
                at,
                Measurement::from_stored(catalogued, reachable, auto_approve != 0, drift),
            ))
        }
    }
}

/// Insert one complete attempt into an existing transaction.
///
/// Ordinary ledger appends commit immediately. Certification keeps this transaction open and
/// advances `member_confined` beside the same factual row, eliminating the crash window between
/// evidence and progress.
fn insert_attempt(
    tx: &rusqlite::Transaction<'_>,
    slug: &Slug,
    attempt: &attempt::ConfinementAttempt,
) -> Result<i64, LifecycleError> {
    let variant = attempt.variant();
    let subject = attempt.authenticated();
    let policy = attempt.policy();
    let measurement = attempt.measurement();
    let blocked = match attempt.reached() {
        attempt::Reached::BindingBlocked { reason } => Some(*reason),
        attempt::Reached::Bound {
            catalogue: attempt::CatalogueResult::Blocked { reason }, ..
        } => Some(*reason),
        _ => None,
    };

    tx.execute(
        "INSERT INTO confinement_attempts(
             slug, variant, expected, at,
             subject_tenant, subject_user,
             service, service_version, policy_fingerprint,
             catalogued, auto_approve, blocked_kind)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            slug.as_str(),
            variant.tag(),
            attempt.expected().as_str(),
            attempt.at().as_str(),
            subject.map(|s| s.tenant().as_str()),
            subject.map(|s| s.user().as_str()),
            policy.map(|p| p.service().as_str()),
            policy.map(|p| i64::from(p.version())),
            policy.map(|p| p.fingerprint().as_str()),
            measurement.map(|m| m.catalogued() as i64),
            measurement.map(|m| i64::from(m.auto_approve())),
            blocked.map(attempt::Blocked::tag),
        ],
    )?;
    let id = tx.last_insert_rowid();
    if let Some(measurement) = measurement {
        for tool in measurement.reachable() {
            tx.execute(
                "INSERT INTO confinement_reachable(attempt_id, tool) VALUES(?1,?2)",
                params![id, tool],
            )?;
        }
        for (ordinal, drift) in measurement.drift().iter().enumerate() {
            let row = drift_columns(drift);
            tx.execute(
                "INSERT INTO confinement_drift(attempt_id, ordinal, kind, tool, state,
                                               observed, expected)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    ordinal as i64,
                    row.kind,
                    row.tool,
                    row.state,
                    row.observed,
                    row.expected
                ],
            )?;
        }
    }
    Ok(id)
}

/// Durable lifecycle state.
pub struct LifecycleStore {
    conn: Mutex<Connection>,
    writable: bool,
    /// Which schema this handle is reading. A read-only handle may legitimately be on v1.
    version: i64,
}

impl std::fmt::Debug for LifecycleStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleStore").field("writable", &self.writable).finish_non_exhaustive()
    }
}

/// A store that cannot be read is either one that is not there and could not be made, or one
/// whose bytes are not a database. The second needs a different sentence: that file may hold
/// state nothing else has, so the operator instruction is to preserve it rather than clear it
/// and retry. Collapsing the two is how a recoverable store gets deleted by someone following
/// the obvious remedy for "could not be opened".
fn opened(path: &Path, source: rusqlite::Error) -> LifecycleError {
    if matches!(&source, rusqlite::Error::SqliteFailure(e, _)
        if e.code == rusqlite::ErrorCode::NotADatabase)
    {
        return LifecycleError::NotADatabase { path: path.display().to_string() };
    }
    LifecycleError::Open { path: path.display().to_string(), source }
}

impl LifecycleStore {
    /// Open for writing, initialising or migrating as the file requires.
    ///
    /// # The ordering is the correctness property
    ///
    /// Discovery is read-only and comes **first**. `PRAGMA journal_mode = WAL` rewrites the header
    /// and creates `-wal`/`-shm` sidecars, so running it before the version check would mutate a
    /// store this command is about to refuse. An unknown version is therefore rejected with the
    /// file untouched.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .map_err(|source| LifecycleError::Open { path: path.display().to_string(), source })?;

        // ── read-only discovery, before anything that writes ────────────────────────
        // The decision is taken here, exhaustively, so the refusals happen while the file is still
        // untouched and the step below has no arm that could be reached by accident.
        let prepare = match discover(path, &conn)? {
            Shape::Unknown(found) => {
                return Err(LifecycleError::Version { found, expected: SCHEMA_VERSION });
            }
            Shape::Foreign => {
                return Err(LifecycleError::Uninitialized { path: path.display().to_string() });
            }
            Shape::Empty => Prepare::Initialise,
            Shape::V1 => Prepare::MigrateV1,
            Shape::V2 => Prepare::MigrateV2,
            Shape::V3 => Prepare::Validate,
        };

        // ── only now may the file change ────────────────────────────────────────────
        // Before the schema, so a file that is both readable and non-empty never exists.
        ironworks_domain::restrict_to_owner(path).map_err(|source| LifecycleError::Exposed {
            path: path.display().to_string(),
            source,
        })?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| opened(path, e))?;
        // This file names every tenant that exists and is the only record of what a half-finished
        // provisioning run left behind. It has to be true after a power cut, not after a clean
        // exit.
        conn.pragma_update(None, "synchronous", "FULL")?;
        // The ledger declares foreign keys; SQLite ignores them unless this is on. It is a no-op
        // for the v1 tables, which declare none.
        conn.pragma_update(None, "foreign_keys", "ON")?;

        match prepare {
            Prepare::Initialise => init_current(&conn)?,
            Prepare::MigrateV1 => migrate_v1_to_v3(&conn)?,
            Prepare::MigrateV2 => migrate_v2_to_v3(&conn)?,
            Prepare::Validate => validate_v3(&conn)?,
        }
        Ok(Self { conn: Mutex::new(conn), writable: true, version: SCHEMA_VERSION })
    }

    /// Open for inspection. Every write path refuses, so a reporting command cannot mutate the
    /// state it reports on.
    ///
    /// **Nothing here writes**: no pragma, no WAL, no metadata row, no sidecar file. That is what
    /// lets `lifecycle confine --dry-run` keep its "nothing written" contract literally, including
    /// not migrating a v1 store on its way past.
    ///
    /// A v1 store is accepted. It holds valid lifecycle state and simply predates the ledger, which
    /// [`Self::schema_version`] reports so a reader can say *predates the ledger* rather than
    /// *no observations*.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        let path = path.as_ref();
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| LifecycleError::Open { path: path.display().to_string(), source })?;
        let version = match discover(path, &conn)? {
            Shape::V1 => 1,
            Shape::V2 => {
                validate_v2(&conn)?;
                2
            }
            Shape::V3 => {
                validate_v3(&conn)?;
                3
            }
            Shape::Unknown(found) => {
                return Err(LifecycleError::Version { found, expected: SCHEMA_VERSION });
            }
            Shape::Empty | Shape::Foreign => {
                return Err(LifecycleError::Uninitialized { path: path.display().to_string() });
            }
        };
        Ok(Self { conn: Mutex::new(conn), writable: false, version })
    }

    /// Which schema this handle is reading. `1` means the confinement ledger is absent — a
    /// different fact from an empty one, and the two owe an operator different sentences.
    pub fn schema_version(&self) -> i64 {
        self.version
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    fn require_writable(&self) -> Result<(), LifecycleError> {
        if self.writable { Ok(()) } else { Err(LifecycleError::ReadOnly) }
    }

    /// Record one already-existing org scope and runtime member, atomically.
    ///
    /// This performs no foreign-system operation. Its caller must obtain [`AdoptionEvidence`] by
    /// observing the authorities first; either every local row commits or none does. A slug that
    /// already exists refuses even when every field agrees, because replacing its first provenance
    /// event with an upsert would make history operator-editable.
    pub fn adopt(
        &self,
        slug: &Slug,
        service: &ServiceId,
        channel: &ChannelRef,
        evidence: &AdoptionEvidence,
        now: &str,
    ) -> Result<InstanceSpec, LifecycleError> {
        self.require_writable()?;
        let conn = self.conn.lock().expect("lifecycle lock");
        let tx = conn.unchecked_transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT count(*) FROM instances WHERE slug=?1",
            params![slug.as_str()],
            |r| r.get(0),
        )?;
        if exists != 0 {
            return Err(LifecycleError::AlreadyExists { slug: slug.as_str().to_owned() });
        }

        tx.execute(
            "INSERT INTO instances(slug,org,service,channel,stage,started_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![
                slug.as_str(),
                evidence.org.as_str(),
                service.as_str(),
                channel.as_str(),
                Stage::MemberMinted.tag(),
                now,
            ],
        )?;
        for (stage, adopted) in [
            (Stage::PreflightPassed, false),
            (Stage::OrgRegistered, true),
            (Stage::DataInitializationSettled, true),
            (Stage::MemberMinted, true),
        ] {
            tx.execute(
                "INSERT INTO stage_history(slug,stage,at,adopted,created_id)
                 VALUES(?1,?2,?3,?4,NULL)",
                params![slug.as_str(), stage.tag(), now, i64::from(adopted)],
            )?;
        }
        tx.execute(
            "INSERT INTO authority_identities(
                 slug,authority,opaque_id,provenance,observed_at,subject_tenant,subject_user
             ) VALUES(?1,?2,?3,'adopted',?4,NULL,NULL)",
            params![slug.as_str(), Authority::OrgRegistration.tag(), evidence.org.as_str(), now,],
        )?;
        tx.execute(
            "INSERT INTO authority_identities(
                 slug,authority,opaque_id,provenance,observed_at,subject_tenant,subject_user
             ) VALUES(?1,?2,?3,'adopted',?4,?5,?3)",
            params![
                slug.as_str(),
                Authority::Member.tag(),
                evidence.subject.user().as_str(),
                now,
                evidence.subject.tenant().as_str(),
            ],
        )?;
        tx.commit()?;

        Ok(InstanceSpec {
            slug: slug.clone(),
            org: evidence.org.clone(),
            service: service.clone(),
            channel: channel.clone(),
        })
    }

    /// Record what an operator asked for. Idempotent: a repeat keeps the existing progress.
    pub fn declare(&self, spec: &InstanceSpec, now: &str) -> Result<(), LifecycleError> {
        self.require_writable()?;
        let conn = self.conn.lock().expect("lifecycle lock");
        conn.execute(
            "INSERT INTO instances(slug,org,service,channel,started_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?5)
             ON CONFLICT(slug) DO UPDATE SET org=excluded.org, service=excluded.service,
                                             channel=excluded.channel, updated_at=excluded.updated_at",
            params![
                spec.slug.as_str(),
                spec.org.as_str(),
                spec.service.as_str(),
                spec.channel.as_str(),
                now
            ],
        )?;
        Ok(())
    }

    /// Record that a stage was reached.
    ///
    /// `adopted` says whether the work was *done* or the authority was *found already there*.
    /// Both advance the stage; only one of them created anything, and an operator asking months
    /// later cannot reconstruct which.
    ///
    /// `created` is the identifier the foreign system gave whatever this stage brought into
    /// existence, when it brought something into existence. It is what lets custody distinguish
    /// *the member we minted* from *a member that happens to exist* — the difference between a
    /// clean instance and one carrying authority nobody accounted for.
    pub fn reach(
        &self,
        slug: &Slug,
        stage: Stage,
        adopted: bool,
        created: Option<&OpaqueId>,
        now: &str,
    ) -> Result<(), LifecycleError> {
        self.require_writable()?;
        if stage == Stage::MemberConfined {
            return Err(LifecycleError::ConfinementEvidenceRequired {
                slug: slug.as_str().to_owned(),
            });
        }
        let progress = self.progress(slug)?;
        if let Some(recorded) = progress.reached()
            && recorded >= stage
        {
            return Err(LifecycleError::StageWouldRegress {
                slug: slug.as_str().to_owned(),
                recorded: recorded.tag().to_owned(),
                attempted: stage.tag().to_owned(),
            });
        }
        // A stage may only be reached from its immediate predecessor. Ordering is the whole
        // contract — `member_confined` sitting between `member_minted` and `staged` is what stops
        // a member with an unbounded tool surface becoming routable — and a gap in the sequence
        // is a claim that work happened which nothing recorded.
        if let Some(previous) = Stage::ALL.iter().copied().rev().find(|s| *s < stage)
            && !progress.has_reached(previous)
        {
            return Err(LifecycleError::StageSkipped {
                slug: slug.as_str().to_owned(),
                attempted: stage.tag().to_owned(),
                predecessor: previous.tag().to_owned(),
            });
        }
        let conn = self.conn.lock().expect("lifecycle lock");
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE instances SET stage=?2, updated_at=?3 WHERE slug=?1",
            params![slug.as_str(), stage.tag(), now],
        )?;
        if changed == 0 {
            return Err(LifecycleError::NoSuchInstance { slug: slug.as_str().to_owned() });
        }
        tx.execute(
            "INSERT INTO stage_history(slug,stage,at,adopted,created_id) VALUES(?1,?2,?3,?4,?5)",
            params![
                slug.as_str(),
                stage.tag(),
                now,
                i64::from(adopted),
                created.map(OpaqueId::as_str)
            ],
        )?;
        if let Some(id) = created
            && let Some(authority) =
                Authority::ALL.iter().copied().find(|a| a.created_at() == stage)
        {
            let provenance =
                if adopted { IdentityProvenance::Adopted } else { IdentityProvenance::Created };
            tx.execute(
                "INSERT INTO authority_identities(
                     slug,authority,opaque_id,provenance,observed_at,subject_tenant,subject_user
                 ) VALUES(?1,?2,?3,?4,?5,NULL,NULL)",
                params![slug.as_str(), authority.tag(), id.as_str(), provenance.tag(), now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Append a qualifying confinement observation and advance `member_confined`, atomically.
    ///
    /// The first certification advances the stage exactly once. A later clean measurement is
    /// still useful evidence and is appended, but it does not rewrite stage history. Conversely,
    /// later drift is recorded through [`Self::record_attempt`] and does not erase the historical
    /// fact that this member was once certified; inspection reports history and current state as
    /// separate facts.
    pub fn certify_confinement(
        &self,
        slug: &Slug,
        evidence: &ConfinementEvidence,
    ) -> Result<i64, LifecycleError> {
        self.require_writable()?;
        let attempt = evidence.attempt();
        let policy = attempt
            .policy()
            .expect("ConfinementEvidence always contains a policy-bound measurement");
        let conn = self.conn.lock().expect("lifecycle lock");
        let tx = conn.unchecked_transaction()?;

        let instance: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT service,stage FROM instances WHERE slug=?1",
                params![slug.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((service, stage_tag)) = instance else {
            return Err(LifecycleError::NoSuchInstance { slug: slug.as_str().to_owned() });
        };
        if service != policy.service().as_str() {
            return Err(LifecycleError::ConfinementServiceMismatch {
                slug: slug.as_str().to_owned(),
                recorded: service,
                observed: policy.service().as_str().to_owned(),
            });
        }
        let reached = stage_tag
            .as_deref()
            .map(|tag| Stage::from_tag(tag).ok_or(LifecycleError::Corrupt { what: "stage" }))
            .transpose()?;
        if !reached.is_some_and(|stage| stage >= Stage::MemberMinted) {
            return Err(LifecycleError::StageSkipped {
                slug: slug.as_str().to_owned(),
                attempted: Stage::MemberConfined.tag().to_owned(),
                predecessor: Stage::MemberMinted.tag().to_owned(),
            });
        }

        let recorded_member: Option<String> = tx
            .query_row(
                "SELECT opaque_id FROM authority_identities
                 WHERE slug=?1 AND authority=?2",
                params![slug.as_str(), Authority::Member.tag()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(recorded_member) = recorded_member else {
            return Err(LifecycleError::ConfinementMemberMissing {
                slug: slug.as_str().to_owned(),
            });
        };
        if recorded_member != attempt.expected().as_str() {
            return Err(LifecycleError::ConfinementMemberMismatch {
                slug: slug.as_str().to_owned(),
                recorded: recorded_member,
                observed: attempt.expected().as_str().to_owned(),
            });
        }

        let id = insert_attempt(&tx, slug, attempt)?;
        if reached == Some(Stage::MemberMinted) {
            let at = attempt.at().as_str();
            let fingerprint = policy.fingerprint().as_str();
            tx.execute(
                "UPDATE instances SET stage=?2, updated_at=?3 WHERE slug=?1",
                params![slug.as_str(), Stage::MemberConfined.tag(), at],
            )?;
            tx.execute(
                "INSERT INTO stage_history(slug,stage,at,adopted,created_id)
                 VALUES(?1,?2,?3,0,?4)",
                params![slug.as_str(), Stage::MemberConfined.tag(), at, fingerprint],
            )?;
            tx.execute(
                "INSERT INTO authority_identities(
                     slug,authority,opaque_id,provenance,observed_at,subject_tenant,subject_user
                 ) VALUES(?1,?2,?3,'created',?4,NULL,NULL)",
                params![slug.as_str(), Authority::MemberConfinement.tag(), fingerprint, at],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Make an instance routable.
    ///
    /// Takes [`SmokeEvidence`], which is constructible only from a passing outcome — so this
    /// method has no argument that could record an activation nothing measured. Refuses unless
    /// every stage has been reached.
    pub fn activate(
        &self,
        slug: &Slug,
        smoke: &SmokeEvidence,
        now: &str,
    ) -> Result<(), LifecycleError> {
        self.require_writable()?;
        let progress = self.progress(slug)?;
        let last = *Stage::ALL.last().expect("the stage list is never empty");
        if !progress.has_reached(last) {
            return Err(LifecycleError::NotReadyToActivate {
                slug: slug.as_str().to_owned(),
                stage: progress.reached().map_or("none".to_owned(), |s| s.tag().to_owned()),
                last: last.tag().to_owned(),
            });
        }
        let conn = self.conn.lock().expect("lifecycle lock");
        conn.execute(
            "UPDATE instances SET smoke_isolation=?2, smoke_reachability=?3, smoke_at=?4,
                                  updated_at=?5
             WHERE slug=?1",
            params![slug.as_str(), smoke.isolation(), smoke.reachability(), smoke.at(), now],
        )?;
        Ok(())
    }

    /// Record what a serving process reports. A separate axis; never derived from `stage`.
    pub fn observe_serving(
        &self,
        slug: &Slug,
        observation: &ServingObservation,
    ) -> Result<(), LifecycleError> {
        self.require_writable()?;
        let at = match observation {
            ServingObservation::Serving { observed_at }
            | ServingObservation::NotServing { observed_at } => Some(observed_at.clone()),
            ServingObservation::Unknown => None,
        };
        let conn = self.conn.lock().expect("lifecycle lock");
        conn.execute(
            "UPDATE instances SET serving=?2, serving_at=?3 WHERE slug=?1",
            params![slug.as_str(), observation.tag(), at],
        )?;
        Ok(())
    }

    pub fn spec(&self, slug: &Slug) -> Result<Option<InstanceSpec>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        conn.query_row(
            "SELECT org,service,channel FROM instances WHERE slug=?1",
            params![slug.as_str()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .optional()?
        .map(|(org, service, channel)| {
            Ok(InstanceSpec {
                slug: slug.clone(),
                org: ironworks_domain::OrgId::parse(org)
                    .map_err(|_| LifecycleError::Corrupt { what: "org id" })?,
                service: ironworks_domain::ServiceId::parse(service)
                    .map_err(|_| LifecycleError::Corrupt { what: "service id" })?,
                channel: ironworks_domain::ChannelRef::new(channel),
            })
        })
        .transpose()
    }

    /// How far provisioning reached. `NotStarted` for an instance that was never declared — which
    /// says nothing whatever about what exists.
    pub fn progress(&self, slug: &Slug) -> Result<ProvisionProgress, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let row: Option<StoredProgress> = conn
            .query_row(
                "SELECT stage, smoke_isolation, smoke_reachability, smoke_at
                 FROM instances WHERE slug=?1",
                params![slug.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((stage, isolation, reachability, at)) = row else {
            return Ok(ProvisionProgress::NotStarted);
        };
        if let (Some(isolation), Some(reachability), Some(at)) = (isolation, reachability, at) {
            let outcome = SmokeOutcome::Passed { isolation, reachability };
            let smoke = SmokeEvidence::from_outcome(&outcome, at)
                .ok_or(LifecycleError::Corrupt { what: "smoke evidence" })?;
            return Ok(ProvisionProgress::Activated { smoke });
        }
        match stage {
            None => Ok(ProvisionProgress::NotStarted),
            Some(tag) => Stage::from_tag(&tag)
                .map(|reached| ProvisionProgress::Provisioning { reached })
                .ok_or(LifecycleError::Corrupt { what: "stage" }),
        }
    }

    pub fn serving(&self, slug: &Slug) -> Result<ServingObservation, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT serving, serving_at FROM instances WHERE slug=?1",
                params![slug.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((tag, Some(at))) if tag == "serving" => {
                ServingObservation::Serving { observed_at: at }
            }
            Some((tag, Some(at))) if tag == "not_serving" => {
                ServingObservation::NotServing { observed_at: at }
            }
            // Anything else, including an absent instance: nothing has observed it.
            _ => ServingObservation::Unknown,
        })
    }

    /// Every stage reached, with whether it was done or adopted.
    pub fn history(&self, slug: &Slug) -> Result<Vec<(Stage, String, bool)>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let mut stmt = conn.prepare(
            "SELECT stage, at, adopted FROM stage_history WHERE slug=?1 ORDER BY at, rowid",
        )?;
        let rows = stmt.query_map(params![slug.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
        })?;
        rows.map(|row| {
            let (tag, at, adopted) = row?;
            Stage::from_tag(&tag)
                .map(|s| (s, at, adopted))
                .ok_or(LifecycleError::Corrupt { what: "stage" })
        })
        .collect()
    }

    /// What IronWorks recorded bringing into existence, by authority.
    ///
    /// Strictly created-only. Adopted identities are deliberately absent: compensation and any
    /// caller asking what IronWorks made must never acquire somebody else's prior authority.
    pub fn created(&self, slug: &Slug) -> Result<Recorded, LifecycleError> {
        self.read_identities(slug, Some(IdentityProvenance::Created))
    }

    /// Every authority identity lifecycle accounts for, preserving created/adopted provenance.
    pub fn recorded(&self, slug: &Slug) -> Result<Recorded, LifecycleError> {
        self.read_identities(slug, None)
    }

    fn read_identities(
        &self,
        slug: &Slug,
        only: Option<IdentityProvenance>,
    ) -> Result<Recorded, LifecycleError> {
        let progress = self.progress(slug)?;
        let conn = self.conn.lock().expect("lifecycle lock");
        let mut identities = std::collections::BTreeMap::new();
        if self.version >= 3 {
            let mut stmt = conn.prepare(
                "SELECT authority,opaque_id,provenance,subject_tenant,subject_user
                 FROM authority_identities WHERE slug=?1 ORDER BY authority",
            )?;
            let rows = stmt.query_map(params![slug.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?;
            for row in rows {
                let (authority, id, provenance, tenant, user) = row?;
                let authority = Authority::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.tag() == authority)
                    .ok_or(LifecycleError::Corrupt { what: "an authority identity" })?;
                let provenance = IdentityProvenance::from_tag(&provenance)
                    .ok_or(LifecycleError::Corrupt { what: "authority identity provenance" })?;
                if only.is_some_and(|wanted| wanted != provenance) {
                    continue;
                }
                let runtime_subject = match (tenant, user) {
                    (Some(tenant), Some(user)) => Some(RuntimeSubject::new(
                        RuntimeTenantId::parse(&tenant)
                            .map_err(|_| LifecycleError::Corrupt { what: "runtime tenant id" })?,
                        RuntimeUserId::parse(&user)
                            .map_err(|_| LifecycleError::Corrupt { what: "runtime user id" })?,
                    )),
                    (None, None) => None,
                    _ => return Err(LifecycleError::Corrupt { what: "a runtime subject" }),
                };
                let identity =
                    AuthorityIdentity { id: OpaqueId::new(id), provenance, runtime_subject };
                identities.insert(authority, identity);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT stage,created_id,adopted FROM stage_history
                 WHERE slug=?1 AND created_id IS NOT NULL ORDER BY at,rowid",
            )?;
            let rows = stmt.query_map(params![slug.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
            })?;
            for row in rows {
                let (tag, id, adopted) = row?;
                let stage =
                    Stage::from_tag(&tag).ok_or(LifecycleError::Corrupt { what: "stage" })?;
                let provenance =
                    if adopted { IdentityProvenance::Adopted } else { IdentityProvenance::Created };
                if only.is_some_and(|wanted| wanted != provenance) {
                    continue;
                }
                if let Some(authority) =
                    Authority::ALL.iter().copied().find(|a| a.created_at() == stage)
                {
                    identities.insert(
                        authority,
                        AuthorityIdentity {
                            id: OpaqueId::new(id),
                            provenance,
                            runtime_subject: None,
                        },
                    );
                }
            }
        }
        Ok(Recorded { progress, identities })
    }

    /// Every identifier recorded across **all** instances for one authority.
    ///
    /// The other half of fleet reconciliation: the instance cannot tell us whose a member is, so
    /// the only way to ask "does anything account for this?" is a set difference against
    /// everything IronWorks wrote down. Opaque identifiers only — this returns nothing a tenant
    /// could be identified by.
    pub fn all_recorded_ids(
        &self,
        authority: Authority,
    ) -> Result<std::collections::BTreeSet<OpaqueId>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let (sql, tag) = if self.version >= 3 {
            (
                "SELECT DISTINCT opaque_id FROM authority_identities WHERE authority=?1",
                authority.tag(),
            )
        } else {
            (
                "SELECT DISTINCT created_id FROM stage_history
                 WHERE stage=?1 AND created_id IS NOT NULL",
                authority.created_at().tag(),
            )
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![tag], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(OpaqueId::new(row?))).collect()
    }

    /// Every instance that is not yet routable. Nothing here expires on age.
    /// Append one confinement attempt. **The only writer, and it never updates or deletes.**
    ///
    /// The parent and all its children go in one transaction, so a reader can never see a
    /// measurement whose reachable set or drift list is half-written.
    ///
    /// It appends whatever happened — a blocked binding and a subject mismatch are recorded exactly
    /// as a measurement is. An operator asking months later *what did we try* deserves the attempts
    /// that failed, and a ledger holding only successes would make a member that has never once
    /// been bindable look identical to one nobody has run `confine` against.
    pub fn record_attempt(
        &self,
        slug: &Slug,
        attempt: &attempt::ConfinementAttempt,
    ) -> Result<i64, LifecycleError> {
        self.require_writable()?;
        let conn = self.conn.lock().expect("lifecycle lock");
        let tx = conn.unchecked_transaction()?;
        let id = insert_attempt(&tx, slug, attempt)?;
        tx.commit()?;
        Ok(id)
    }

    /// The latest attempt about the member that is **currently** recorded.
    ///
    /// # Why it filters by member rather than taking the newest row
    ///
    /// A member can legitimately be replaced over time, and history for a previous member is
    /// history — not a contradiction, and not evidence about the member serving now. Filtering here
    /// is what stops an old member's clean measurement standing in as the current baseline, and
    /// equally what stops an old member's mismatch condemning the current one.
    pub fn latest_applicable(
        &self,
        slug: &Slug,
        member: &OpaqueId,
    ) -> Result<Option<attempt::ConfinementAttempt>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM confinement_attempts
                 WHERE slug=?1 AND expected=?2
                 ORDER BY at DESC, id DESC LIMIT 1",
                params![slug.as_str(), member.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            None => Ok(None),
            Some(id) => read_attempt(&conn, id).map(Some),
        }
    }

    /// Recent attempts for this instance, newest first, **across every member**.
    ///
    /// History is the one view that does not filter: an operator reading it should see that a
    /// previous member existed and what happened to it.
    ///
    /// Ordering is `(at DESC, id DESC)`. The tiebreak is load-bearing rather than defensive:
    /// `now_iso` has one-second granularity, and a single `confine` run appends its initial and
    /// final attempts inside the same second as a matter of course.
    pub fn confinement_history(
        &self,
        slug: &Slug,
        limit: usize,
    ) -> Result<Vec<attempt::ConfinementAttempt>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let mut stmt = conn.prepare(
            "SELECT id FROM confinement_attempts WHERE slug=?1 ORDER BY at DESC, id DESC LIMIT ?2",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![slug.as_str(), limit as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        ids.into_iter().map(|id| read_attempt(&conn, id)).collect()
    }

    pub fn unfinished(&self) -> Result<Vec<Slug>, LifecycleError> {
        let conn = self.conn.lock().expect("lifecycle lock");
        let mut stmt = conn.prepare(
            "SELECT slug FROM instances WHERE smoke_at IS NULL ORDER BY started_at, slug",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Slug::parse(row?).map_err(|_| LifecycleError::Corrupt { what: "slug" }))
            .collect()
    }
}

#[cfg(test)]
mod ledger_tests;
#[cfg(test)]
mod tests;
