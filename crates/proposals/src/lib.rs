//! Durable proposals: what a person has asked to promote, and not yet confirmed.
//!
//! # Why this is not a table in the journal
//!
//! `ironworks-journal` holds **identifiers only — never message text**, because everything in it
//! survives a crash on purpose and a credential or a message body there would be a second copy of
//! something that should not have one. A proposal is the person's exact words. Putting it beside
//! the turn rows would break that rule for the whole file, so it gets its own store, and the
//! reason is the rule rather than tidiness.
//!
//! It is a **buffer, not a second record store.** Nothing reads it to answer a question. A
//! proposal exists between `/share` and `/confirm` and its durable home afterwards is the Account
//! Service.
//!
//! # The two rules the type system carries
//!
//! Both are things a later patch must not be able to get wrong, so neither is left to a check a
//! caller has to remember:
//!
//! 1. **An unconfirmed echo cannot be confirmed.** A person who never saw the proposal cannot
//!    have approved it. The only route to a confirmable value is [`ProposalStore::confirmable`],
//!    which returns `None` for anything the channel did not confirm delivering.
//! 2. **A proposal belongs to the actor who made it.** [`ProposalStore::confirmable`] takes the
//!    actor and matches on it, so there is no call that could hand one person another's words.
//!
//! [`Confirmable`] has no public constructor, which is what makes those two facts unavoidable
//! rather than advisory.

