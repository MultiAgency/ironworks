//! Durable turn state: what survives process death around one IronClaw turn.
//!
//! # The question this answers
//!
//! Everything before this crate establishes *identity → records → turn → runtime invocation*.
//! This one answers the next boundary: **the runtime accepted a turn — what became durable, what
//! may safely be retried, and what may safely be delivered?**
//!
//! # Ambiguity is a state, not a missing boolean
//!
//! [`TurnOutcome`] never collapses "may have been accepted" into unsent or failed. Those are
//! three different facts with three different correct responses, and the cost asymmetry lives
//! here: treating a may-have-run turn as unsent bills a second execution and produces a
//! different answer chained on a pointer that may already have moved.
//!
//! # The distinction that is easy to misread
//!
//! > **Never submit a new logical turn. An idempotent replay of the same logical turn is
//! > permitted when byte identity is proven.**
//!
//! "Never regenerate" is ambiguous, because B3 recovery *does* re-POST. What it must never do is
//! create a **new logical turn**: a fresh key, or the same key with a different body, either of
//! which the runtime treats as a new execution. Re-sending the *same* key with the *same* bytes
//! is retrieval wearing the shape of a request.
//!
//! # Where this deliberately improves on the reference
//!
//! ironworks treats crash boundary **B3** — the model completed but the response id was never
//! persisted — as unrecoverable, and blocks it for operator reconciliation. Measured against the
//! pinned runtime it is recoverable: replaying the same idempotency key with a **byte-identical**
//! body returns the original response id (21 ms against 984 ms, so it is served from the
//! idempotency index rather than re-executed). See ADR 0005.
//!
//! That is why [`TurnRecord::request_digest`] exists. A rebuild that does not match byte for byte
//! may not be replayed, and *that* — not the crash itself — is the blocked case.
//!
//! # What it holds, and what it must never hold
//!
//! Identifiers only: an idempotency key, an instance slug, an org, a timestamp, digests and
//! response ids. **No message text, no answer text, no records, no credentials, no headers.** A
//! journal exists to outlive a crash, and a credential in it would be a second copy of every
//! tenant's authority in the one file guaranteed to survive.
//!
//! # Where the boundary with the channel falls
//!
//! Delivery evidence lives here, on the turn, because an answer belongs to one conversation:
//! [`TurnRecord::delivery`] is what is known about the outbound message. Acknowledgement does
//! **not** live here. Whether the channel has been told to stop redelivering a question is a
//! shared, cross-tenant fact, and it belongs to `ironworks-channel`.
//!
//! The two meet through exactly one method — [`TurnProgress::openness`], three values — so the
//! coordinator that owns the shared cursor can order work without acquiring any tenant's
//! business context. See [`ironworks_domain::progress`] for why that trait is shaped the way it is.
//!
//! Nothing here ages anything away. Compaction is the channel's, it is bounded below the
//! acknowledged offset, and it never touches an update that cost a model turn.

