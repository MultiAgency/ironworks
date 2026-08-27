//! Channel durability: what survives process death **around delivery**, across every tenant at
//! once.
//!
//! # The question this answers
//!
//! `ironworks-journal` answers *what became durable about one turn?* — and everything it holds is
//! scoped to one tenant. This crate answers the next boundary, and it is the first one that is
//! not:
//!
//! > **An inbound stream has one offset. Whose unfinished work may hold it, and what may the
//! > thing that decides know?**
//!
//! # Two axes, kept apart
//!
//! - **Delivery** — did the outbound answer reach the client? Tenant-scoped, and it lives with
//!   the turn, in [`ironworks_domain::delivery`] and the turn journal.
//! - **Acknowledgement** — has the channel been told we are past this inbound update, so it will
//!   stop sending it? Channel-scoped, shared, and it lives here.
//!
//! Collapsing them is the defect the reference implementation names `DELIVERED` versus `ACKED`.
//! An answer can be in a client's room while the channel still intends to redeliver the question
//! that produced it; a process that treats those as one fact will either drop the answer or send
//! it twice, and which one is a matter of luck.
//!
//! # What this crate is structurally forbidden from knowing
//!
//! Because the coordinator is cross-tenant, it is exactly where a scope leak would be invisible.
//! So it is denied the vocabulary:
//!
//! > **Channel-level state may coordinate ordering across tenants, but may never acquire tenant
//! > business authority or model context.**
//!
//! It holds update ids, a channel address, a routing decision, and a timestamp. It learns one
//! thing about a tenant's work — [`Openness`], two values — through [`TurnProgress`]. It cannot
//! reach a record, an org, a persona, a question or an answer, and
//! `dependencies_are_the_narrow_set` keeps it that way by asserting this crate's own dependency
//! list.
//!
//! # Boundaries B5–B7
//!
//! - **B5** — the process died between the send and its record. The update is still open, so it
//!   is redelivered and answered again from the *stored response id*: a duplicate, and
//!   necessarily identical. `tests/crash_boundaries.rs` proves the identity, not just the count.
//! - **B6** — delivered without the cursor advancing, or the reverse. **Unrepresentable**: the
//!   cursor is derived, so there is no second write to be non-atomic with. See [`cursor`].
//! - **B7** — the offset is durable but the channel has not been told. It redelivers; the update
//!   is already settled, so there is no rerun and no resend, only an acknowledgement.
//!
//! Those are unit boundaries. The same four questions are asked again in `dev/prove-delivery.sh`
//! against a live runtime and a disposable channel, across genuine process deaths — because a
//! state machine agreeing with itself is not the question. The question is whether a client reads
//! the same answer twice, and the only witness to that is the channel.

use ironworks_domain::{ChannelRef, IdempotencyKey, Openness, TurnProgress};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

pub mod cursor;
pub use cursor::safe_cursor;

/// The schema this implementation writes. Refused if it does not match, in either direction.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS channel_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS channel_updates (
    id            INTEGER PRIMARY KEY,
    channel       TEXT NOT NULL,
    disposition   TEXT NOT NULL,
    -- Present only for `turn`. It is a handle into the tenant journal, and the only thing here
    -- that refers to a tenant's work at all.
    turn_key      TEXT,
    received_at   TEXT NOT NULL,
    settled_at    TEXT
);
CREATE INDEX IF NOT EXISTS channel_updates_disposition ON channel_updates(disposition);
";