use ironworks_domain::{
    ActorRef, AttemptOutcome, ChannelRef, ContributorId, OrgId, Slug, Utc, record::AccountId,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

/// The schema this implementation writes.
///
/// Refused rather than migrated, exactly as the turn journal refuses: a store whose shape we
/// cannot account for may hold words a person is waiting on, and guessing at them is worse than
/// stopping.
///
/// **2 since promotion landed.** A version-1 file holds proposals but no confirmation lane, so a
/// version-1 row cannot say whether its words were already appended. Reading one under these
/// semantics would risk appending a second time; rewriting one would decide, on its owner's
/// behalf, that words they never confirmed are now unconfirmable. Both are worse than refusing,
/// so an older file is kept, named, and left for an operator.
///
/// **3 since a refused append got a terminal state**, and this one MIGRATES where 1 does not —
/// the difference is whether an existing row can be misread, not a preference for one policy.
/// A version-2 row is read identically under version-3 semantics: `refused` is a state only this
/// build writes, so no v2 row carries it and nothing about an existing proposal changes meaning.
/// The migration adds one nullable column and touches no row. Refusing the store instead would
/// strand someone's unconfirmed words behind an operator ticket to buy nothing.
///
/// The other direction is covered without a version check: an older binary meeting a `refused`
/// row cannot read the tag, and `ProposalStore::read` refuses an unknown tag rather than
/// coercing it. A downgrade therefore fails closed on the one row it cannot understand instead of
/// treating a refused append as retryable.
pub const SCHEMA_VERSION: i64 = 3;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS proposal_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Keyed by the channel update that created it, and by nothing else. A redelivery of that update
-- is the same request arriving twice, so it must find the same row rather than mint a second
-- proposal; there is no separate proposal id because a second name for one thing is a second
-- thing to keep in agreement.
CREATE TABLE IF NOT EXISTS proposals (
    update_id    INTEGER PRIMARY KEY,
    -- Every column below except `echo` and `echoed_at` is immutable after creation. A redelivery
    -- carrying different values is a conflict, never an update.
    -- The conversation it was made in. Authorization is the (channel, actor) pair, so the
    -- channel is part of what a later confirmation has to match, not decoration.
    channel      TEXT NOT NULL,
    actor_key    TEXT NOT NULL,
    contributor  TEXT NOT NULL,
    slug         TEXT NOT NULL,
    org          TEXT NOT NULL,
    account      TEXT NOT NULL,
    body         TEXT NOT NULL,
    proposed_at  TEXT NOT NULL,
    -- 'pending' | 'attempting' | 'delivered' | 'uncertain'.
    --
    -- `attempting` is the durable send INTENT, written before a byte leaves. Without it a crash
    -- between an accepted send and a recorded outcome is indistinguishable from a send that never
    -- happened, and the redelivery would put a second copy of the proposal in the room. With it
    -- that window reads as tried-and-cannot-say, which a person settles rather than a retry.
    --
    -- There is no 'rejected': a send the channel proved never left returns to 'pending', because
    -- a retry is safe and the person has still seen nothing.
    echo         TEXT NOT NULL DEFAULT 'pending',
    -- How many chunks the channel CONFIRMED when it could not confirm the rest. Present only for
    -- 'uncertain', where it is what an operator needs to decide whether a resend duplicates.
    echo_chunks  INTEGER,
    echoed_at    TEXT,

    -- ── the confirmation lane (schema 2) ────────────────────────────────────────────────────
    -- 'unconfirmed' | 'appending' | 'committed' | 'contested' | 'refused'.
    --
    -- `appending` is the durable append INTENT, written before the request leaves, carrying the
    -- deterministic activity id and the date — the two parts of the immutable request that are
    -- not already columns on this row. A crash between the POST and its recorded outcome
    -- therefore leaves a state that knows exactly which byte-identical request to replay, which
    -- is safe precisely because the endpoint is idempotent on it.
    --
    -- `contested` is a 409 the store decided: an activity with that id exists carrying different
    -- content. Permanent, and an operator's, because two different notes cannot both be the
    -- promotion of one proposal and neither may overwrite the other.
    --
    -- `refused` is every OTHER decision the store made against the request: unauthorized, unknown
    -- account, a shape it will not take. Also permanent. It exists because without it those
    -- outcomes were left in `appending`, which says the opposite — no outcome became durable —
    -- and every later /confirm replayed a request the store had already decided about.
    confirm         TEXT NOT NULL DEFAULT 'unconfirmed',
    -- Which `/confirm` message began this. Kept so a redelivery of that update finds its own
    -- attempt rather than starting a second one.
    confirm_update  INTEGER,
    activity_id     TEXT,
    occurred_at     TEXT,
    -- The organization the STORE answered with, not the one we asked about. Persisted before the
    -- success reply, so a crash after committing still knows what was committed.
    committed_org   TEXT,
    contested_field TEXT,
    -- The store's own refusal code, for `confirm = 'refused'`. Its own column rather than a second
    -- meaning for `contested_field`: a conflict and a refusal are different decisions, and one
    -- column holding either would leave which-kind-of-permanent a thing to infer.
    refused_code    TEXT,
    -- The success reply gets the same protection the proposal echo has, for the same reason: a
    -- crash between sending it and recording that must not send it again.
    reply_echo      TEXT NOT NULL DEFAULT 'pending',
    reply_chunks    INTEGER
);
CREATE INDEX IF NOT EXISTS proposals_actor ON proposals(actor_key);
";

/// What `ironworks-store` needs to know about this file: its schema, where the version is
/// recorded, and which version this implementation writes.
const SCHEMA: ironworks_store::Schema = ironworks_store::Schema {
    sql: SCHEMA_SQL,
    meta_table: "proposal_meta",
    version: SCHEMA_VERSION,
};

#[derive(Debug, thiserror::Error)]
pub enum ProposalError {
    #[error("the proposal snapshot at {path} could not be created: {source}")]
    Snapshot {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the proposal store at {path} could not be opened: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "the proposal store at {path} is not a readable database. Refusing to run — and do NOT \
         delete it blind: it holds words someone typed and is waiting to confirm. Move it aside \
         and keep it."
    )]
    NotADatabase { path: String },
    #[error(
        "the proposal store at {path} was created but could not be restricted to its owner: \
         {source}. Refusing to run: it holds what people wrote before they decided to share it, \
         and continuing would leave that readable while reporting success."
    )]
    Exposed {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the proposal store is schema version {found}, this implementation writes {expected}. \
         Refusing to run: an unknown version means a downgrade or an unfinished write, and \
         guessing which would risk someone's unconfirmed words."
    )]
    Version { found: i64, expected: i64 },
    #[error(
        "the proposal store's recorded schema version is {raw:?}, which is not a number. Refusing to \
         read it as 'no version recorded': that arm stamps this implementation's version onto a \
         database whose shape was never established, and this file holds words someone typed and is waiting to confirm."
    )]
    UnreadableVersion { raw: String },
    /// The same update arriving with different content. Nothing was changed.
    ///
    /// This is not a redelivery: a redelivery of update *N* carries what update *N* carried. A
    /// differing one means the key no longer identifies the request, and overwriting would
    /// replace words a person may already have been shown.
    #[error(
        "update {update} already holds a different proposal ({field} differs) — a redelivery \
         carries what it carried the first time, so nothing was changed"
    )]
    Conflict { update: u64, field: &'static str },
    #[error("no proposal is stored for update {update}")]
    NoSuchProposal { update: u64 },
    /// A second echo attempt on a proposal the channel already confirmed delivering.
    ///
    /// Refused rather than ignored: reaching here means a caller was about to put a second copy
    /// of the same proposal in someone's room.
    #[error("update {update} was already echoed; a second copy is not sent automatically")]
    AlreadyEchoed { update: u64 },
    /// A confirmation on a proposal that is already appending, committed or contested.
    ///
    /// Refused rather than ignored: reaching here means a caller was about to write a second
    /// copy of confirmed evidence, or to overwrite a conflict somebody has to look at.
    #[error(
        "update {update} has already been confirmed; a second append is never made \
         automatically"
    )]
    AlreadyConfirmed { update: u64 },
    #[error("the proposal store holds an unusable {what} for update {update}")]
    Unusable { update: u64, what: &'static str },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// What is known about showing a proposal to the person who made it.
///
/// The same three-valued shape as [`AttemptOutcome`], and for the same reason: "we could not
/// tell" is a state, not a missing boolean, and collapsing it into either neighbour is what puts
/// a second copy in someone's room or confirms words they never saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// Minted, and nothing has reached the channel. **The only state that may be attempted.**
    ///
    /// A send the channel *proved* never left returns here: nothing was delivered, the person has
    /// seen nothing, and a retry cannot duplicate.
    Pending,
    /// A send was begun and its outcome never became durable.
    ///
    /// The state that closes the duplicate window. It is written *before* the send, so a crash
    /// anywhere between the intent and the recorded outcome leaves this rather than `Pending` —
    /// and this never resends. It is deliberately ambiguous: the message may be in the room or
    /// may not, and only a person looking can tell.
    Attempting,
    /// The channel confirmed every chunk. The person has seen the exact bytes. Never resent.
    Delivered,
    /// The channel could not say. Never resent, and an operator's to settle.
    Uncertain { acknowledged_chunks: u32 },
}