use ironworks_domain::{
    AttemptOutcome, ChannelRef, DeliveryState, IdempotencyKey, NonEmptyReason, Openness, OrgId,
    PriorThread, Repair, ReportedModel, ResponseId, Sha256Digest, Slug, SuppliedMark,
    ThreadPointer, ThreadScope, TurnProgress, Utc,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

mod digest;
pub use digest::RequestDigest;

/// The schema this implementation writes.
///
/// Version 7 has one explicit migration: it gains the nullable `reported_model` column and no row
/// is touched, so every turn it held reads [`ReportedModel::Absent`] — nothing was recorded, so
/// nothing is claimed, and no evidence is fabricated.
///
/// Only that one step back migrates. Version 6 is refused on purpose — the same one-step policy
/// applied a version later, not a new one: a v6 file also lacks the four confinement-attribution
/// columns, which this migration does not add. Every other old or future version is refused
/// because its semantics are not known to this build.
pub const SCHEMA_VERSION: i64 = 8;
const MIGRATABLE_SCHEMA_VERSION: i64 = 7;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS turns (
    key             TEXT PRIMARY KEY,
    instance        TEXT NOT NULL,
    org             TEXT NOT NULL,
    retrieved_at    TEXT NOT NULL,
    request_digest  TEXT NOT NULL,
    outcome         TEXT NOT NULL,
    response_id     TEXT,
    attempts        INTEGER NOT NULL DEFAULT 0,
    first_seen      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    -- Delivery evidence. Two columns rather than one because the chunk count is a measurement,
    -- not a state name: `uncertain` with three chunks confirmed and `uncertain` with none are
    -- the same state carrying different amounts of client-visible content.
    delivery        TEXT NOT NULL DEFAULT 'pending',
    delivery_chunks INTEGER NOT NULL DEFAULT 0,
    -- What this turn DECIDED, so a rebuild replays it instead of deciding again.
    --
    -- ADR 0005 recovers a turn by reconstructing its request body byte-identically. The body
    -- depends on two choices made before it was built — whether to chain, and whether to re-send
    -- the book — and neither was recorded, so recovery recomputed them and got a different answer
    -- for anything that had chained. Measured: a chained turn could never be rebuilt, and the
    -- refusal held the shared cursor
    -- (docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md).
    --
    -- NULL is a value here, not an absence. `prev_response` NULL means the turn was FRESH;
    -- `carried_records` NULL means the book was supplied in full. Both are what a fresh, fully
    -- supplied turn recorded before this column existed, which is why the old rows that recovered
    -- correctly are exactly the ones these defaults describe.
    prev_response   TEXT,
    carried_records INTEGER,
    -- Why automatic processing stopped. Present exactly when the outcome is
    -- `operator_required`; it is the operator-visible alarm, so nothing may age it away and a
    -- row that has lost it is refused rather than read as a blank.
    operator_reason TEXT,
    profile_sha256 TEXT,
    policy_sha256 TEXT,
    catalogue_sha256 TEXT,
    confinement_observed_at TEXT,
    -- What the runtime said produced the turn, written when it was accepted, because a later
    -- fetch answers with a placeholder instead (`ReportedModel`, §G-2). NULL is `Absent` — the
    -- response claimed nothing — and is also what every row written before this column existed
    -- says, which is the honest reading of those rows rather than a convenient one. The
    -- placeholder is stored as itself, so a reader can tell not-recoverable from not-claimed.
    reported_model TEXT
);
CREATE INDEX IF NOT EXISTS turns_outcome ON turns(outcome);
-- One thread per CONVERSATION, not per tenant.
--
-- Version 2 keyed this by instance alone, which was correct while one tenant meant one room and
-- silently wrong the moment it did not. Nothing read it yet — `cli` never chains, so every turn
-- is `Fresh` — so no conversation was ever crossed. But the row was already being written under
-- the wrong key, and the leak would have appeared later, attached to a feature nobody would think
-- to connect to it: with continuation on, a second room's turn would chain from the first room's
-- response, and IronClaw replays thread history.
--
-- `channel` is the conversation's address. A turn with no conversation — `ironworks turn`, a
-- one-shot answered on a terminal — writes no row at all, because a thread that can never be
-- continued is not a thread.
CREATE TABLE IF NOT EXISTS threads (
    instance     TEXT NOT NULL,
    channel      TEXT NOT NULL,
    org          TEXT NOT NULL,
    response_id  TEXT NOT NULL,
    -- What the thread was built on, and how long it has run. Both are inputs to the continuation
    -- decision, and both must be durable: a thread whose composition moved may not be continued,
    -- and neither may one long enough that its next turn is mostly its own history. Deciding
    -- either from memory would decide it wrongly after a restart, which is exactly when a
    -- conversation is most likely to be resumed.
    alloy        TEXT NOT NULL,
    turns        INTEGER NOT NULL DEFAULT 0,
    -- What this thread has been shown, as the model saw it. NULL means never shown, which is a
    -- different fact from shown-an-empty-book: an empty book still has a fingerprint. Both are
    -- reset when the composition moves, because the chain that carried them is gone.
    supplied_catalog TEXT,
    supplied_records INTEGER,
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (instance, channel)
);
";

/// What `ironworks-store` needs to know about this file: its schema, where the version is
/// recorded, and which version this implementation writes.
const SCHEMA: ironworks_store::Schema =
    ironworks_store::Schema { sql: SCHEMA_SQL, meta_table: "meta", version: SCHEMA_VERSION };

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("the journal snapshot at {path} could not be created: {source}")]
    Snapshot {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the journal at {path} could not be opened: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "the journal at {path} is not a readable database. Refusing to run — and do NOT delete it \
         blind: it is the record of which turns ran and which answers reached a client, and a turn it still holds may have been billed. Move it aside and keep it."
    )]
    NotADatabase { path: String },
    #[error(
        "the journal at {path} was created but could not be restricted to its owner: {source}. \
         Refusing to run: the file names every organization served and every turn outstanding, \
         and continuing would leave that readable while reporting success."
    )]
    Exposed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the journal is schema version {found}, this implementation writes {expected}. Refusing \
         to run: an unknown version means a downgrade or an unfinished write, and guessing which \
         would risk a live conversation."
    )]
    Version { found: i64, expected: i64 },
    #[error(
        "the journal's recorded schema version is {raw:?}, which is not a number. Refusing to \
         read it as 'no version recorded': that arm stamps this implementation's version onto a \
         database whose shape was never established, and this file holds the record of which turns ran and which answers reached a client."
    )]
    UnreadableVersion { raw: String },
    #[error("no turn is journalled under this key")]
    NoSuchTurn,
    #[error(
        "a turn already holding response id {existing} cannot be re-recorded as {attempted} — a \
         known response is fetched, never regenerated"
    )]
    ResponseWouldChange { existing: String, attempted: String },
    #[error(
        "a thread pointer belonging to {pointer_instance} cannot be committed against instance \
         {instance} — a pointer carries its instance and they must agree"
    )]
    ForeignPointer { instance: String, pointer_instance: String },
    #[error("this journal is open read-only; inspection has no side effects")]
    ReadOnly,
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// What is durably known about one turn.
///
/// Five states, none of them a boolean. `SubmittedUnknown` is the one that matters: it is
/// neither "sent" nor "failed", and collapsing it into either is the defect this type exists to
/// make unrepresentable.
///
/// This is the *turn* axis only. What happened to the answer afterwards is
/// [`TurnRecord::delivery`], and the two are read together — never merged into one "done".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Recorded before dispatch, or a dispatch that provably never left. Safe to send.
    NotSubmitted,
    /// It may have executed.
    ///
    /// **Never submit a new logical turn.** An *idempotent replay* of the same logical turn — the
    /// same key with a byte-identical body — is permitted, and is how this state is resolved.
    /// Those are different acts: the first would bill a second model execution, the second
    /// retrieves the one that may already have run.
    SubmittedUnknown,
    /// The runtime named a response. Fetch it; never regenerate.
    Accepted { response: ResponseId },
    /// The turn completed, and the continuation pointer it produced became authoritative in the
    /// same transaction.
    Completed { response: ResponseId },
    /// **Automatic processing has deliberately stopped, and an operator is required.**
    ///
    /// Not "we gave up on it" — the name matters, because this is persisted vocabulary and an
    /// operator reading `abandoned` in a journal would reasonably conclude the work was
    /// discarded. Nothing here is discarded: the reason, the response id and the delivery
    /// evidence are all retained, and the row is exempt from every form of compaction.
    ///
    /// # Why a settled-but-unsuccessful state has to exist
    ///
    /// Without it, a turn that cannot proceed stays open forever — and because the channel cursor
    /// is the *lowest open update*, one unresolvable turn would hold the offset behind it and
    /// every conversation after it would go unanswered while the process looked healthy.
    /// Stopping automatic work on it is what stops one stuck conversation deafening the rest; the
    /// alarm lives on in [`Journal::unresolved`].
    OperatorRequired { response: Option<ResponseId>, reason: NonEmptyReason },
}