/// The key under which the acknowledged offset lives. It is stored because it is **not**
/// derivable: it records what the channel was actually told, which no amount of local state can
/// imply.
const ACKED: &str = "cursor_acked";

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("the channel journal at {path} could not be opened: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "the channel journal at {path} is not a readable database. Refusing to run — and do NOT delete it \
         blind: it is the record of how far the channel has been acknowledged, and losing it redelivers every update it still holds. Move it aside and keep it."
    )]
    NotADatabase { path: String },
    #[error(
        "the channel journal at {path} was created but could not be restricted to its owner: {source}. \
         Refusing to run: the file names every organization served and every turn outstanding, \
         and continuing would leave that readable while reporting success."
    )]
    Exposed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the channel journal is schema version {found}, this implementation writes {expected}. \
         Refusing to run: an unknown version means a downgrade or an unfinished write, and an \
         offset guessed from a shape we do not understand redelivers or drops live messages."
    )]
    Version { found: i64, expected: i64 },
    #[error("update {id} is not journalled")]
    NoSuchUpdate { id: u64 },
    #[error(
        "update id {id} does not fit the journal's signed integer. No channel numbers updates \
         this high; storing it would put two updates at one offset."
    )]
    UnrepresentableId { id: u64 },
    #[error(
        "update {id} is already dispositioned as {existing}; a disposition is written once. \
         Rewriting one is how an update with a possibly-billed answer becomes 'not addressed to \
         us' and stops being recoverable."
    )]
    AlreadyDispositioned { id: u64, existing: &'static str },
    #[error(
        "an acknowledged offset of {attempted} is behind the recorded {existing} — the channel \
         has already been told {existing}, and unsaying it would redeliver messages a client has \
         been answered about"
    )]
    AcknowledgementWouldRegress { existing: u64, attempted: u64 },
    #[error(
        "offset {attempted} is past the safe cursor {safe}: update {safe} is still open, and \
         acknowledging past it would let the channel stop redelivering work that was never done"
    )]
    AcknowledgementPastTheBarrier { safe: u64, attempted: u64 },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// A channel's own numbering of an inbound update. Opaque, monotone, and not ours to invent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UpdateId(u64);

impl UpdateId {
    pub fn new(n: u64) -> Self {
        Self(n)
    }
    pub fn get(self) -> u64 {
        self.0
    }
    /// The offset that acknowledges *through* this update. Saturating, because an offset past the
    /// end of the number line is a worse answer than a stuck one.
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
    /// SQLite's `INTEGER` is signed, and an id is stored as one so the journal sorts by it.
    ///
    /// No channel produces an id past `i64::MAX`, so this is not a case to handle gracefully —
    /// it is a case to refuse. Saturating would put two different updates at the same offset.
    fn sql(self) -> Result<i64, ChannelError> {
        i64::try_from(self.0).map_err(|_| ChannelError::UnrepresentableId { id: self.0 })
    }
}

impl std::fmt::Display for UpdateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What was decided about an inbound update.
///
/// # A disposition is written once
///
/// [`ChannelJournal::disposition`] refuses to overwrite one, and that refusal is the whole of
/// E-4. In the reference implementation, routing and addressing return the same "no" — so an
/// update already in flight, with an answer that may already have been billed, could be
/// re-dispositioned as *not addressed to us*, have the cursor advanced past it, and become
/// unrecoverable. Making the transition one-way means the in-flight case cannot be relabelled:
/// its route may vanish, but its history cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Journalled on arrival, before routing and before any worker. **Open.**
    ///
    /// This is what "journal the batch first" buys: a crash between the poll and the work leaves
    /// every update in the batch here, so the offset stays behind all of them.
    Unclaimed,
    /// A turn was minted for this update. Open exactly while that turn is open — a question this
    /// crate asks [`TurnProgress`] and cannot answer itself.
    Turn(IdempotencyKey),
    /// Routed, but the message was not addressed to us. **Settled** on arrival: no turn was
    /// minted, nothing was billed, and there is nothing to recover.
    NotAddressed,
    /// Could not be routed to exactly one tenant — no claimant, or more than one. **Settled**,
    /// because no turn was minted, but recorded rather than discarded: an update arriving for a
    /// channel nobody owns is a configuration defect, and it is only visible here.
    Unroutable,
    /// A command this process answered deterministically. **Settled**: no turn was minted and
    /// nothing was billed.
    ///
    /// Distinct from [`NotAddressed`](Self::NotAddressed) because that one says *nobody acted on
    /// this*, and a command is the opposite — it was addressed to us and we did something. A
    /// disposition is written once and can never be relabelled (E-4), so writing the wrong one
    /// here would put a permanent falsehood in the record of what happened.
    ///
    /// Named for the **class of update**, not for what one command happened to produce. Every
    /// deterministic reply lands here — a refusal, an unknown verb, a proposal — because most of
    /// them create nothing, and a disposition called `Proposal` on an update that made no
    /// proposal would be a second falsehood in the same one-way row. Whether a proposal exists,
    /// and what became of showing it, belongs to the proposal store.
    ///
    /// It is written **only after the reply's outcome is known**, and only when that outcome was
    /// not the channel's positive claim that nothing was delivered. A reply that provably never
    /// left leaves the update `Unclaimed`, so the channel redelivers it and the attempt is made
    /// again — which is safe precisely because nothing reached anyone.
    Command,
}