impl Echo {
    fn tag(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Attempting => "attempting",
            Self::Delivered => "delivered",
            Self::Uncertain { .. } => "uncertain",
        }
    }

    /// Whether a send may be begun.
    ///
    /// The whole state machine in one predicate, and the safe direction is the default: exactly
    /// one state says yes, so a state this build cannot interpret, a state added later, and every
    /// ambiguous state all answer no.
    pub fn may_attempt(self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Whether an operator has to look at this one.
    ///
    /// `Attempting` counts. It means a send was begun and nothing durable says what happened, so
    /// it is exactly as unresolved as `Uncertain` — and treating it as anything else is what
    /// would license the automatic resend this state exists to prevent.
    pub fn needs_operator(self) -> bool {
        matches!(self, Self::Attempting | Self::Uncertain { .. })
    }
}

/// How far a proposal has got towards becoming shared evidence.
///
/// Four states, and only one of them writes. The shape is the echo's, one layer up: an intent
/// that is durable *before* the effect, so the window between "it happened" and "we know it
/// happened" resolves to something that never repeats the effect by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    /// Nobody has confirmed it. The only state a confirmation may begin from.
    Unconfirmed,
    /// An append was begun and no outcome became durable.
    ///
    /// Carries the whole immutable request, so the retry is provably the same bytes — which is
    /// what makes it a replay at an idempotent endpoint rather than a second write.
    Appending { activity_id: String, occurred_at: String, confirm_update: u64 },
    /// The store holds it. **Terminal**, and the reply may be sent.
    Committed { activity_id: String, org: OrgId, confirm_update: u64 },
    /// The store refused: that id exists carrying different content.
    ///
    /// **Terminal and an operator's.** Nothing here retries, because a conflict is a decision
    /// about the world rather than a transport failure, and the one repair — overwriting — is
    /// the thing immutable evidence forbids.
    Contested { activity_id: String, differing_field: String, confirm_update: u64 },
    /// The store decided against the request itself: unauthorized, no such account, a shape it
    /// will not accept.
    ///
    /// # Why this is a state and not an absence
    ///
    /// It used to be [`Appending`](Self::Appending). That variant means *a request was begun and
    /// no outcome became durable*, which licenses the one thing it must license: replaying the
    /// byte-identical request. A permanent refusal is the opposite fact — an outcome that became
    /// perfectly well known and was simply not recorded — and leaving it in `Appending` made every
    /// later `/confirm` re-send a request the store had already refused, receive the same refusal,
    /// and say so again. Harmless, idempotent, and endless.
    ///
    /// So `Appending` now means what it says, and this carries the decision. **Terminal**, like
    /// [`Contested`](Self::Contested) and for the same reason: repeating a decided request either
    /// changes nothing or changes something that must not change. The repair is an operator's —
    /// re-point a credential, create the account — and then a fresh `/share`, because the words
    /// were never shared and the proposal is still exactly what she wrote.
    Refused { activity_id: String, code: String, confirm_update: u64 },
}

/// The stored spelling of each confirmation state.
///
/// Named constants because a **writer needs the tag without having a value** to ask for it. The
/// alternative was in the tree: `record_commit` built a `Confirmation::Committed` out of empty
/// strings and a zero, purely to call `tag()` on it and throw it away — which reads at the call
/// site as though those empty fields were about to be stored. One spelling each, and `tag()` is
/// still the only thing that maps a value to one.
const UNCONFIRMED: &str = "unconfirmed";
const APPENDING: &str = "appending";
const COMMITTED: &str = "committed";
const CONTESTED: &str = "contested";
const REFUSED: &str = "refused";

impl Confirmation {
    /// The stored spelling of this state — the one mapping from a value to its tag.
    ///
    /// Public because it is what an operator-facing caller shows, and exhaustive because that is
    /// the whole reason it survives now that the writers name their tag directly: a variant added
    /// later fails to compile here rather than reaching SQLite with no spelling at all.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Unconfirmed => UNCONFIRMED,
            Self::Appending { .. } => APPENDING,
            Self::Committed { .. } => COMMITTED,
            Self::Contested { .. } => CONTESTED,
            Self::Refused { .. } => REFUSED,
        }
    }

    /// Whether a confirmation may be started. Exactly one state says yes.
    pub fn may_begin(&self) -> bool {
        matches!(self, Self::Unconfirmed)
    }

    /// Whether an operator has to look at this one.
    ///
    /// `Appending` counts for the same reason [`Echo::Attempting`] does: a request was begun and
    /// nothing durable says what happened. `Contested` and `Refused` count because they are
    /// permanent decisions the store made, and nothing here will retry either.
    pub fn needs_operator(&self) -> bool {
        matches!(self, Self::Appending { .. } | Self::Contested { .. } | Self::Refused { .. })
    }
}