impl TurnOutcome {
    /// Stable spelling used on disk and in read-only operator output.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NotSubmitted => "not_submitted",
            Self::SubmittedUnknown => "submitted_unknown",
            Self::Accepted { .. } => "accepted",
            Self::Completed { .. } => "completed",
            Self::OperatorRequired { .. } => "operator_required",
        }
    }
    /// The response id, once one is durably known. After that the answer is fetched forever.
    pub fn response(&self) -> Option<&ResponseId> {
        match self {
            Self::NotSubmitted | Self::SubmittedUnknown => None,
            Self::Accepted { response } | Self::Completed { response } => Some(response),
            Self::OperatorRequired { response, .. } => response.as_ref(),
        }
    }
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// One journalled turn. Identifiers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRecord {
    pub key: IdempotencyKey,
    pub instance: Slug,
    pub org: OrgId,
    /// The one nondeterministic input to the request body. Stored so a rebuild reuses it rather
    /// than reading the clock again — which is what makes a replay byte-identical.
    pub retrieved_at: String,
    /// A fingerprint of the exact body that was, or would be, sent. Never the body.
    pub request_digest: RequestDigest,
    pub outcome: TurnOutcome,
    /// What is known about the *outbound* answer. A separate axis from `outcome`, which is about
    /// the model turn: a turn can be completed and undelivered, and the two must never be
    /// collapsed into one "done".
    pub delivery: DeliveryState,
    pub attempts: u32,
    pub first_seen: String,
    pub updated_at: String,
    /// What this turn chose before its body was built. Replayed on recovery, never re-decided.
    pub decision: Decision,
    /// Short-lived confinement evidence attached when this logical turn was first accepted for
    /// dispatch. `None` is an honest migrated or non-deployed turn, never implicit compliance.
    pub attribution: Option<CapabilityAttribution>,
    /// What the runtime reported running this turn, as it said so on acceptance. A migrated row
    /// reads [`ReportedModel::Absent`], which is true of it: nothing was recorded, so nothing is
    /// claimed.
    pub reported_model: ReportedModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAttribution {
    pub profile_sha256: Sha256Digest,
    pub policy_sha256: Sha256Digest,
    pub catalogue_sha256: Sha256Digest,
    pub observed_at: Utc,
}

/// Every value that distinguishes an attributed new-turn admission.
pub struct AttributedAdmission<'a> {
    pub retrieved_at: &'a str,
    pub digest: &'a RequestDigest,
    pub decision: &'a Decision,
    pub attribution: &'a CapabilityAttribution,
    pub now: &'a Utc,
}

struct AdmissionEvidence<'a> {
    decision: &'a Decision,
    attribution: Option<&'a CapabilityAttribution>,
    now: &'a Utc,
}

impl TurnRecord {
    /// Whether this turn is `scope`'s to act on.
    ///
    /// # Why a row found by key is not necessarily yours
    ///
    /// An idempotency key is minted from a channel update id — `upd-{id}` — and **names no
    /// tenant**, while one journal file holds every tenant's turns: `relay` opens the same
    /// `--journal` for each entry in the registry. So [`Journal::turn`], which looks a row up by
    /// key alone, can hand back a row belonging to somebody else. A caller that acts on one
    /// fetches another tenant's response under this tenant's credential, and commits a thread
    /// pointing at it.
    ///
    /// [`ironworks_domain::TurnError::ForeignThread`] does not cover this. That check lives inside
    /// `build_turn`, and a recovery holding a response id returns *before* a turn is built — so
    /// the one path that can reach a foreign response is the one path the pointer check never
    /// sees. This is the check for it.
    ///
    /// The reachable case is an operator re-pointing a channel at another tenant while an update
    /// for the first is still in flight. Routing already refuses a channel that maps to *nobody*;
    /// it does not compare the tenant it now maps to against the turn already recorded.
    ///
    /// # Why both axes
    ///
    /// The slug is what routing produces; the org is what actually scoped the records the turn
    /// was built from. An operator editing the registry moves the first, and a credential
    /// re-pointed at another organization moves the second. Comparing one would let the other
    /// through.
    pub fn belongs_to(&self, scope: &ThreadScope) -> bool {
        self.instance == *scope.slug() && self.org == *scope.org()
    }
}

/// The two choices a turn made before its body was built.
///
/// Journalled at [`Journal::begin`] so recovery **replays** them rather than deciding again. The
/// distinction matters: `cli` used to reconstruct a recovered turn with no previous pointer and
/// a full book, which is not "not deciding" — it is deciding `Fresh` a second time, and it is only
/// right for a turn that was fresh to begin with.
///
/// The chained-from response is stored, never the pointer: a [`ThreadPointer`] is reconstructed
/// through the caller's own [`ThreadScope`], so a pointer belonging to another tenant stays
/// unrepresentable rather than merely unwritten.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decision {
    /// The response this turn chained from. `None` is a fresh turn.
    pub previous: Option<ResponseId>,
    /// How many records the conversation was already carrying. `None` means the book was supplied
    /// in this turn's own envelope.
    pub carried: Option<u32>,
}

impl TurnRecord {
    /// Whether a rebuilt body may be replayed under this key.
    ///
    /// The runtime refuses a changed body under an existing key, so a mismatch is not merely
    /// unwise — it cannot work. Checking here turns a remote `409` into a local decision.
    pub fn may_replay(&self, rebuilt: &RequestDigest) -> bool {
        &self.request_digest == rebuilt
    }

    /// Whether this turn still has work a process may do unattended.
    ///
    /// This is the whole of what a channel coordinator is allowed to learn about a tenant's work
    /// — see [`ironworks_domain::progress`]. Open means: automatic processing has not stopped, and
    /// the answer has not yet been attempted. Everything else is settled, *including* a delivery parked for
    /// reconciliation, because an update nobody will act on must not hold the shared cursor.
    pub fn openness(&self) -> Openness {
        match self.outcome {
            TurnOutcome::OperatorRequired { .. } => Openness::Settled,
            _ if self.delivery.awaits_automatic_work() => Openness::Open,
            _ => Openness::Settled,
        }
    }
}

/// What to do with a journalled turn after a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// Nothing was sent. Dispatch, reusing the same key and timestamp. **B1, B2-unsent.**
    Dispatch { key: IdempotencyKey, retrieved_at: String },
    /// A turn may have run. Rebuild the body, prove it matches `expect`, replay the same key.
    /// **B3** — which the reference implementation treats as unrecoverable.
    ReplayIdentical { key: IdempotencyKey, retrieved_at: String, expect: RequestDigest },
    /// The response is known. Fetch it; never regenerate. **B4.**
    Fetch { response: ResponseId },
    /// Complete and authoritative: the answer and its continuation pointer are both durable, and
    /// nothing has been sent yet. Fetch the stored answer and send it. **B5.**
    ///
    /// This is also where B5's *identity* requirement is met, and it is worth being precise about
    /// how. A crash between the send and its record produces a duplicate — that is unavoidable —
    /// and the duplicate must be the same answer, never a new one. The journal does not achieve
    /// that by holding the text (it holds no client content at all). It achieves it by the
    /// response id being durable: fetching a fixed id returns the answer that was already sent.
    Deliver { response: ResponseId },
    /// The answer went out, or may have. Nothing further happens automatically. **B7.**
    Settled { delivery: DeliveryState },
    /// A send was attempted and not confirmed. An operator decides, and
    /// [`Repair::requires_operator`] on the repair says whether saying yes duplicates content a
    /// client can already read.
    ///
    /// Named for what is unresolved rather than for who must act, because the variant below also
    /// needs an operator and the two would otherwise read as synonyms.
    ReconcileDelivery { delivery: DeliveryState, repair: Repair, response: Option<ResponseId> },
    /// Automatic processing stopped. Carries the reason, so the alarm is legible without a
    /// second lookup.
    OperatorRequired { reason: NonEmptyReason, response: Option<ResponseId> },
}