impl Disposition {
    fn tag(&self) -> &'static str {
        match self {
            Self::Unclaimed => "unclaimed",
            Self::Turn(_) => "turn",
            Self::NotAddressed => "not_addressed",
            Self::Unroutable => "unroutable",
            Self::Command => "command",
        }
    }

    /// Whether this update still holds the cursor, given what the tenant journal says.
    ///
    /// The only place the two scopes meet, and it is one call returning two values.
    pub fn openness(&self, progress: &dyn TurnProgress) -> Openness {
        match self {
            Self::Unclaimed => Openness::Open,
            Self::Turn(key) => progress.openness(key),
            Self::NotAddressed | Self::Unroutable | Self::Command => Openness::Settled,
        }
    }

    /// Whether this update ever cost a model turn.
    ///
    /// The compaction rule in one predicate: an update that cost nothing may be forgotten, and
    /// one that cost a turn may not.
    pub fn cost_a_turn(&self) -> bool {
        matches!(self, Self::Turn(_))
    }
}

/// One journalled inbound update. Identifiers only — never the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRecord {
    pub id: UpdateId,
    pub channel: ChannelRef,
    pub disposition: Disposition,
    pub received_at: String,
    pub settled_at: Option<String>,
}

/// The cross-tenant coordinator.
///
/// Deliberately not a service object and deliberately not per-tenant: it owns the shared inbound
/// stream and nothing else. Its whole surface is update ids, routing decisions, and an offset.
pub struct ChannelJournal {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for ChannelJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelJournal").finish_non_exhaustive()
    }
}

/// A store that cannot be read is either one that is not there and could not be made, or one
/// whose bytes are not a database. The second needs a different sentence: that file may hold
/// state nothing else has, so the operator instruction is to preserve it rather than clear it
/// and retry. Collapsing the two is how a recoverable store gets deleted by someone following
/// the obvious remedy for "could not be opened".
fn opened(path: &Path, source: rusqlite::Error) -> ChannelError {
    if matches!(&source, rusqlite::Error::SqliteFailure(e, _)
        if e.code == rusqlite::ErrorCode::NotADatabase)
    {
        return ChannelError::NotADatabase { path: path.display().to_string() };
    }
    ChannelError::Open { path: path.display().to_string(), source }
}