/// One stored proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub update: u64,
    pub channel: ChannelRef,
    pub contributor: ContributorId,
    pub slug: Slug,
    pub org: OrgId,
    pub account: AccountId,
    /// The bytes the person typed, unchanged.
    pub body: String,
    pub proposed_at: String,
    pub echo: Echo,
    /// How far it has got towards being shared. The proposal is retained whatever this says —
    /// it is the evidence of what was confirmed, and deleting it after an append would leave the
    /// activity with no record of the act that produced it.
    pub confirm: Confirmation,
    /// Delivery of the success reply, once there is one to send.
    pub reply: Echo,
}

/// A proposal that is provably this person's, in this conversation, for this organization, and
/// provably shown to them.
///
/// **No public constructor.** The only way to one is [`ProposalStore::confirmable`], so a
/// confirmation path cannot be handed a proposal the channel never delivered, one belonging to
/// someone else, or one made under authority that has since moved — not because it checks, but
/// because there is no value it could check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmable(Proposal);

impl Confirmable {
    pub fn proposal(&self) -> &Proposal {
        &self.0
    }
}

/// The authority a confirmation is offered under: everything the relay has re-verified *now*,
/// from the registry and the record store, for the message that asked to confirm.
///
/// A stored proposal is only confirmable when every one of these still matches it. A registry
/// edit that moved a conversation to another tenant, or a credential that now resolves to a
/// different organization, therefore makes yesterday's proposal unreachable rather than
/// promotable into somewhere it was never meant for.
#[derive(Debug, Clone, Copy)]
pub struct ConfirmAuthority<'a> {
    pub channel: &'a ChannelRef,
    pub actor: &'a ActorRef,
    pub slug: &'a Slug,
    pub org: &'a OrgId,
    pub contributor: &'a ContributorId,
}

/// What a new proposal is made of. Every field is immutable once stored.
#[derive(Debug, Clone)]
pub struct NewProposal<'a> {
    pub update: u64,
    pub channel: &'a ChannelRef,
    pub actor: &'a ActorRef,
    pub contributor: &'a ContributorId,
    pub slug: &'a Slug,
    pub org: &'a OrgId,
    pub account: &'a AccountId,
    pub body: &'a str,
}

pub struct ProposalStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for ProposalStore {
    /// Names the store and nothing in it: every row holds someone's unshared words.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProposalStore").finish_non_exhaustive()
    }
}

/// An open failure, as the sentence an operator has to act on.
///
/// A store that cannot be read is either one that is not there and could not be made, or one whose
/// bytes are not a database. The second needs a different sentence: that file may hold state
/// nothing else has, so the operator instruction is to preserve it rather than clear it and retry.
///
/// `ironworks-store` runs the protocol and returns a cause carrying no path and no prose, because
/// the three crates sharing it keep three different files and the wording is the part that says
/// which one this is. This translation is the only thing about opening a proposal store that is not shared.
fn opened(path: &Path, why: ironworks_store::OpenFailure) -> ProposalError {
    use ironworks_store::OpenFailure as F;
    let path = path.display().to_string();
    match why {
        F::Open(source) => ProposalError::Open { path, source },
        F::NotADatabase => ProposalError::NotADatabase { path },
        F::Exposed(source) => ProposalError::Exposed { path, source },
        F::UnreadableVersion { raw } => ProposalError::UnreadableVersion { raw },
        F::Version { found } => ProposalError::Version { found, expected: SCHEMA_VERSION },
        F::Sqlite(source) => ProposalError::Sqlite(source),
    }
}