/// The recovery decision. Pure, so every boundary is exercisable without a database.
///
/// **Delivery evidence is consulted before the turn outcome, always.** A turn whose answer has
/// reached a client — or may have — must never be handed back to the turn state machine, whatever
/// its outcome column says. Reading the axes in the other order is how a delivered answer gets
/// re-driven as an incomplete turn.
pub fn recover(record: &TurnRecord) -> Recovery {
    // One match on the pair, in precedence order, and **that is what removes the
    // `unreachable!`**. This was three passes — an `if let` for the alarm, a `match` on delivery,
    // then a `match` on the outcome — and the third could not name `OperatorRequired` as anything
    // but impossible, asserting in prose ("decided first") what the first pass had done twelve
    // lines above. Matching `(outcome, delivery)` lets the compiler see the same thing: the
    // wildcard in the first arm covers `(OperatorRequired, Pending)`, so no arm is left over.
    //
    // The order is the precedence and is load-bearing in both places it matters. An **alarm
    // outranks delivery evidence**: a turn an operator has to look at is not settled by having
    // been delivered. And `Delivered` outranks the outcome: delivery is terminal, so a completed
    // turn that reached a client is settled rather than re-driven.
    match (&record.outcome, record.delivery) {
        (TurnOutcome::OperatorRequired { reason, response }, _) => {
            Recovery::OperatorRequired { reason: reason.clone(), response: response.clone() }
        }
        (_, DeliveryState::Delivered) => Recovery::Settled { delivery: record.delivery },
        (_, DeliveryState::KnownUnsent | DeliveryState::Uncertain { .. }) => {
            Recovery::ReconcileDelivery {
                delivery: record.delivery,
                repair: record.delivery.repair(),
                response: record.outcome.response().cloned(),
            }
        }
        (TurnOutcome::NotSubmitted, DeliveryState::Pending) => Recovery::Dispatch {
            key: record.key.clone(),
            retrieved_at: record.retrieved_at.clone(),
        },
        (TurnOutcome::SubmittedUnknown, DeliveryState::Pending) => Recovery::ReplayIdentical {
            key: record.key.clone(),
            retrieved_at: record.retrieved_at.clone(),
            expect: record.request_digest.clone(),
        },
        (TurnOutcome::Accepted { response }, DeliveryState::Pending) => {
            Recovery::Fetch { response: response.clone() }
        }
        (TurnOutcome::Completed { response }, DeliveryState::Pending) => {
            Recovery::Deliver { response: response.clone() }
        }
    }
}

/// A durable journal.
pub struct Journal {
    conn: Mutex<Connection>,
    writable: bool,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal").field("writable", &self.writable).finish_non_exhaustive()
    }
}

/// An open failure, as the sentence an operator has to act on.
///
/// A store that cannot be read is either one that is not there and could not be made, or one whose
/// bytes are not a database. The second needs a different sentence: that file may hold state
/// nothing else has, so the operator instruction is to preserve it rather than clear it and retry.
/// Collapsing the two is how a recoverable store gets deleted by someone following the obvious
/// remedy for "could not be opened".
///
/// `ironworks-store` runs the protocol and returns a cause carrying no path and no prose, because
/// the three crates that share it keep three different files and the wording is the part that
/// says which one this is. This is that translation, and it is the only thing about opening a
/// journal that is not shared.
fn opened(path: &Path, why: ironworks_store::OpenFailure) -> JournalError {
    use ironworks_store::OpenFailure as F;
    let path = path.display().to_string();
    match why {
        F::Open(source) => JournalError::Open { path, source },
        F::NotADatabase => JournalError::NotADatabase { path },
        F::Exposed(source) => JournalError::Exposed { path, source },
        F::UnreadableVersion { raw } => JournalError::UnreadableVersion { raw },
        F::Version { found } => JournalError::Version { found, expected: SCHEMA_VERSION },
        F::Sqlite(source) => JournalError::Sqlite(source),
    }
}