impl ChannelJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ChannelError> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .map_err(|source| ChannelError::Open { path: path.display().to_string(), source })?;
        // Before the schema, so a file that is both readable and non-empty never exists.
        ironworks_domain::restrict_to_owner(path)
            .map_err(|source| ChannelError::Exposed { path: path.display().to_string(), source })?;
        // The first statement is where a file that is not a database says so: `open` itself is
        // lazy and succeeds against any path it can touch.
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| opened(path, e))?;
        // The whole point of this file is to be true after a power cut, not merely after a clean
        // exit. `FULL` is the setting that buys that, and its cost is paid once per update.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        let found: Option<String> = conn
            .query_row("SELECT value FROM channel_meta WHERE key='schema_version'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match found {
            None => {
                conn.execute(
                    "INSERT INTO channel_meta(key,value) VALUES('schema_version',?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) => {
                let found = v.parse::<i64>().unwrap_or(-1);
                if found != SCHEMA_VERSION {
                    return Err(ChannelError::Version { found, expected: SCHEMA_VERSION });
                }
            }
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Journal a whole poll's worth of updates **before any of them is worked on**, in one
    /// transaction.
    ///
    /// # Why the batch, and why before
    ///
    /// One global offset means a fast tenant's completion can acknowledge a slow tenant's
    /// unfinished work — but only if the slow tenant's update is not on disk yet. An update that
    /// exists is [`Disposition::Unclaimed`] and therefore open, so it holds the barrier from the
    /// moment it arrives. Journalling the batch first is what makes the barrier apply to work
    /// that has not started.
    ///
    /// Idempotent per id: a redelivered update keeps its original row rather than resetting its
    /// disposition. That is B1 — a crash before any of this leaves nothing durable, the channel
    /// resends, and the update is handled exactly once.
    pub fn note_batch(
        &self,
        updates: &[(UpdateId, ChannelRef)],
        now: &str,
    ) -> Result<usize, ChannelError> {
        let mut conn = self.conn.lock().expect("channel journal lock");
        let tx = conn.transaction()?;
        let mut fresh = 0;
        for (id, channel) in updates {
            fresh += tx.execute(
                "INSERT INTO channel_updates(id,channel,disposition,received_at)
                 VALUES(?1,?2,'unclaimed',?3) ON CONFLICT(id) DO NOTHING",
                params![id.sql()?, channel.as_str(), now],
            )?;
        }
        tx.commit()?;
        Ok(fresh)
    }

    /// Record what was decided about an update. One-way: see [`Disposition`].
    pub fn disposition(
        &self,
        id: UpdateId,
        disposition: &Disposition,
        now: &str,
    ) -> Result<(), ChannelError> {
        let conn = self.conn.lock().expect("channel journal lock");
        let existing: Option<String> = conn
            .query_row(
                "SELECT disposition FROM channel_updates WHERE id=?1",
                params![id.sql()?],
                |r| r.get(0),
            )
            .optional()?;
        let Some(existing) = existing else {
            return Err(ChannelError::NoSuchUpdate { id: id.get() });
        };
        if existing != "unclaimed" {
            return Err(ChannelError::AlreadyDispositioned {
                id: id.get(),
                existing: match existing.as_str() {
                    "turn" => "turn",
                    "not_addressed" => "not_addressed",
                    "command" => "command",
                    _ => "unroutable",
                },
            });
        }
        let key = match disposition {
            Disposition::Turn(k) => Some(k.as_str()),
            _ => None,
        };
        // `settled_at` is a timestamp on the *disposition*, not on the turn. An update handed to
        // a tenant is not settled here; the tenant journal decides when it is.
        let settled = if disposition.cost_a_turn() { None } else { Some(now) };
        conn.execute(
            "UPDATE channel_updates SET disposition=?2, turn_key=?3, settled_at=?4 WHERE id=?1",
            params![id.sql()?, disposition.tag(), key, settled],
        )?;
        Ok(())
    }

    pub fn record(&self, id: UpdateId) -> Result<Option<UpdateRecord>, ChannelError> {
        let conn = self.conn.lock().expect("channel journal lock");
        conn.query_row(
            "SELECT id,channel,disposition,turn_key,received_at,settled_at
             FROM channel_updates WHERE id=?1",
            params![id.sql()?],
            row_to_record,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Every journalled update, oldest first.
    pub fn records(&self) -> Result<Vec<UpdateRecord>, ChannelError> {
        let conn = self.conn.lock().expect("channel journal lock");
        let mut stmt = conn.prepare(
            "SELECT id,channel,disposition,turn_key,received_at,settled_at
             FROM channel_updates ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// The offset the channel may safely be told, computed from durable rows every time.
    ///
    /// Not cached, not stored, not passed between calls. After a crash this returns the same
    /// answer it returned before, because it never depended on anything the crash could take.
    pub fn safe_cursor(
        &self,
        progress: &dyn TurnProgress,
    ) -> Result<Option<UpdateId>, ChannelError> {
        let states: Vec<(UpdateId, Openness)> =
            self.records()?.into_iter().map(|r| (r.id, r.disposition.openness(progress))).collect();
        Ok(safe_cursor(&states))
    }

    /// Record that the channel was told this offset — which is the moment the updates below it
    /// stop being redeliverable, and therefore the moment it becomes safe to forget them.
    ///
    /// Refuses to regress, and refuses to move past the barrier. The second check is the one that
    /// matters: a caller that computed a cursor, then lost a race to a slower tenant journalling
    /// work, would otherwise acknowledge past an update that is open by the time the write lands.
    pub fn acknowledge(
        &self,
        offset: UpdateId,
        progress: &dyn TurnProgress,
    ) -> Result<(), ChannelError> {
        if let Some(safe) = self.safe_cursor(progress)?
            && offset > safe
        {
            return Err(ChannelError::AcknowledgementPastTheBarrier {
                safe: safe.get(),
                attempted: offset.get(),
            });
        }
        let conn = self.conn.lock().expect("channel journal lock");
        let existing: Option<u64> = conn
            .query_row("SELECT value FROM channel_meta WHERE key=?1", params![ACKED], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok());
        if let Some(existing) = existing
            && offset.get() < existing
        {
            return Err(ChannelError::AcknowledgementWouldRegress {
                existing,
                attempted: offset.get(),
            });
        }
        conn.execute(
            "INSERT INTO channel_meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![ACKED, offset.get().to_string()],
        )?;
        Ok(())
    }

    /// The offset the channel has actually been told, if any.
    pub fn acknowledged(&self) -> Result<Option<UpdateId>, ChannelError> {
        let conn = self.conn.lock().expect("channel journal lock");
        let v: Option<String> = conn
            .query_row("SELECT value FROM channel_meta WHERE key=?1", params![ACKED], |r| r.get(0))
            .optional()?;
        Ok(v.and_then(|v| v.parse().ok()).map(UpdateId::new))
    }

    /// The offset to send the channel on the next poll.
    ///
    /// Three cases, and the middle one is the whole reason this is not
    /// `safe_cursor().unwrap_or(0)`:
    ///
    /// 1. **Rows exist.** The barrier governs — everything below the lowest open update is
    ///    finished, across every tenant.
    /// 2. **No rows, but an acknowledgement is recorded.** Compaction removed the rows the
    ///    derivation reads, so the stored acknowledgement is what carries the position. ADR 0006
    ///    names this as the one failure mode deriving the cursor introduces: *"a derivation that
    ///    fell back to zero here would redeliver the entire history"*.
    /// 3. **Neither.** Nothing has ever been journalled, and `0` — the beginning — is correct.
    ///
    /// The offset is computed here rather than at the call site because case 2 is a protocol
    /// fact, not a caller's business, and a binary that reconstructs it is a binary that can get
    /// it wrong once per call site. [`Self::acknowledge`] already refuses to let the *stored*
    /// value regress; this is what stops the *wire* value regressing past it.
    pub fn poll_offset(&self, progress: &dyn TurnProgress) -> Result<u64, ChannelError> {
        if let Some(safe) = self.safe_cursor(progress)? {
            // Deliberately not `max(safe, acknowledged)`: a barrier below the acknowledged offset
            // means a tenant has unfinished work down there, and re-seeing an update is always
            // safer than skipping one.
            return Ok(safe.get());
        }
        Ok(self.acknowledged()?.map_or(0, UpdateId::get))
    }

    /// Whether one update has been acknowledged to the channel.
    ///
    /// Derived, like the cursor: an update is acknowledged exactly when the offset the channel
    /// was told is past it. There is no `ACKED` column, so there is no way for a row to claim
    /// acknowledgement the channel was never given.
    pub fn is_acknowledged(&self, id: UpdateId) -> Result<bool, ChannelError> {
        Ok(self.acknowledged().is_ok_and(|a| a.is_some_and(|acked| id < acked)))
    }

    /// Forget updates the channel will never send again.
    ///
    /// Two bounds, and both are load-bearing:
    ///
    /// 1. **Below the acknowledged offset, never the safe one.** Compaction is bounded by what
    ///    the channel has been *told*, not by what this process has *done*. Dropping a row the
    ///    channel can still redeliver is the duplicate defect, reintroduced by tidiness — the
    ///    update comes back, nothing here remembers answering it, and it is answered again.
    /// 2. **Only updates that never cost a turn.** Anything that produced a model turn is kept
    ///    forever. Its id and its delivery evidence are how a client's "I got two answers" report
    ///    is diagnosed days later, and nothing ages that away. This is stricter than the
    ///    reference implementation, which retains terminal rows by enumerating the states worth
    ///    keeping — an enumeration that was wrong twice, most expensively when the
    ///    operator-reconciliation alarm turned out to be age-compactable and a possibly-billed,
    ///    never-delivered turn was deleted with nobody having acted on it.
    pub fn compact(&self) -> Result<usize, ChannelError> {
        let Some(acked) = self.acknowledged()? else { return Ok(0) };
        let conn = self.conn.lock().expect("channel journal lock");
        let removed = conn.execute(
            "DELETE FROM channel_updates
             WHERE id < ?1 AND disposition IN ('not_addressed','unroutable')",
            params![acked.sql()?],
        )?;
        Ok(removed)
    }
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateRecord> {
    let bad = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("the channel journal holds an unusable {what}").into(),
        )
    };
    let tag: String = r.get(2)?;
    let key: Option<String> = r.get(3)?;
    // A `turn` row without its key is a reference to a tenant's work that nothing can follow.
    // Refuse it rather than silently treat it as unclaimed and answer the question twice.
    let disposition = match (tag.as_str(), key) {
        ("unclaimed", None) => Disposition::Unclaimed,
        ("not_addressed", None) => Disposition::NotAddressed,
        ("unroutable", None) => Disposition::Unroutable,
        ("command", None) => Disposition::Command,
        ("turn", Some(k)) => {
            Disposition::Turn(IdempotencyKey::parse(k).map_err(|_| bad("idempotency key"))?)
        }
        _ => return Err(bad("disposition")),
    };
    Ok(UpdateRecord {
        id: UpdateId::new(u64::try_from(r.get::<_, i64>(0)?).map_err(|_| bad("update id"))?),
        channel: ChannelRef::new(r.get::<_, String>(1)?),
        disposition,
        received_at: r.get(4)?,
        settled_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests;