impl ProposalStore {
    /// The connection, whatever a previous panic left behind.
    ///
    /// `lock().expect("proposal store lock")` opened 11 methods, and a poisoned mutex made every one of them
    /// abort — inside a store whose error enum exists to fail closed and whose whole subject is
    /// words someone typed and is waiting to confirm. Poisoning means *some other* thread panicked while holding this; the
    /// data behind it is a SQLite connection, which has its own transactional integrity and is not
    /// left half-written by a panic in Rust code. Taking the guard is therefore the honest
    /// recovery, and it is one documented decision rather than 11 undocumented ones.
    fn locked(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Copy a consistent live view into a new owner-only SQLite file.
    pub fn snapshot_to(&self, destination: impl AsRef<Path>) -> Result<(), ProposalError> {
        let destination = destination.as_ref();
        let source = self.locked();
        ironworks_store::snapshot_to(&source, destination).map_err(|why| match why {
            ironworks_store::SnapshotFailure::Reserve(source) => {
                ProposalError::Snapshot { path: destination.display().to_string(), source }
            }
            ironworks_store::SnapshotFailure::Sqlite(source) => ProposalError::Sqlite(source),
        })
    }

    /// Open or create the store, owner-only.
    ///
    /// The same posture as the journals, and it fails closed for the same reason: the file holds
    /// what people wrote before deciding to share it, and a mode this cannot restrict is one it
    /// must not fill.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProposalError> {
        let path = path.as_ref();
        let conn = ironworks_store::open_durable(path, SCHEMA, |tx, found| {
            if found != 2 {
                return Ok(false);
            }
            // Additive and touching no row. `CREATE TABLE IF NOT EXISTS` no-ops on an existing
            // file, so the column has to be added here — inside the transaction `open_durable`
            // owns, the one that also writes the new version. An interrupted migration therefore
            // rolls back with the version still 2, and the next open re-runs it.
            tx.execute_batch("ALTER TABLE proposals ADD COLUMN refused_code TEXT")?;
            Ok(true)
        })
        .map_err(|why| opened(path, why))?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Record a proposal, or return the one this update already made.
    ///
    /// Idempotent by update id: a redelivery finds its own row and changes nothing, so a channel
    /// that resends until acknowledged cannot mint a second proposal. A redelivery carrying
    /// *different* content is a [`ProposalError::Conflict`] and writes nothing — the key would no
    /// longer identify one request, and the stored words may already have been shown.
    pub fn propose(&self, new: NewProposal<'_>, now: &Utc) -> Result<Proposal, ProposalError> {
        let conn = self.locked();
        if let Some(existing) = Self::read(&conn, new.update)? {
            for (field, same) in [
                ("actor", Self::actor_key(&conn, new.update)? == new.actor.persistence_key()),
                ("conversation", &existing.channel == new.channel),
                ("contributor", &existing.contributor == new.contributor),
                ("tenant", &existing.slug == new.slug),
                ("organization", &existing.org == new.org),
                ("account", &existing.account == new.account),
                ("note", existing.body == new.body),
            ] {
                if !same {
                    return Err(ProposalError::Conflict { update: new.update, field });
                }
            }
            return Ok(existing);
        }
        conn.execute(
            "INSERT INTO proposals(update_id,channel,actor_key,contributor,slug,org,account,body,
                                   proposed_at,echo)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending')",
            params![
                sql_update(new.update)?,
                new.channel.as_str(),
                new.actor.persistence_key(),
                new.contributor.as_str(),
                new.slug.as_str(),
                new.org.as_str(),
                new.account.as_str(),
                new.body,
                now.as_str(),
            ],
        )?;
        Self::read(&conn, new.update)?.ok_or(ProposalError::NoSuchProposal { update: new.update })
    }

