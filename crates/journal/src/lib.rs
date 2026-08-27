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
//! The two meet through exactly one method — [`TurnProgress::openness`], two values — so the
//! coordinator that owns the shared cursor can order work without acquiring any tenant's
//! business context. See [`ironworks_domain::progress`] for why that trait is shaped the way it is.
//!
//! Nothing here ages anything away. Compaction is the channel's, it is bounded below the
//! acknowledged offset, and it never touches an update that cost a model turn.

use ironworks_domain::{
    AttemptOutcome, ChannelRef, DeliveryState, IdempotencyKey, NonEmptyReason, Openness, OrgId,
    PriorThread, Repair, ResponseId, Slug, SuppliedMark, ThreadPointer, ThreadScope, TurnProgress,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

mod digest;
pub use digest::RequestDigest;

/// The schema this implementation writes.
///
/// There is no migration path and none is planned. A journal written by an earlier version is
/// refused, loudly, by the version check in [`Journal::open`] — which is the same fail-closed
/// behaviour an unknown *future* version gets, and for the same reason: a journal whose shape we
/// cannot account for is one whose live conversations we must not guess at.
const SCHEMA_VERSION: i64 = 6;

const SCHEMA: &str = "
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
    operator_reason TEXT
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

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
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
    if let TurnOutcome::OperatorRequired { reason, response } = &record.outcome {
        return Recovery::OperatorRequired { reason: reason.clone(), response: response.clone() };
    }
    match record.delivery {
        DeliveryState::Delivered => return Recovery::Settled { delivery: record.delivery },
        DeliveryState::KnownUnsent | DeliveryState::Uncertain { .. } => {
            return Recovery::ReconcileDelivery {
                delivery: record.delivery,
                repair: record.delivery.repair(),
                response: record.outcome.response().cloned(),
            };
        }
        DeliveryState::Pending => {}
    }
    match &record.outcome {
        TurnOutcome::NotSubmitted => Recovery::Dispatch {
            key: record.key.clone(),
            retrieved_at: record.retrieved_at.clone(),
        },
        TurnOutcome::SubmittedUnknown => Recovery::ReplayIdentical {
            key: record.key.clone(),
            retrieved_at: record.retrieved_at.clone(),
            expect: record.request_digest.clone(),
        },
        TurnOutcome::Accepted { response } => Recovery::Fetch { response: response.clone() },
        TurnOutcome::Completed { response } => Recovery::Deliver { response: response.clone() },
        // Answered above, before delivery evidence was consulted.
        TurnOutcome::OperatorRequired { .. } => unreachable!("decided first"),
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

/// A store that cannot be read is either one that is not there and could not be made, or one
/// whose bytes are not a database. The second needs a different sentence: that file may hold
/// state nothing else has, so the operator instruction is to preserve it rather than clear it
/// and retry. Collapsing the two is how a recoverable store gets deleted by someone following
/// the obvious remedy for "could not be opened".
fn opened(path: &Path, source: rusqlite::Error) -> JournalError {
    if matches!(&source, rusqlite::Error::SqliteFailure(e, _)
        if e.code == rusqlite::ErrorCode::NotADatabase)
    {
        return JournalError::NotADatabase { path: path.display().to_string() };
    }
    JournalError::Open { path: path.display().to_string(), source }
}

impl Journal {
    /// Open for writing, creating the schema if absent.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .map_err(|source| JournalError::Open { path: path.display().to_string(), source })?;
        // Before the schema, so a file that is both readable and non-empty never exists.
        ironworks_domain::restrict_to_owner(path)
            .map_err(|source| JournalError::Exposed { path: path.display().to_string(), source })?;
        // The first statement is where a file that is not a database says so: `open` itself is
        // lazy and succeeds against any path it can touch.
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| opened(path, e))?;
        // A crash test that survives fsync is the only kind worth writing.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        let found: Option<i64> = conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok());
        match found {
            None => {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES('schema_version',?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION => {}
            Some(found) => return Err(JournalError::Version { found, expected: SCHEMA_VERSION }),
        }
        Ok(Self { conn: Mutex::new(conn), writable: true })
    }

    /// Open for **inspection**: read-only at the SQLite level, so a reader cannot mutate a live
    /// journal even by mistake. Looking at production must not be able to change it.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| JournalError::Open { path: path.display().to_string(), source })?;
        // Inspection must reject a journal this binary would refuse to serve. Without the
        // version check, an old schema that happens to retain these columns could look healthy
        // while carrying semantics this binary no longer understands.
        let found: Option<i64> = conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(|source| opened(path, source))?
            .and_then(|v| v.parse().ok());
        match found {
            Some(v) if v == SCHEMA_VERSION => {}
            Some(found) => return Err(JournalError::Version { found, expected: SCHEMA_VERSION }),
            None => return Err(JournalError::Version { found: -1, expected: SCHEMA_VERSION }),
        }
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
        now: &str,
    ) -> Result<(), JournalError> {
        self.require_writable()?;
        let conn = self.conn.lock().expect("journal lock");
        conn.execute(
            "INSERT INTO turns(key,instance,org,retrieved_at,request_digest,outcome,attempts,
                               first_seen,updated_at,prev_response,carried_records)
             VALUES(?1,?2,?3,?4,?5,'not_submitted',0,?6,?6,?7,?8)
             ON CONFLICT(key) DO NOTHING",
            params![
                key.as_str(),
                scope.slug().as_str(),
                scope.org().as_str(),
                retrieved_at,
                digest.as_str(),
                now,
                decision.previous.as_ref().map(ResponseId::as_str),
                decision.carried,
            ],
        )?;
        Ok(())
    }

    /// A dispatch attempt provably never left. Still safe to send.
    pub fn record_unsent(&self, key: &IdempotencyKey, now: &str) -> Result<(), JournalError> {
        self.set_outcome(key, &TurnOutcome::NotSubmitted, now)
    }

    /// A dispatch attempt may have executed.
    ///
    /// A one-way door: this is never walked back to `not_submitted`, because the evidence that
    /// produced it does not improve with time.
    pub fn record_unknown(&self, key: &IdempotencyKey, now: &str) -> Result<(), JournalError> {
        self.set_outcome(key, &TurnOutcome::SubmittedUnknown, now)
    }

    /// The runtime named a response.
    pub fn record_accepted(
        &self,
        key: &IdempotencyKey,
        response: &ResponseId,
        now: &str,
    ) -> Result<(), JournalError> {
        self.set_outcome(key, &TurnOutcome::Accepted { response: response.clone() }, now)
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
        now: &str,
    ) -> Result<(), JournalError> {
        self.require_writable()?;
        let mut conn = self.conn.lock().expect("journal lock");
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
            params![key.as_str(), pointer.response().as_str(), now],
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
                    now
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
        now: &str,
    ) -> Result<(), JournalError> {
        let (tag, response) = (outcome.tag(), outcome.response());
        self.require_writable()?;
        let conn = self.conn.lock().expect("journal lock");
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
            params![key.as_str(), tag, response.map(ResponseId::as_str), now],
        )?;
        Ok(())
    }

    /// One journalled turn.
    pub fn turn(&self, key: &IdempotencyKey) -> Result<Option<TurnRecord>, JournalError> {
        let conn = self.conn.lock().expect("journal lock");
        conn.query_row(
            "SELECT key,instance,org,retrieved_at,request_digest,outcome,response_id,attempts,
                    first_seen,updated_at,delivery,delivery_chunks,operator_reason,
                    prev_response,carried_records
             FROM turns WHERE key=?1",
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
        let conn = self.conn.lock().expect("journal lock");
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
        now: &str,
    ) -> Result<DeliveryState, JournalError> {
        self.require_writable()?;
        let conn = self.conn.lock().expect("journal lock");
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
            params![key.as_str(), after.tag(), chunks, now],
        )?;
        Ok(after)
    }

    /// Stop automatic work on a turn and raise the operator's alarm.
    ///
    /// Keeps any response id already known, because [`set_outcome`](Self::set_outcome) refuses to
    /// unlearn one — and rightly: the id is half the evidence for diagnosing a duplicate.
    pub fn record_operator_required(
        &self,
        key: &IdempotencyKey,
        reason: &NonEmptyReason,
        now: &str,
    ) -> Result<(), JournalError> {
        let response = self.turn(key)?.ok_or(JournalError::NoSuchTurn)?.outcome.response().cloned();
        let outcome = TurnOutcome::OperatorRequired { response, reason: reason.clone() };
        self.set_outcome(key, &outcome, now)?;
        let conn = self.conn.lock().expect("journal lock");
        conn.execute(
            "UPDATE turns SET operator_reason=?2 WHERE key=?1",
            params![key.as_str(), reason.as_str()],
        )?;
        Ok(())
    }

    /// Turns that are not both completed and delivered. Nothing here expires on age.
    ///
    /// The predicate is deliberately "not fully done" rather than "outcome is not completed": a
    /// completed turn whose answer is parked for reconciliation is exactly the case an operator
    /// needs to see, and the earlier predicate reported it as resolved.
    pub fn unresolved(&self) -> Result<Vec<TurnRecord>, JournalError> {
        let conn = self.conn.lock().expect("journal lock");
        let mut stmt = conn.prepare(
            "SELECT key,instance,org,retrieved_at,request_digest,outcome,response_id,attempts,
                    first_seen,updated_at,delivery,delivery_chunks,operator_reason,
                    prev_response,carried_records FROM turns
             WHERE NOT (outcome = 'completed' AND delivery = 'delivered')
             ORDER BY first_seen, key",
        )?;
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
            // A key with no row has no outstanding work. A read failure is reported as settled
            // for the same reason: an unreadable journal must not silently freeze the cursor and
            // make the process deaf — it surfaces through `open`, which fails closed.
            Ok(None) | Err(_) => Openness::Settled,
        }
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

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<TurnRecord> {
    let bad = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("the journal holds an unusable {what}").into(),
        )
    };
    let tag: String = r.get(5)?;
    let response: Option<String> = r.get(6)?;
    let response = match response {
        Some(v) => Some(ResponseId::parse(v).map_err(|_| bad("response id"))?),
        None => None,
    };
    let operator_reason: Option<String> = r.get(12)?;
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
    let prev: Option<String> = r.get(13)?;
    let decision = Decision {
        previous: match prev {
            Some(v) => Some(ResponseId::parse(v).map_err(|_| bad("chained-from response id"))?),
            None => None,
        },
        carried: r.get(14)?,
    };
    let delivery = decode_delivery(&r.get::<_, String>(10)?, r.get(11)?)
        .ok_or_else(|| bad("delivery state"))?;
    Ok(TurnRecord {
        key: IdempotencyKey::parse(r.get::<_, String>(0)?).map_err(|_| bad("idempotency key"))?,
        instance: Slug::parse(r.get::<_, String>(1)?).map_err(|_| bad("instance"))?,
        org: OrgId::parse(r.get::<_, String>(2)?).map_err(|_| bad("org"))?,
        retrieved_at: r.get(3)?,
        request_digest: RequestDigest::from_hex(r.get::<_, String>(4)?),
        outcome,
        delivery,
        attempts: r.get(7)?,
        first_seen: r.get(8)?,
        updated_at: r.get(9)?,
        decision,
    })
}

#[cfg(test)]
mod tests;