impl Journal {
    /// The connection, whatever a previous panic left behind.
    ///
    /// `lock().expect("journal lock")` opened 10 methods, and a poisoned mutex made every one of them
    /// abort — inside a store whose error enum exists to fail closed and whose whole subject is
    /// which turns ran and which answers reached a client. Poisoning means *some other* thread panicked while holding this; the
    /// data behind it is a SQLite connection, which has its own transactional integrity and is not
    /// left half-written by a panic in Rust code. Taking the guard is therefore the honest
    /// recovery, and it is one documented decision rather than 10 undocumented ones.
    fn locked(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Copy a transactionally consistent view into a new owner-only SQLite file.
    ///
    /// SQLite's online backup API, rather than copying the database and its WAL, makes this safe
    /// while a relay is writing. An existing destination is never replaced.
    pub fn snapshot_to(&self, destination: impl AsRef<Path>) -> Result<(), JournalError> {
        let destination = destination.as_ref();
        let source = self.locked();
        ironworks_store::snapshot_to(&source, destination).map_err(|why| match why {
            ironworks_store::SnapshotFailure::Reserve(source) => {
                JournalError::Snapshot { path: destination.display().to_string(), source }
            }
            ironworks_store::SnapshotFailure::Sqlite(source) => JournalError::Sqlite(source),
        })
    }

    /// Open for writing, creating the schema if absent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let conn = ironworks_store::open_durable(path, SCHEMA, |tx, found| {
            if found != MIGRATABLE_SCHEMA_VERSION {
                return Ok(false);
            }
            // Additive and touching no row, inside the transaction `open_durable` owns — the one
            // that also writes the new version. An interrupted migration therefore rolls back with
            // the version still 7, and the next open re-runs it.
            tx.execute_batch("ALTER TABLE turns ADD COLUMN reported_model TEXT;")?;
            Ok(true)
        })
        .map_err(|why| opened(path, why))?;
        Ok(Self { conn: Mutex::new(conn), writable: true })
    }

    /// Open for **inspection**: read-only at the SQLite level, so a reader cannot mutate a live
    /// journal even by mistake. Looking at production must not be able to change it.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let conn = ironworks_store::open_readonly(path, SCHEMA).map_err(|why| opened(path, why))?;
        Ok(Self { conn: Mutex::new(conn), writable: false })
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    fn require_writable(&self) -> Result<(), JournalError> {
        if self.writable { Ok(()) } else { Err(JournalError::ReadOnly) }
    }

    /// Record intent **before** dispatch.
    ///
    /// The key, the timestamp and the body digest become durable before a single byte leaves,
    /// because they are the only handles that exist to a turn we may never hear back about.
    /// Idempotent: a repeat keeps the original row rather than resetting its state.
    pub fn begin(
        &self,
        scope: &ThreadScope,
        key: &IdempotencyKey,
        retrieved_at: &str,
        digest: &RequestDigest,
        decision: &Decision,
        now: &Utc,
    ) -> Result<(), JournalError> {
        self.begin_inner(
            scope,
            key,
            retrieved_at,
            digest,
            AdmissionEvidence { decision, attribution: None, now },
        )
    }

    pub fn begin_attributed(
        &self,
        scope: &ThreadScope,
        key: &IdempotencyKey,
        admission: AttributedAdmission<'_>,
    ) -> Result<(), JournalError> {
        self.begin_inner(
            scope,
            key,
            admission.retrieved_at,
            admission.digest,
            AdmissionEvidence {
                decision: admission.decision,
                attribution: Some(admission.attribution),
                now: admission.now,
            },
        )
    }

    fn begin_inner(
        &self,
        scope: &ThreadScope,
        key: &IdempotencyKey,
        retrieved_at: &str,
        digest: &RequestDigest,
        evidence: AdmissionEvidence<'_>,
    ) -> Result<(), JournalError> {
        let decision = evidence.decision;
        let attribution = evidence.attribution;
        let now = evidence.now;
        self.require_writable()?;
        let conn = self.locked();
        conn.execute(
            "INSERT INTO turns(key,instance,org,retrieved_at,request_digest,outcome,attempts,
                               first_seen,updated_at,prev_response,carried_records,
                               profile_sha256,policy_sha256,catalogue_sha256,
                               confinement_observed_at)
             VALUES(?1,?2,?3,?4,?5,'not_submitted',0,?6,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(key) DO NOTHING",
            params![
                key.as_str(),
                scope.slug().as_str(),
                scope.org().as_str(),
                retrieved_at,
                digest.as_str(),
                now.as_str(),
                decision.previous.as_ref().map(ResponseId::as_str),
                decision.carried,
                attribution.map(|value| value.profile_sha256.as_str()),
                attribution.map(|value| value.policy_sha256.as_str()),
                attribution.map(|value| value.catalogue_sha256.as_str()),
                attribution.map(|value| value.observed_at.as_str()),
            ],
        )?;
        Ok(())
    }

    /// A dispatch attempt provably never left. Still safe to send.
    pub fn record_unsent(&self, key: &IdempotencyKey, now: &Utc) -> Result<(), JournalError> {
        self.set_outcome(key, &TurnOutcome::NotSubmitted, now)
    }

    /// A dispatch attempt may have executed.
    ///
    /// A one-way door: this is never walked back to `not_submitted`, because the evidence that
    /// produced it does not improve with time.
    pub fn record_unknown(&self, key: &IdempotencyKey, now: &Utc) -> Result<(), JournalError> {
        self.set_outcome(key, &TurnOutcome::SubmittedUnknown, now)
    }

    /// The runtime named a response.
    /// Accept a turn, recording what the runtime said produced it.
    ///
    /// The model is written **here** and nowhere else, because acceptance is the only moment it
    /// exists: `model` is echoed on create and comes back as a placeholder on every later fetch
    /// (§G-2), so a path that recorded it after the fact would record the placeholder. The two
    /// writes share one transaction for the reason `record_operator_required` gives — a row whose
    /// outcome moved without its evidence is a row that claims more than was measured.
    pub fn record_accepted(
        &self,
        key: &IdempotencyKey,
        response: &ResponseId,
        reported_model: &ReportedModel,
        now: &Utc,
    ) -> Result<(), JournalError> {
        self.require_writable()?;
        let outcome = TurnOutcome::Accepted { response: response.clone() };
        let conn = self.locked();
        let tx = conn.unchecked_transaction()?;
        Self::set_outcome_in(&tx, key, &outcome, now)?;
        tx.execute(
            "UPDATE turns SET reported_model=?2 WHERE key=?1",
            params![key.as_str(), reported_model.wire()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// **The critical transaction.** The response id and the continuation pointer it produced
    /// become durable together, or neither does.
    ///
    /// Two rows in two tables can otherwise disagree in exactly one direction that silently
    /// corrupts a live conversation: the turn says completed while the thread still points at
    /// the turn before it, so the next turn chains from a stale parent and the conversation
    /// loses a turn with nothing reporting it. Both rows are individually well-formed
    /// afterwards, which is what makes it undetectable.
    ///
    /// Taking a [`ThreadPointer`] rather than a bare id is the tenant binding: a pointer can
    /// only be minted from a [`ThreadScope`], and a scope only from a verified instance.
    pub fn commit_completed(
        &self,
        key: &IdempotencyKey,
        pointer: &ThreadPointer,
        conversation: Option<&ChannelRef>,
        alloy: &str,
        // What the thread now carries. `None` leaves the existing mark alone — a turn answered
        // with the store degraded supplied nothing, and must not erase what an earlier turn did
        // supply.
        supplied: Option<&SuppliedMark>,
        now: &Utc,
    ) -> Result<(), JournalError> {
        self.require_writable()?;
        let mut conn = self.locked();
        let existing: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT instance, response_id FROM turns WHERE key=?1",
                params![key.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((instance, prior)) = existing else { return Err(JournalError::NoSuchTurn) };
        if pointer.instance().as_str() != instance {
            return Err(JournalError::ForeignPointer {
                instance,
                pointer_instance: pointer.instance().as_str().to_owned(),
            });
        }
        if let Some(prior) = prior.filter(|p| p.as_str() != pointer.response().as_str()) {
            return Err(JournalError::ResponseWouldChange {
                existing: prior,
                attempted: pointer.response().as_str().to_owned(),
            });
        }
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE turns SET outcome='completed', response_id=?2, updated_at=?3 WHERE key=?1",
            params![key.as_str(), pointer.response().as_str(), now.as_str()],
        )?;
        // No conversation, no thread. A terminal one-shot has no room to continue in, and a row
        // it could never be asked for would be a pointer waiting to be handed to the wrong caller.
        if let Some(channel) = conversation {
            // `turns` counts up here and nowhere else, so the count is of turns that actually
            // completed. Counting at dispatch would count turns that failed, and the cost bound
            // would then break threads that never grew.
            tx.execute(
                "INSERT INTO threads(instance,channel,org,response_id,alloy,turns,
                                      supplied_catalog,supplied_records,updated_at)
                 VALUES(?1,?2,(SELECT org FROM turns WHERE key=?3),?4,?5,1,?6,?7,?8)
                 ON CONFLICT(instance,channel) DO UPDATE SET
                     response_id=excluded.response_id,
                     alloy=excluded.alloy,
                     -- A composition change starts the count again: the chain is broken, so the
                     -- turns before it are not this thread's history any more.
                     turns=CASE WHEN threads.alloy=excluded.alloy THEN threads.turns + 1 ELSE 1 END,
                     -- And it forgets what was supplied, for the same reason: the records the old
                     -- thread carried are not in the new one. Within one composition, a turn that
                     -- supplied nothing keeps what the thread already had.
                     supplied_catalog=CASE WHEN threads.alloy=excluded.alloy
                         THEN COALESCE(excluded.supplied_catalog, threads.supplied_catalog)
                         ELSE excluded.supplied_catalog END,
                     supplied_records=CASE WHEN threads.alloy=excluded.alloy
                         THEN COALESCE(excluded.supplied_records, threads.supplied_records)
                         ELSE excluded.supplied_records END,
                     updated_at=excluded.updated_at",
                params![
                    instance,
                    channel.as_str(),
                    key.as_str(),
                    pointer.response().as_str(),
                    alloy,
                    supplied.map(|m| m.catalog.as_str()),
                    supplied.map(|m| m.records),
                    now.as_str()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Takes the typed outcome, so [`TurnOutcome::tag`] is the one place the on-disk spelling is
    /// decided and a new variant cannot be persisted under an invented name.
    fn set_outcome(
        &self,
        key: &IdempotencyKey,
        outcome: &TurnOutcome,
        now: &Utc,
    ) -> Result<(), JournalError> {
        self.require_writable()?;
        let conn = self.locked();
        Self::set_outcome_in(&conn, key, outcome, now)
    }

    /// [`Self::set_outcome`] against a connection the caller already holds.
    ///
    /// Split out so a caller that must write the outcome *and something else* atomically can put
    /// both inside one transaction. `record_operator_required` is that caller, and the two writes
    /// it makes are the pair whose gap left an unreadable row.
    ///
    /// The writability check stays on the `&self` wrapper: this takes a live connection, and every
    /// caller reaches it through one.
    fn set_outcome_in(
        conn: &Connection,
        key: &IdempotencyKey,
        outcome: &TurnOutcome,
        now: &Utc,
    ) -> Result<(), JournalError> {
        let (tag, response) = (outcome.tag(), outcome.response());
        let prior: Option<Option<String>> = conn
            .query_row("SELECT response_id FROM turns WHERE key=?1", params![key.as_str()], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(prior) = prior else { return Err(JournalError::NoSuchTurn) };
        // A known response id is never unlearned. Once the runtime has named one, the answer is
        // fetched forever after — regressing to an earlier state would license a second billed
        // turn, which is the whole failure this layer exists to prevent.
        if let Some(existing) = prior {
            match response {
                Some(r) if r.as_str() == existing => {}
                Some(r) => {
                    return Err(JournalError::ResponseWouldChange {
                        existing,
                        attempted: r.as_str().to_owned(),
                    });
                }
                None => {
                    return Err(JournalError::ResponseWouldChange {
                        existing,
                        attempted: format!("<{tag}>"),
                    });
                }
            }
        }
        conn.execute(
            "UPDATE turns SET outcome=?2, response_id=COALESCE(?3,response_id),
                              attempts=attempts+1, updated_at=?4
             WHERE key=?1",
            params![key.as_str(), tag, response.map(ResponseId::as_str), now.as_str()],
        )?;
        Ok(())
    }

    /// One journalled turn.
    pub fn turn(&self, key: &IdempotencyKey) -> Result<Option<TurnRecord>, JournalError> {
        let conn = self.locked();
        conn.query_row(
            &format!("SELECT {TURN_COLUMNS} FROM turns WHERE key=?1"),
            params![key.as_str()],
            row_to_record,
        )
        .optional()
        .map_err(Into::into)
    }

    /// The continuation pointer for one instance, if a turn has ever completed for it.
    ///
    /// Requires a [`ThreadScope`], so a pointer is only ever handed to something that already
    /// holds a verified instance — the same rule on the way out as on the way in.
    /// The conversation must be named. There is deliberately no way to ask for "this tenant's
    /// thread": a tenant with two rooms has two, and a caller that could omit the room would get
    /// whichever one was written last.
    pub fn prior_thread(
        &self,
        scope: &ThreadScope,
        conversation: &ChannelRef,
    ) -> Result<Option<PriorThread>, JournalError> {
        let conn = self.locked();
        /// One `threads` row as SQLite hands it back, before it becomes a `PriorThread`.
        type Row = (String, String, i64, Option<String>, Option<i64>);
        let found: Option<Row> = conn
            .query_row(
                "SELECT response_id, alloy, turns, supplied_catalog, supplied_records
                 FROM threads WHERE instance=?1 AND channel=?2 AND org=?3",
                params![scope.slug().as_str(), conversation.as_str(), scope.org().as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        // A stored id that no longer parses is a journal we do not understand — refuse rather
        // than hand back a pointer nobody can account for.
        found
            .map(|(response, alloy, turns, catalog, records)| {
                ResponseId::parse(response).map(|id| PriorThread {
                    pointer: scope.pointer(id),
                    alloy,
                    turns: turns.max(0) as u32,
                    // Both columns or neither. A half-written mark is a mark nobody can act on,
                    // so it reads as never supplied rather than as a guess.
                    supplied: catalog.zip(records).map(|(catalog, records)| SuppliedMark {
                        catalog,
                        records: records.max(0) as u32,
                    }),
                })
            })
            .transpose()
            .map_err(|_| JournalError::Sqlite(rusqlite::Error::InvalidQuery))
    }

    /// Fold one send attempt's outcome into the durable delivery evidence.
    ///
    /// The join is computed from what is **on disk**, not from what a caller believes, so two
    /// processes reporting attempts in either order converge on the same state and neither can
    /// lower it. The monotonicity that makes that true is a property of the lattice — see
    /// [`ironworks_domain::delivery`] — and it is re-asserted here against the stored value because
    /// this is the only place a regression could actually reach disk.
    pub fn record_delivery_attempt(
        &self,
        key: &IdempotencyKey,
        outcome: AttemptOutcome,
        now: &Utc,
    ) -> Result<DeliveryState, JournalError> {
        self.require_writable()?;
        let conn = self.locked();
        let stored: Option<(String, i64)> = conn
            .query_row(
                "SELECT delivery, delivery_chunks FROM turns WHERE key=?1",
                params![key.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some(stored) = stored else { return Err(JournalError::NoSuchTurn) };
        let before = decode_delivery(&stored.0, stored.1)
            .ok_or(JournalError::Sqlite(rusqlite::Error::InvalidQuery))?;
        let after = before.after(outcome);
        let chunks = match after {
            DeliveryState::Uncertain { acknowledged_chunks } => i64::from(acknowledged_chunks),
            _ => 0,
        };
        conn.execute(
            "UPDATE turns SET delivery=?2, delivery_chunks=?3, updated_at=?4 WHERE key=?1",
            params![key.as_str(), after.tag(), chunks, now.as_str()],
        )?;
        Ok(after)
    }

    /// Stop automatic work on a turn and raise the operator's alarm.
    ///
    /// Keeps any response id already known, because `set_outcome` refuses to
    /// unlearn one — and rightly: the id is half the evidence for diagnosing a duplicate.
    pub fn record_operator_required(
        &self,
        key: &IdempotencyKey,
        reason: &NonEmptyReason,
        now: &Utc,
    ) -> Result<(), JournalError> {
        let response = self.turn(key)?.ok_or(JournalError::NoSuchTurn)?.outcome.response().cloned();
        let outcome = TurnOutcome::OperatorRequired { response, reason: reason.clone() };
        // The outcome and its reason become durable together or not at all.
        //
        // These used to be two commits: `set_outcome` wrote `outcome='operator_required'` and
        // released the lock, and a second `UPDATE` then wrote the reason. A crash in that gap left
        // a row `row_to_record` refuses — "operator-required row without a reason" — and because
        // `unresolved` collects into one `Result`, that single row made the whole operator alarm
        // list unreadable. A state whose entire purpose is to raise an alarm could silence every
        // alarm at once. `commit_completed` already moves its two rows in one transaction, for the
        // same reason; this path had simply not been given it.
        let conn = self.locked();
        let tx = conn.unchecked_transaction()?;
        Self::set_outcome_in(&tx, key, &outcome, now)?;
        tx.execute(
            "UPDATE turns SET operator_reason=?2 WHERE key=?1",
            params![key.as_str(), reason.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Turns that are not both completed and delivered. Nothing here expires on age.
    ///
    /// The predicate is deliberately "not fully done" rather than "outcome is not completed": a
    /// completed turn whose answer is parked for reconciliation is exactly the case an operator
    /// needs to see, and the earlier predicate reported it as resolved.
    pub fn unresolved(&self) -> Result<Vec<TurnRecord>, JournalError> {
        let conn = self.locked();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TURN_COLUMNS} FROM turns
                 WHERE NOT (outcome = 'completed' AND delivery = 'delivered')
                 ORDER BY first_seen, key"
        ))?;
        let rows = stmt.query_map([], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// The journal is the authority on whether a tenant's turn is finished, so it is the journal that
/// answers the channel's one question. Nothing else about a turn is reachable through this trait.
impl TurnProgress for Journal {
    fn openness(&self, key: &IdempotencyKey) -> Openness {
        match self.turn(key) {
            Ok(Some(record)) => record.openness(),
            // A key with no row has no outstanding work: an update whose turn does not exist has
            // nothing outstanding, and a compacted row must not stall the cursor forever.
            Ok(None) => Openness::Settled,
            // **A read that failed has established nothing.**
            //
            // These two arms were one, and reporting a failed read as `Settled` was a control
            // failing open. The cursor is the lowest open update across EVERY tenant, so one
            // transient read error against a live, billed, undelivered turn moved the shared
            // barrier past it — and once the channel is acknowledged past an update it stops
            // redelivering it, so the answer is not delayed but gone, silently, for every tenant
            // behind it too. The comment that defended it — "it surfaces through `open`, which
            // fails closed" — is true of a file that will not open, and says nothing about a
            // connection that opened and then failed a read, which is the case that reaches here.
            //
            // # Why holding the barrier is safe here and was not in 0033
            //
            // 0033 is the counter-argument and it is a real one: refusing without settling held
            // the cursor forever on a turn that could never be rebuilt, and one stuck conversation
            // deafened every conversation behind it while the process looked healthy. The
            // difference is what the hold is attached to.
            //
            // That hold was permanent and local — one row, a deterministic verdict that would
            // never change, everything else in the process serving normally. When the store is
            // the thing that failed, this hold is the opposite on both axes, transient and
            // global: **a journal nobody can read is not one anybody is being served from.** Every
            // serving turn writes here before it dispatches — `begin` records the key and the
            // digest before a byte leaves — so a store that is not answering is a store that
            // cannot accept new work either. There is no healthy-process-being-deafened state for
            // this to create; there is only a stalled process, stalled, and it lifts the moment
            // the store answers.
            //
            // The case that argument does not cover is a single row this version will not parse.
            // That one *is* local and permanent, and it does hold the barrier until an operator
            // acts — 0033's shape. It is still the right answer, because the alternative is not
            // "keep serving": it is advancing the shared offset past a turn that may have been
            // billed and never delivered, on the strength of a row we just refused to read.
            // `row_to_record` refuses rather than interprets every unusable column for that
            // reason, and between two failures that both need an operator, the loud one is the
            // one that does not also lose the answer.
            Err(_) => Openness::Unestablished,
        }
    }
}

/// The two arms above that have no [`TurnRecord`] behind them at all.
///
/// `tests.rs` covers what an *answered* read yields, across every outcome and every delivery
/// state. These are about the answers that are not readings: no row, and no reply. They were one
/// arm, and the whole of the defect was that they had been given one meaning.
#[cfg(test)]
mod progress {
    use super::*;
    use crate::tests::{Tmp, key};

    /// A row written without [`Journal::begin`], which would want a verified instance. It is what
    /// `begin` writes; the outcome tag is the only thing these tests vary.
    fn row(j: &Journal, k: &str, tag: &str) {
        let conn = j.conn.lock().expect("journal lock");
        conn.execute(
            "INSERT INTO turns(key,instance,org,retrieved_at,request_digest,outcome,attempts,
                               first_seen,updated_at)
             VALUES(?1,'acme','acme-sales','r0','d0',?2,0,'t0','t0')",
            params![k, tag],
        )
        .expect("insert");
    }

    #[test]
    fn a_key_with_no_row_is_settled_and_a_key_that_could_not_be_read_is_not() {
        let t = Tmp::new("progress-split");
        let j = Journal::open(&t.0).expect("open");

        // No row: nothing outstanding, and it must stay that way. Holding the barrier here would
        // stall the cursor forever on an update whose row was compacted away — which is the
        // over-correction this half of the assertion exists to catch.
        assert_eq!(j.openness(&key("upd-1")), Openness::Settled);

        // A row whose outcome column this version will not interpret. `turn` returns `Err`, so
        // nothing has been established and the barrier holds.
        row(&j, "upd-2", "a-tag-from-another-version");
        assert!(
            j.turn(&key("upd-2")).is_err(),
            "the row read back cleanly — this test is measuring nothing"
        );
        assert_eq!(j.openness(&key("upd-2")), Openness::Unestablished);

        // POSITIVE CONTROL: the same shape of row, readable. A journal that answered
        // `Unestablished` to everything would satisfy the assertion above while telling the
        // cursor nothing at all.
        row(&j, "upd-3", "not_submitted");
        assert_eq!(j.openness(&key("upd-3")), Openness::Open);
    }
}

fn decode_delivery(tag: &str, chunks: i64) -> Option<DeliveryState> {
    match tag {
        "pending" => Some(DeliveryState::Pending),
        "known_unsent" => Some(DeliveryState::KnownUnsent),
        "uncertain" => {
            u32::try_from(chunks).ok().map(|c| DeliveryState::Uncertain { acknowledged_chunks: c })
        }
        "delivered" => Some(DeliveryState::Delivered),
        _ => None,
    }
}

/// Every column [`row_to_record`] reads, in one place.
///
/// # It was written out twice, and read by position
///
/// `Journal::turn` and `Journal::unresolved` each spelled these nineteen names, and the decoder
/// took them as `r.get(0)` … `r.get(18)`. Nothing coupled the two lists to those offsets — and
/// `instance` and `org` sit next to each other and are both `TEXT`, so reordering one list would
/// have compiled, run, and recorded a turn under the wrong tenant. Silent and cross-tenant is the
/// worst pair of properties a defect can have here.
///
/// The decoder now asks by name, so the offsets are rusqlite's problem rather than a reader's.
const TURN_COLUMNS: &str = "key,instance,org,retrieved_at,request_digest,outcome,response_id,\
                            attempts,first_seen,updated_at,delivery,delivery_chunks,\
                            operator_reason,prev_response,carried_records,profile_sha256,\
                            policy_sha256,catalogue_sha256,confinement_observed_at,\
                            reported_model";

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<TurnRecord> {
    let bad = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("the journal holds an unusable {what}").into(),
        )
    };
    let tag: String = r.get("outcome")?;
    let response: Option<String> = r.get("response_id")?;
    let response = match response {
        Some(v) => Some(ResponseId::parse(v).map_err(|_| bad("response id"))?),
        None => None,
    };
    let operator_reason: Option<String> = r.get("operator_reason")?;
    // An `accepted` or `completed` row with no response id cannot be produced by this code, and a
    // tag from a future version is not something to guess at. Refuse rather than interpret.
    let outcome = match (tag.as_str(), response) {
        ("not_submitted", None) => TurnOutcome::NotSubmitted,
        ("submitted_unknown", None) => TurnOutcome::SubmittedUnknown,
        ("accepted", Some(response)) => TurnOutcome::Accepted { response },
        ("completed", Some(response)) => TurnOutcome::Completed { response },
        // A row in this state without its reason is an alarm with no content, which is the one
        // thing the state exists to carry. Refuse it rather than report a blank.
        ("operator_required", response) => TurnOutcome::OperatorRequired {
            response,
            reason: operator_reason
                .and_then(|r| NonEmptyReason::parse(r).ok())
                .ok_or_else(|| bad("operator-required row without a reason"))?,
        },
        _ => return Err(bad("outcome")),
    };
    let prev: Option<String> = r.get("prev_response")?;
    let decision = Decision {
        previous: match prev {
            Some(v) => Some(ResponseId::parse(v).map_err(|_| bad("chained-from response id"))?),
            None => None,
        },
        carried: r.get("carried_records")?,
    };
    let delivery = decode_delivery(&r.get::<_, String>("delivery")?, r.get("delivery_chunks")?)
        .ok_or_else(|| bad("delivery state"))?;
    let reported_model =
        ReportedModel::read(r.get::<_, Option<String>>("reported_model")?.as_deref());
    let attribution_fields: (Option<String>, Option<String>, Option<String>, Option<String>) = (
        r.get("profile_sha256")?,
        r.get("policy_sha256")?,
        r.get("catalogue_sha256")?,
        r.get("confinement_observed_at")?,
    );
    let attribution = match attribution_fields {
        (None, None, None, None) => None,
        (Some(profile), Some(policy), Some(catalogue), Some(observed_at)) => {
            Some(CapabilityAttribution {
                profile_sha256: Sha256Digest::parse(profile)
                    .map_err(|_| bad("profile fingerprint"))?,
                policy_sha256: Sha256Digest::parse(policy)
                    .map_err(|_| bad("policy fingerprint"))?,
                catalogue_sha256: Sha256Digest::parse(catalogue)
                    .map_err(|_| bad("catalogue fingerprint"))?,
                observed_at: Utc::parse(observed_at)
                    .map_err(|_| bad("confinement observation time"))?,
            })
        }
        _ => return Err(bad("partial confinement attribution")),
    };
    Ok(TurnRecord {
        key: IdempotencyKey::parse(r.get::<_, String>("key")?)
            .map_err(|_| bad("idempotency key"))?,
        instance: Slug::parse(r.get::<_, String>("instance")?).map_err(|_| bad("instance"))?,
        org: OrgId::parse(r.get::<_, String>("org")?).map_err(|_| bad("org"))?,
        retrieved_at: r.get("retrieved_at")?,
        request_digest: RequestDigest::from_hex(r.get::<_, String>("request_digest")?),
        outcome,
        delivery,
        attempts: r.get("attempts")?,
        first_seen: r.get("first_seen")?,
        updated_at: r.get("updated_at")?,
        decision,
        attribution,
        reported_model,
    })
}

#[cfg(test)]
mod tests;