    /// Make the intent to send durable, **before** a byte leaves.
    ///
    /// # The window this closes
    ///
    /// Without it the order is send-then-record, and a crash in between is indistinguishable
    /// from a send that never happened. The channel redelivers the update, the proposal is found
    /// still `Pending`, and a second copy of the same words goes into the room automatically —
    /// the one outcome this whole path exists to prevent.
    ///
    /// With it the crash leaves [`Echo::Attempting`], which never sends. The cost is conservative
    /// false ambiguity: a proposal whose send failed to leave at all still needs a person to
    /// look. That is the correct direction, and it is why [`Echo::Attempting`] is
    /// [`Echo::needs_operator`].
    ///
    /// Refuses anything that is not [`Echo::Pending`], so the caller cannot begin a second
    /// attempt on a proposal already in flight, already delivered, or already uncertain.
    pub fn begin_echo(&self, update: u64) -> Result<(), ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        if !existing.echo.may_attempt() {
            return Err(ProposalError::AlreadyEchoed { update });
        }
        took(conn.execute(
            "UPDATE proposals SET echo='attempting' WHERE update_id=?1 AND echo=?2",
            params![sql_update(update)?, existing.echo.tag()],
        )?)
        .ok_or(ProposalError::AlreadyEchoed { update })
    }

    /// Record what the channel established about showing the proposal.
    ///
    /// [`AttemptOutcome::Rejected`] — the channel's positive claim that nothing was delivered —
    /// returns the proposal to [`Echo::Pending`], because the person has seen nothing and a retry
    /// cannot duplicate. Everything else settles.
    ///
    /// Only an attempt that was begun may be recorded: reaching here from any other state means a
    /// send happened without a durable intent, which is the window [`Self::begin_echo`] exists to
    /// close.
    pub fn record_echo(
        &self,
        update: u64,
        outcome: AttemptOutcome,
        now: &Utc,
    ) -> Result<Echo, ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        if existing.echo != Echo::Attempting {
            return Err(ProposalError::AlreadyEchoed { update });
        }
        let echo = match outcome {
            AttemptOutcome::Complete => Echo::Delivered,
            AttemptOutcome::Rejected => Echo::Pending,
            AttemptOutcome::Ambiguous { acknowledged_chunks } => {
                Echo::Uncertain { acknowledged_chunks }
            }
        };
        let chunks = match echo {
            Echo::Uncertain { acknowledged_chunks } => Some(acknowledged_chunks),
            _ => None,
        };
        // `echoed_at` marks when the attempt settled, and a pending row keeps `NULL`: a timestamp
        // on a proposal nobody has seen would read as evidence that they had.
        let at = if echo == Echo::Pending { None } else { Some(now.as_str()) };
        took(conn.execute(
            "UPDATE proposals SET echo=?2, echo_chunks=?3, echoed_at=?4
              WHERE update_id=?1 AND echo=?5",
            params![sql_update(update)?, echo.tag(), chunks, at, existing.echo.tag()],
        )?)
        .ok_or(ProposalError::AlreadyEchoed { update })?;
        Ok(echo)
    }

    /// The proposal this update made, whatever state it is in. For operators and for redelivery.
    pub fn proposal(&self, update: u64) -> Result<Option<Proposal>, ProposalError> {
        let conn = self.locked();
        Self::read(&conn, update)
    }

    /// Make the intent to append durable, **before** the request leaves.
    ///
    /// The append's half of the rule the echo already obeys. Without it a crash between an
    /// accepted POST and a recorded outcome is indistinguishable from a POST that never
    /// happened, and the retry would be a second write rather than a replay. With it the crash
    /// leaves [`Confirmation::Appending`] carrying the exact immutable request, so the retry is
    /// provably the same bytes — which the store's own idempotency then turns into a replay.
    ///
    /// Refuses anything but [`Confirmation::Unconfirmed`], so a second `/confirm` cannot begin a
    /// second append on a proposal already in flight, already committed, or already contested.
    pub fn begin_append(
        &self,
        update: u64,
        activity_id: &str,
        occurred_at: &str,
        confirm_update: u64,
    ) -> Result<(), ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        if !existing.confirm.may_begin() {
            return Err(ProposalError::AlreadyConfirmed { update });
        }
        took(conn.execute(
            "UPDATE proposals
                SET confirm=?5, activity_id=?2, occurred_at=?3, confirm_update=?4
              WHERE update_id=?1 AND confirm=?6",
            params![
                sql_update(update)?,
                activity_id,
                occurred_at,
                sql_update(confirm_update)?,
                APPENDING,
                existing.confirm.tag(),
            ],
        )?)
        .ok_or(ProposalError::AlreadyConfirmed { update })
    }

    /// The store holds it. Persisted **before** the success reply is sent.
    ///
    /// `org` is what the store answered with, not what was asked — a committed row records what
    /// actually happened, and a crash after this still knows exactly what to report.
    pub fn record_commit(
        &self,
        update: u64,
        activity_id: &str,
        org: &OrgId,
    ) -> Result<(), ProposalError> {
        self.settle_append(update, COMMITTED, activity_id, Some(org.as_str()), None, None)
    }

    /// The store refused: that id exists carrying different content. Permanent.
    pub fn record_contest(
        &self,
        update: u64,
        activity_id: &str,
        differing_field: &str,
    ) -> Result<(), ProposalError> {
        self.settle_append(update, CONTESTED, activity_id, None, Some(differing_field), None)
    }

    /// The store decided against the request itself. Permanent, and never replayed.
    ///
    /// `code` is the store's own word for what it refused, kept so an operator reading
    /// [`Self::awaiting_operator`] can tell an unauthorized credential from a missing account
    /// without going back to a log that may have rotated.
    pub fn record_refusal(
        &self,
        update: u64,
        activity_id: &str,
        code: &str,
    ) -> Result<(), ProposalError> {
        self.settle_append(update, REFUSED, activity_id, None, None, Some(code))
    }

    fn settle_append(
        &self,
        update: u64,
        state: &str,
        activity_id: &str,
        org: Option<&str>,
        differing: Option<&str>,
        refused: Option<&str>,
    ) -> Result<(), ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        // Only an attempt that was begun may settle. Reaching here from any other state means an
        // append happened without a durable intent, which is the window `begin_append` closes.
        let Confirmation::Appending { activity_id: begun, .. } = &existing.confirm else {
            return Err(ProposalError::AlreadyConfirmed { update });
        };
        if begun != activity_id {
            // The outcome names a different activity than the intent did. Something is wrong
            // upstream and writing either would record a fiction.
            return Err(ProposalError::Unusable { update, what: "settled activity id" });
        }
        took(conn.execute(
            "UPDATE proposals SET confirm=?2, committed_org=?3, contested_field=?4, refused_code=?5
              WHERE update_id=?1 AND confirm=?6",
            params![sql_update(update)?, state, org, differing, refused, existing.confirm.tag()],
        )?)
        .ok_or(ProposalError::AlreadyConfirmed { update })
    }

    /// Durable intent for the success reply, before it is sent. Mirrors [`Self::begin_echo`].
    pub fn begin_reply(&self, update: u64) -> Result<(), ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        if !existing.reply.may_attempt() {
            return Err(ProposalError::AlreadyEchoed { update });
        }
        took(conn.execute(
            "UPDATE proposals SET reply_echo='attempting' WHERE update_id=?1 AND reply_echo=?2",
            params![sql_update(update)?, existing.reply.tag()],
        )?)
        .ok_or(ProposalError::AlreadyEchoed { update })
    }

    /// What the channel established about the success reply. Mirrors [`Self::record_echo`].
    pub fn record_reply(
        &self,
        update: u64,
        outcome: AttemptOutcome,
    ) -> Result<Echo, ProposalError> {
        let conn = self.locked();
        let existing =
            Self::read(&conn, update)?.ok_or(ProposalError::NoSuchProposal { update })?;
        if existing.reply != Echo::Attempting {
            return Err(ProposalError::AlreadyEchoed { update });
        }
        let echo = match outcome {
            AttemptOutcome::Complete => Echo::Delivered,
            AttemptOutcome::Rejected => Echo::Pending,
            AttemptOutcome::Ambiguous { acknowledged_chunks } => {
                Echo::Uncertain { acknowledged_chunks }
            }
        };
        let chunks = match echo {
            Echo::Uncertain { acknowledged_chunks } => Some(acknowledged_chunks),
            _ => None,
        };
        took(conn.execute(
            "UPDATE proposals SET reply_echo=?2, reply_chunks=?3
              WHERE update_id=?1 AND reply_echo=?4",
            params![sql_update(update)?, echo.tag(), chunks, existing.reply.tag()],
        )?)
        .ok_or(ProposalError::AlreadyEchoed { update })?;
        Ok(echo)
    }

    /// The proposal this person may confirm, if their latest one is confirmable.
    ///
    /// # Newest first, then judged — never searched for a confirmable one
    ///
    /// The echo tells people that another `/share` replaces the proposal, so the newest is the
    /// only one they could mean. Selecting the newest **delivered** row instead would quietly
    /// keep an older proposal alive behind a newer one that is pending or uncertain, and confirm
    /// words the person believes they already superseded. So this finds the newest row whatever
    /// state it is in, and returns `None` unless *that* one is confirmable.
    ///
    /// # Authority is re-verified, not remembered
    ///
    /// Every axis in `authority` is what the relay established for the message asking to confirm
    /// — from the registry and the record store, now. A stored proposal is reachable only when it
    /// agrees on all of them, so a registry edit that moved a conversation to another tenant, or
    /// a credential that now resolves elsewhere, makes an old proposal unreachable rather than
    /// promotable into an organization it was never meant for.
    ///
    /// Together with [`Confirmable`] having no public constructor, this is the whole of what a
    /// later confirmation path is allowed to act on.
    pub fn confirmable(
        &self,
        authority: ConfirmAuthority<'_>,
    ) -> Result<Option<Confirmable>, ProposalError> {
        let conn = self.locked();
        let newest: Option<i64> = conn
            .query_row(
                "SELECT update_id FROM proposals
                 WHERE actor_key=?1 AND channel=?2
                 ORDER BY update_id DESC LIMIT 1",
                params![authority.actor.persistence_key(), authority.channel.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(newest) = newest else { return Ok(None) };
        let newest = u64::try_from(newest)
            .map_err(|_| ProposalError::Unusable { update: 0, what: "update id" })?;
        let Some(proposal) = Self::read(&conn, newest)? else { return Ok(None) };
        if proposal.echo != Echo::Delivered {
            return Ok(None);
        }
        if &proposal.channel != authority.channel
            || &proposal.slug != authority.slug
            || &proposal.org != authority.org
            || &proposal.contributor != authority.contributor
        {
            return Ok(None);
        }
        Ok(Some(Confirmable(proposal)))
    }

    /// Every proposal an operator has to settle: shown-or-not-shown could not be established.
    ///
    /// Every lane, not just the echo. A send that was begun and whose outcome never landed is as
    /// unresolved as one the channel could not confirm; an append in the same position is worse,
    /// because it may already be shared evidence; and a contested append is permanent. Leaving
    /// any of them out would hide exactly the cases the durable intents were added to surface.
    pub fn awaiting_operator(&self) -> Result<Vec<Proposal>, ProposalError> {
        let conn = self.locked();
        let mut stmt = conn.prepare(
            "SELECT update_id FROM proposals
              WHERE echo IN ('attempting','uncertain')
                 OR confirm IN ('appending','contested','refused')
                 OR reply_echo IN ('attempting','uncertain')
              ORDER BY update_id",
        )?;
        let ids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let id = u64::try_from(id)
                .map_err(|_| ProposalError::Unusable { update: 0, what: "update id" })?;
            if let Some(p) = Self::read(&conn, id)? {
                out.push(p);
            }
        }
        Ok(out)
    }

    fn actor_key(conn: &Connection, update: u64) -> Result<String, ProposalError> {
        conn.query_row(
            "SELECT actor_key FROM proposals WHERE update_id=?1",
            params![sql_update(update)?],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(ProposalError::NoSuchProposal { update })
    }

    fn read(conn: &Connection, update: u64) -> Result<Option<Proposal>, ProposalError> {
        /// One `proposals` row as SQLite hands it back, before it becomes a `Proposal`.
        type Row = (
            (String, String, String, String, String, String, String),
            (String, Option<i64>),
            (
                String,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
            (String, Option<i64>),
        );
        let row: Option<Row> = conn
            .query_row(
                "SELECT channel,contributor,slug,org,account,body,proposed_at,
                        echo,echo_chunks,
                        confirm,confirm_update,activity_id,occurred_at,committed_org,
                        contested_field,refused_code,
                        reply_echo,reply_chunks
                 FROM proposals WHERE update_id=?1",
                params![sql_update(update)?],
                |r| {
                    Ok((
                        (
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ),
                        (r.get(7)?, r.get(8)?),
                        (
                            r.get(9)?,
                            r.get(10)?,
                            r.get(11)?,
                            r.get(12)?,
                            r.get(13)?,
                            r.get(14)?,
                            r.get(15)?,
                        ),
                        (r.get(16)?, r.get(17)?),
                    ))
                },
            )
            .optional()?;
        let Some((
            (channel, contributor, slug, org, account, body, proposed_at),
            (echo, chunks),
            (
                confirm,
                confirm_update,
                activity_id,
                occurred_at,
                committed_org,
                contested_field,
                refused_code,
            ),
            (reply_echo, reply_chunks),
        )) = row
        else {
            return Ok(None);
        };
        let bad = |what: &'static str| ProposalError::Unusable { update, what };
        // A row this build cannot read is refused, never coerced. `pending` is the permissive
        // state, and defaulting an unknown tag to it would offer a retry on a proposal that may
        // already be in someone's room.
        let echo = match (echo.as_str(), chunks) {
            ("pending", None) => Echo::Pending,
            ("attempting", None) => Echo::Attempting,
            ("delivered", None) => Echo::Delivered,
            ("uncertain", Some(n)) => Echo::Uncertain {
                acknowledged_chunks: u32::try_from(n).map_err(|_| bad("chunk count"))?,
            },
            _ => return Err(bad("echo state")),
        };
        let confirm_update = || -> Result<u64, ProposalError> {
            confirm_update
                .and_then(|v| u64::try_from(v).ok())
                .ok_or(ProposalError::Unusable { update, what: "confirmation update id" })
        };
        // Same rule as the echo: a row this build cannot read is refused, never coerced.
        // `unconfirmed` is the permissive state, and defaulting an unknown tag to it would offer
        // to append words that may already be shared.
        let confirm = match confirm.as_str() {
            UNCONFIRMED => Confirmation::Unconfirmed,
            APPENDING => Confirmation::Appending {
                activity_id: activity_id.clone().ok_or(bad("activity id"))?,
                occurred_at: occurred_at.ok_or(bad("occurrence date"))?,
                confirm_update: confirm_update()?,
            },
            COMMITTED => Confirmation::Committed {
                activity_id: activity_id.clone().ok_or(bad("activity id"))?,
                org: OrgId::parse(committed_org.ok_or(bad("committed organization"))?)
                    .map_err(|_| bad("committed organization"))?,
                confirm_update: confirm_update()?,
            },
            CONTESTED => Confirmation::Contested {
                activity_id: activity_id.clone().ok_or(bad("activity id"))?,
                differing_field: contested_field.ok_or(bad("contested field"))?,
                confirm_update: confirm_update()?,
            },
            REFUSED => Confirmation::Refused {
                activity_id: activity_id.clone().ok_or(bad("activity id"))?,
                code: refused_code.ok_or(bad("refusal code"))?,
                confirm_update: confirm_update()?,
            },
            _ => return Err(bad("confirmation state")),
        };
        let reply = match (reply_echo.as_str(), reply_chunks) {
            ("pending", None) => Echo::Pending,
            ("attempting", None) => Echo::Attempting,
            ("delivered", None) => Echo::Delivered,
            ("uncertain", Some(n)) => Echo::Uncertain {
                acknowledged_chunks: u32::try_from(n).map_err(|_| bad("chunk count"))?,
            },
            _ => return Err(bad("reply state")),
        };
        Ok(Some(Proposal {
            update,
            channel: ChannelRef::new(channel),
            contributor: ContributorId::parse(contributor).map_err(|_| bad("contributor"))?,
            slug: Slug::parse(slug).map_err(|_| bad("tenant slug"))?,
            org: OrgId::parse(org).map_err(|_| bad("organization"))?,
            account: AccountId::parse(account).map_err(|_| bad("account"))?,
            body,
            proposed_at,
            echo,
            confirm,
            reply,
        }))
    }
}

/// SQLite has no unsigned integer. An update id past `i64::MAX` is not one any channel issued.
fn sql_update(update: u64) -> Result<i64, ProposalError> {
    i64::try_from(update).map_err(|_| ProposalError::Unusable { update, what: "update id" })
}

/// `Some(())` when a compare-and-swap actually moved the row, `None` when it matched nothing.
///
/// # Why every transition carries its observed state in the `WHERE`
///
/// Each one reads the row, judges it in Rust, and then writes. The `Mutex<Connection>` orders that
/// pair within one process and says nothing about two — and `relay` is documented as one cycle per
/// process, with other subcommands opening the same file. Without the guard both processes read
/// `pending`, both pass the judgement, and both write `attempting`; each then believes it holds the
/// durable intent and sends, which is "the one outcome this whole path exists to prevent".
///
/// **Measured, not reasoned about.** Two handles on one file, released from a barrier, both
/// acquired the echo intent on the first round of a two-hundred-round probe. This is not a narrow
/// window.
///
/// The guard is the state that was *observed*, passed back as a parameter — never a predicate
/// rebuilt in SQL. Re-deriving `may_attempt` or `may_begin` in a `WHERE` clause would be a second
/// copy of the state machine, and the tags are already spelled in enough places.
fn took(rows: usize) -> Option<()> {
    (rows > 0).then_some(())
}

#[cfg(test)]
mod tests;
