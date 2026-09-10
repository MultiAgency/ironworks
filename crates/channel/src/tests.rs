use super::*;
use std::collections::HashMap;

/// Ordered instants, in the one form the timestamp columns hold.
///
/// These were the string literals `"t0"`, `"t1"`, `"t2"` — placeholders that are not
/// timestamps
/// and would sort arbitrarily against a real one. Nothing caught that while the parameter was
/// `&str`.
fn at(second: u32) -> ironworks_domain::Utc {
    ironworks_domain::Utc::parse(format!("2026-08-27T12:00:{second:02}+00:00"))
        .expect("the fixture instants are the canonical UTC form")
}

/// A stand-in tenant journal. It can answer exactly what the real one can answer, because the
/// trait only permits one question — which is the property under test as much as a convenience.
#[derive(Default)]
struct Progress(HashMap<String, Openness>);

impl Progress {
    fn open(&mut self, key: &str) -> IdempotencyKey {
        self.set(key, Openness::Open)
    }
    fn settled(&mut self, key: &str) -> IdempotencyKey {
        self.set(key, Openness::Settled)
    }
    /// The journal was asked and did not answer. Note that this is a *stored* state here only
    /// because a stub has no other way to fail; in the real journal it is a read that errored.
    fn unreadable(&mut self, key: &str) -> IdempotencyKey {
        self.set(key, Openness::Unestablished)
    }
    fn set(&mut self, key: &str, o: Openness) -> IdempotencyKey {
        self.0.insert(key.to_owned(), o);
        IdempotencyKey::parse(key).expect("test key")
    }
}

impl TurnProgress for Progress {
    fn openness(&self, key: &IdempotencyKey) -> Openness {
        self.0.get(key.as_str()).copied().unwrap_or(Openness::Settled)
    }
}

fn journal() -> (tempdir::Dir, ChannelJournal) {
    let dir = tempdir::Dir::new();
    let j = ChannelJournal::open(dir.path().join("channel.db")).expect("open");
    (dir, j)
}

fn chan(s: &str) -> ChannelRef {
    ChannelRef::new(s)
}

fn u(n: u64) -> UpdateId {
    UpdateId::new(n)
}

// ── the batch barrier ───────────────────────────────────────────────────────────────────────

#[test]
fn a_journalled_batch_holds_the_cursor_before_any_of_it_is_worked_on() {
    let (_d, j) = journal();
    let p = Progress::default();
    j.note_batch(&[(u(7), chan("g1")), (u(8), chan("g2")), (u(9), chan("g1"))], &at(0)).unwrap();
    // Nothing has been routed, let alone answered. The offset must be behind all three, which is
    // the entire reason the batch is journalled before a worker starts.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(7)));
}

#[test]
fn a_redelivered_update_keeps_its_original_row() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.settled("k-7");
    j.note_batch(&[(u(7), chan("g1"))], &at(0)).unwrap();
    j.disposition(u(7), &Disposition::Turn(key), &at(1)).unwrap();

    // The channel sends 7 again, because it was never told an offset past it.
    let fresh = j.note_batch(&[(u(7), chan("g1"))], &at(2)).unwrap();
    assert_eq!(fresh, 0, "a redelivery inserted a second row");
    let row = j.record(u(7)).unwrap().unwrap();
    assert_eq!(row.received_at, at(0).as_str(), "a redelivery reset the arrival time");
    assert!(row.disposition.cost_a_turn(), "a redelivery reset the disposition to unclaimed");
}

// ── the one-way disposition (E-4) ───────────────────────────────────────────────────────────

#[test]
fn an_update_that_cost_a_turn_can_never_be_relabelled_as_noise() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.open("k-1");
    j.note_batch(&[(u(1), chan("g1"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::Turn(key), &at(1)).unwrap();

    // The tenant leaves the registry mid-turn. Routing now says "nobody owns this channel" —
    // and that must not be allowed to rewrite the history of an update with a possibly-billed
    // answer behind it.
    let err = j.disposition(u(1), &Disposition::NotAddressed, &at(2)).unwrap_err();
    assert!(matches!(err, ChannelError::AlreadyDispositioned { existing: "turn", .. }), "{err:?}");
    // And it still holds the barrier, because the turn is still open.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(1)));
}

#[test]
fn a_command_this_process_answered_is_settled_and_never_read_as_noise() {
    // `NotAddressed` says nobody acted on this. A command is the opposite — it was addressed to
    // us and we did something — and a disposition is written once, so the wrong one here would
    // be a permanent falsehood about what happened rather than a label to be corrected later.
    //
    // Named for the class of update rather than for a proposal: most commands make none, and the
    // row must not claim one exists.
    let (_d, j) = journal();
    let p = Progress::default();
    j.note_batch(&[(u(1), chan("dm"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::Command, &at(1)).unwrap();

    let record = j.record(u(1)).unwrap().expect("the row survives the round trip");
    assert_eq!(record.disposition, Disposition::Command);
    assert_eq!(record.settled_at.as_deref(), Some(at(1).as_str()));
    // It cost no turn, so it holds nothing back and may be compacted away.
    assert!(!Disposition::Command.cost_a_turn());
    assert_eq!(Disposition::Command.openness(&p), Openness::Settled);
    // And the cursor moves past it: a settled command holds no barrier, so the conversation
    // behind it does not stop while an operator wonders why.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(2)));
}

#[test]
fn a_command_already_answered_is_not_re_dispositioned() {
    // The same one-way rule every other disposition obeys: an update whose reply may already be
    // in someone's room must not be relabelled by a later cycle.
    let (_d, j) = journal();
    j.note_batch(&[(u(1), chan("dm"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::Command, &at(1)).unwrap();
    let err = j.disposition(u(1), &Disposition::NotAddressed, &at(2)).unwrap_err();
    assert!(
        matches!(err, ChannelError::AlreadyDispositioned { existing: "command", .. }),
        "{err:?}"
    );
}

#[test]
fn a_disposition_needs_an_update_to_attach_to() {
    let (_d, j) = journal();
    let err = j.disposition(u(4), &Disposition::NotAddressed, &at(0)).unwrap_err();
    assert!(matches!(err, ChannelError::NoSuchUpdate { id: 4 }), "{err:?}");
}

// ── the cross-tenant barrier, through the durable path ──────────────────────────────────────

#[test]
fn a_faster_tenant_cannot_acknowledge_a_slower_tenants_unfinished_work() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let alpha = p.open("k-alpha");
    let beta = p.settled("k-beta");
    j.note_batch(&[(u(10), chan("alpha")), (u(11), chan("beta"))], &at(0)).unwrap();
    j.disposition(u(10), &Disposition::Turn(alpha), &at(1)).unwrap();
    j.disposition(u(11), &Disposition::Turn(beta), &at(1)).unwrap();

    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(10)), "beta's completion moved the offset");

    // Only when alpha finishes does the offset clear both.
    p.settled("k-alpha");
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(12)));
}

#[test]
fn noise_between_two_tenants_settles_on_arrival_and_costs_nothing() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.open("k-3");
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g")), (u(3), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(2), &Disposition::Unroutable, &at(1)).unwrap();
    j.disposition(u(3), &Disposition::Turn(key), &at(1)).unwrap();
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(3)));
    assert!(!j.record(u(1)).unwrap().unwrap().disposition.cost_a_turn());
}

// ── acknowledgement is not delivery ─────────────────────────────────────────────────────────

#[test]
fn an_offset_may_not_be_acknowledged_past_the_barrier() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.open("k-5");
    j.note_batch(&[(u(5), chan("g")), (u(6), chan("g"))], &at(0)).unwrap();
    j.disposition(u(5), &Disposition::Turn(key), &at(1)).unwrap();
    j.disposition(u(6), &Disposition::NotAddressed, &at(1)).unwrap();

    let err = j.acknowledge(u(7), &p).unwrap_err();
    assert!(matches!(err, ChannelError::AcknowledgementPastTheBarrier { safe: 5, .. }), "{err:?}");
    assert_eq!(j.acknowledged().unwrap(), None, "a refused acknowledgement was recorded anyway");

    j.acknowledge(u(5), &p).unwrap();
    assert_eq!(j.acknowledged().unwrap(), Some(u(5)));
}

#[test]
fn an_acknowledgement_never_goes_backwards() {
    let (_d, j) = journal();
    let p = Progress::default();
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(2), &Disposition::NotAddressed, &at(1)).unwrap();
    j.acknowledge(u(3), &p).unwrap();

    let err = j.acknowledge(u(1), &p).unwrap_err();
    assert!(
        matches!(err, ChannelError::AcknowledgementWouldRegress { existing: 3, attempted: 1 }),
        "{err:?}"
    );
    assert_eq!(j.acknowledged().unwrap(), Some(u(3)));
}

/// The distinction the whole crate turns on. An update can be entirely finished locally and still
/// not be acknowledged, because acknowledgement is something the *channel* is told, on a later
/// poll, and until then it will send the update again.
#[test]
fn finished_is_not_acknowledged() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.settled("k-1");
    j.note_batch(&[(u(1), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::Turn(key), &at(1)).unwrap();

    // Locally done: the safe cursor has moved past it.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(2)));
    // But the channel has not been told, so it is not acknowledged and will be redelivered.
    assert!(!j.is_acknowledged(u(1)).unwrap());

    j.acknowledge(u(2), &p).unwrap();
    assert!(j.is_acknowledged(u(1)).unwrap());
}

// ── compaction ──────────────────────────────────────────────────────────────────────────────

#[test]
fn nothing_is_compacted_before_the_channel_has_acknowledged_anything() {
    let (_d, j) = journal();
    let p = Progress::default();
    j.note_batch(&[(u(1), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    // The safe cursor is 2, but the channel has been told nothing — so update 1 is still
    // redeliverable, and forgetting it is how it gets handled a second time.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(2)));
    assert_eq!(j.compact().unwrap(), 0, "compacted below an offset the channel never received");
}

#[test]
fn compaction_forgets_noise_and_keeps_everything_that_cost_a_turn() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.settled("k-2");
    let batch: Vec<_> = (1..=5).map(|n| (u(n), chan("g"))).collect();
    j.note_batch(&batch, &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(2), &Disposition::Turn(key), &at(1)).unwrap();
    j.disposition(u(3), &Disposition::Unroutable, &at(1)).unwrap();
    j.disposition(u(4), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(5), &Disposition::NotAddressed, &at(1)).unwrap();
    j.acknowledge(u(6), &p).unwrap();

    assert_eq!(j.compact().unwrap(), 4);
    let left: Vec<u64> = j.records().unwrap().iter().map(|r| r.id.get()).collect();
    assert_eq!(left, vec![2], "an update that cost a model turn was forgotten");
}

/// Compaction must not move the cursor. Deleting the rows below the acknowledged offset removes
/// the `max` the "nothing outstanding" branch reads, so a naive derivation would answer with a
/// *lower* offset afterwards and redeliver everything.
#[test]
fn compaction_does_not_move_the_cursor_backwards() {
    let (_d, j) = journal();
    let p = Progress::default();
    let batch: Vec<_> = (1..=4).map(|n| (u(n), chan("g"))).collect();
    j.note_batch(&batch, &at(0)).unwrap();
    for n in 1..=4 {
        j.disposition(u(n), &Disposition::NotAddressed, &at(1)).unwrap();
    }
    j.acknowledge(u(5), &p).unwrap();
    let before = j.safe_cursor(&p).unwrap();
    assert_eq!(before, Some(u(5)));

    assert_eq!(j.compact().unwrap(), 4);
    // Every row is gone, so the derivation has nothing to answer from — and `None` is the honest
    // answer, not a lower offset. The durable acknowledgement is what carries the position now,
    // which is exactly why it is stored rather than derived.
    assert_eq!(j.safe_cursor(&p).unwrap(), None);
    assert_eq!(j.acknowledged().unwrap(), Some(u(5)));
}

// ── failing closed ──────────────────────────────────────────────────────────────────────────

/// The empty case first, because it is the one that reads as clean by accident: a journal that
/// has never seen anything must poll from the beginning, and `0` here is a measurement rather
/// than a fallback.
#[test]
fn a_journal_that_has_never_seen_anything_polls_from_the_beginning() {
    let (_d, j) = journal();
    let p = Progress::default();
    assert_eq!(j.safe_cursor(&p).unwrap(), None);
    assert_eq!(j.acknowledged().unwrap(), None);
    assert_eq!(j.poll_offset(&p).unwrap(), 0);
}

/// ADR 0006's named failure mode, at the only place it can occur. Compaction empties the rows the
/// cursor derives from; if the offset fell back to zero there, the channel would be asked to
/// redeliver every update it has already been told to stop sending.
#[test]
fn a_compacted_journal_polls_from_the_acknowledged_offset_not_the_beginning() {
    let (_d, j) = journal();
    let p = Progress::default();
    let batch: Vec<_> = (1..=4).map(|n| (u(n), chan("g"))).collect();
    j.note_batch(&batch, &at(0)).unwrap();
    for n in 1..=4 {
        j.disposition(u(n), &Disposition::NotAddressed, &at(1)).unwrap();
    }
    j.acknowledge(u(5), &p).unwrap();
    assert_eq!(j.poll_offset(&p).unwrap(), 5, "the barrier governs while rows exist");

    assert_eq!(j.compact().unwrap(), 4);
    assert_eq!(j.safe_cursor(&p).unwrap(), None, "compaction emptied the derivation");
    assert_eq!(
        j.poll_offset(&p).unwrap(),
        5,
        "polled from the beginning after compaction — the entire history would be redelivered"
    );
}

/// An open update holds the offset below a higher acknowledgement rather than above it. Skipping
/// unfinished work to honour a stored number is the one direction that loses an answer.
#[test]
fn an_open_update_holds_the_offset_below_the_acknowledged_one() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(2), &Disposition::NotAddressed, &at(1)).unwrap();
    j.acknowledge(u(3), &p).unwrap();

    j.note_batch(&[(u(3), chan("g"))], &at(2)).unwrap();
    let key = p.open("k-1");
    j.disposition(u(3), &Disposition::Turn(key), &at(3)).unwrap();
    assert_eq!(j.poll_offset(&p).unwrap(), 3);
}

/// A turn the tenant journal could not answer for holds the barrier, and blocks acknowledgement
/// outright.
///
/// The two are not the same rule. The barrier stops the offset *passing* update 6; the refusal
/// stops any offset being written at all while 6 is unreadable, because acknowledgement is the
/// irreversible half — it is what makes the channel stop redelivering, and it is the bound
/// compaction deletes below.
#[test]
fn an_update_whose_progress_could_not_be_read_holds_the_barrier_and_refuses_acknowledgement() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let five = p.settled("k-5");
    let six = p.unreadable("k-6");
    let seven = p.settled("k-7");
    j.note_batch(&[(u(5), chan("g")), (u(6), chan("g")), (u(7), chan("g"))], &at(0)).unwrap();
    j.disposition(u(5), &Disposition::Turn(five), &at(1)).unwrap();
    j.disposition(u(6), &Disposition::Turn(six), &at(1)).unwrap();
    j.disposition(u(7), &Disposition::Turn(seven), &at(1)).unwrap();

    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(6)), "the barrier passed an unreadable turn");
    assert_eq!(j.poll_offset(&p).unwrap(), 6, "the channel would be asked to skip update 6");

    // Even an offset the barrier alone would allow — 6 acknowledges only *through* 5 — is
    // refused, and the refusal names the turn nothing could be read about.
    let err = j.acknowledge(u(6), &p).unwrap_err();
    let ChannelError::AcknowledgementOnUnestablishedProgress { id: 6, key } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(key, "k-6");
    assert_eq!(j.acknowledged().unwrap(), None, "a refused acknowledgement was recorded anyway");

    // POSITIVE CONTROL: the same three updates, with the journal answering. Without it, an
    // `acknowledge` that refused everything and a `safe_cursor` that never advanced would both
    // pass every assertion above.
    p.settled("k-6");
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(8)));
    j.acknowledge(u(8), &p).unwrap();
    assert_eq!(j.acknowledged().unwrap(), Some(u(8)));
}

/// The hold is not a property of the update, it is a property of the failed read — so it lifts,
/// and it lifts for the whole batch rather than one row at a time.
///
/// This is what separates it from
/// `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`, where the refusal
/// was a permanent verdict on one turn and nothing would ever lift it.
#[test]
fn an_unreadable_journal_stalls_the_offset_and_never_loses_it() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.unreadable("k-2");
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g"))], &at(0)).unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(1)).unwrap();
    j.disposition(u(2), &Disposition::Turn(key), &at(1)).unwrap();

    // The channel keeps being told 2, so it keeps sending 2. Redelivery is the recoverable
    // direction: the answer is delayed, not dropped.
    assert_eq!(j.poll_offset(&p).unwrap(), 2);
    assert!(j.acknowledge(u(2), &p).is_err());

    p.settled("k-2");
    assert_eq!(j.poll_offset(&p).unwrap(), 3);
    j.acknowledge(u(3), &p).unwrap();
}

#[test]
fn a_journal_from_another_version_is_refused() {
    let dir = tempdir::Dir::new();
    let path = dir.path().join("channel.db");
    {
        let j = ChannelJournal::open(&path).unwrap();
        let conn = j.conn.lock().unwrap();
        conn.execute("UPDATE channel_meta SET value='99' WHERE key='schema_version'", []).unwrap();
    }
    let err = ChannelJournal::open(&path).unwrap_err();
    assert!(matches!(err, ChannelError::Version { found: 99, expected: 1 }), "{err:?}");
}

#[test]
fn a_turn_row_without_its_key_is_refused_rather_than_read_as_noise() {
    let dir = tempdir::Dir::new();
    let path = dir.path().join("channel.db");
    let j = ChannelJournal::open(&path).unwrap();
    j.note_batch(&[(u(1), chan("g"))], &at(0)).unwrap();
    {
        let conn = j.conn.lock().unwrap();
        conn.execute("UPDATE channel_updates SET disposition='turn' WHERE id=1", []).unwrap();
    }
    // Reading it as `Unclaimed` would answer the question a second time; reading it as settled
    // would advance the offset past a possibly-billed turn. Neither is a guess worth making.
    assert!(j.record(u(1)).is_err());
}

// ── the dependency invariant ────────────────────────────────────────────────────────────────

/// This crate's narrowness is a property of its dependency list, so the dependency list is what
/// is asserted.
///
/// Reading the manifest is unusual for a unit test and it is the point: the invariant is not
/// "nobody imports records here", which any future edit could quietly break, but "this crate
/// cannot name the types where tenant business authority lives" — and that is decided in
/// `Cargo.toml`, not in any `.rs` file.
///
/// # Why `ironworks-store` is in the set
///
/// It holds the SQLite open protocol this crate shares with `journal` and `proposals`, and its own
/// dependency list is `ironworks-domain`, `rusqlite`, `thiserror` — this same narrow set. So the
/// **transitive** reach is unchanged: a store crate that later took a dependency on records or
/// service definitions would widen this list through the back door, which is why its manifest
/// carries the same reasoning and why this test names it rather than allowing any new edge.
#[test]
fn dependencies_are_the_narrow_set() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own manifest");
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("a [dependencies] section")
        .split("\n[")
        .next()
        .expect("the section body");
    let names: Vec<&str> = deps
        .lines()
        .filter_map(|l| l.split_once('=').map(|(n, _)| n.trim()))
        .filter(|n| !n.is_empty() && !n.starts_with('#'))
        .collect();
    assert_eq!(
        names,
        vec!["ironworks-domain", "ironworks-store", "rusqlite", "thiserror"],
        "the channel coordinator's dependency set changed. It is cross-tenant, so a dependency \
         on records, service definitions, composition or the runtime transport would give one \
         object both the shared cursor and tenant business authority. If this is deliberate, the \
         invariant in ironworks_domain::progress has to change first."
    );
}

// ── a temporary directory, without a dependency ─────────────────────────────────────────────

mod tempdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Removed on drop. A test-only convenience, and not worth a dependency in the process that
    /// holds every tenant's bearer token.
    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Self {
            // A counter, not a heap address. The first draft used `{:p}` of a fresh `Box`, which
            // is unique only while the box is alive — the allocator hands the same address back
            // after the drop, so parallel tests silently shared one database and asserted over
            // each other's rows. It failed loudly here; in a proof harness it would have passed.
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ironworks-channel-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(unix)]
fn mode_of(p: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).expect("stat the store").permissions().mode() & 0o777
}

/// The store is owner-only, and so are the write-ahead siblings.
///
/// The `-wal` file holds pages that have not been checkpointed, so a claim about the main
/// database alone would be a claim about the wrong file. Measured here rather than assumed:
/// SQLite creates both siblings with the mode of the database it opened, which is why restricting
/// one restricts all three.
#[cfg(unix)]
#[test]
fn the_store_and_its_write_ahead_siblings_are_owner_only() {
    let (d, j) = journal();
    let db = d.path().join("channel.db");
    j.note_batch(&[(u(1), chan("g"))], &at(0)).unwrap();

    assert_eq!(mode_of(&db), 0o600, "the channel journal is group- or world-readable");
    for suffix in ["-wal", "-shm"] {
        let sibling = d.path().join(format!("channel.db{suffix}"));
        assert!(sibling.exists(), "{suffix} was never created — this test measures nothing");
        assert_eq!(mode_of(&sibling), 0o600, "{suffix} is group- or world-readable");
    }
}

/// B10: a store whose bytes are not a database fails closed, and says the one thing that stops
/// the obvious remedy from destroying it.
///
/// The reference implementation's message carries "do NOT delete it blind" for a reason: the
/// natural response to "could not be opened" is to remove the file and retry, and this file is
/// the only record of how far the channel has been acknowledged. Asserting the *guidance* rather
/// than merely the error type is the point — a bare `file is not a database` is a true sentence
/// that leads an operator to the wrong action.
#[test]
fn a_store_that_is_not_a_database_refuses_and_says_not_to_delete_it() {
    let d = tempdir::Dir::new();
    let p = d.path().join("channel.db");
    std::fs::write(&p, b"this is not a database".repeat(10)).expect("write");

    let err = ChannelJournal::open(&p).expect_err("a corrupt store was opened");
    assert!(matches!(err, ChannelError::NotADatabase { .. }), "{err:?}");
    let said = err.to_string();
    assert!(said.contains("do NOT delete it blind"), "{said}");
    assert!(said.contains("channel.db"), "the message does not name the file: {said}");
}

/// The control the previous test needs: an ordinary unopenable path must NOT be reported as a
/// corrupt store, or the guidance becomes noise attached to every failure.
#[test]
fn an_unopenable_path_is_not_reported_as_a_corrupt_store() {
    let d = tempdir::Dir::new();
    let err = ChannelJournal::open(d.path().join("no-such-dir").join("channel.db"))
        .expect_err("opened a store under a directory that does not exist");
    assert!(matches!(err, ChannelError::Open { .. }), "{err:?}");
    assert!(!err.to_string().contains("do NOT delete"), "{err}");
}

/// A stored cursor that does not parse must refuse, not read as "nothing recorded".
///
/// Both readers of `cursor_acked` used `.and_then(|v| v.parse().ok())`, and `None` from that is
/// indistinguishable from an absent row. Two consequences, in opposite directions and both wrong:
///
/// 1. `acknowledge` reads `None` as "no prior acknowledgement", which **switches off**
///    `AcknowledgementWouldRegress` — any offset becomes acceptable.
/// 2. Once compaction has removed the rows `safe_cursor` derives from, `poll_offset` falls through
///    to the stored acknowledgement — and `None` there becomes `0`. That is verbatim the failure
///    its own doc cites ADR 0006 to rule out: *"a derivation that fell back to zero here would
///    redeliver the entire history"*.
///
/// Nothing in-process writes a non-numeric value. A damaged file is the case a crate running
/// `synchronous = FULL` exists to survive, which is why this refuses rather than assumes.
#[test]
fn a_cursor_that_cannot_be_read_refuses_rather_than_reading_as_absent() {
    let (d, j) = journal();
    let p = Progress::default();
    j.note_batch(&[(u(10), chan("g")), (u(11), chan("g"))], &at(0)).unwrap();
    j.disposition(u(10), &Disposition::NotAddressed, &at(0)).unwrap();
    j.disposition(u(11), &Disposition::NotAddressed, &at(0)).unwrap();
    // Everything is finished, so the barrier sits past the last update and the channel may be
    // told that offset — which is what lets compaction then clear every row.
    let safe = j.safe_cursor(&p).unwrap().expect("a barrier with rows present");
    j.acknowledge(safe, &p).unwrap();

    // Positive control: while the row is readable every one of these answers, so what follows the
    // corruption is about the corruption alone.
    assert_eq!(j.acknowledged().unwrap(), Some(safe));
    assert!(
        matches!(j.acknowledge(u(4), &p), Err(ChannelError::AcknowledgementWouldRegress { .. })),
        "the regression guard is not armed even on a readable cursor"
    );
    // Compaction is what removes the rows `safe_cursor` reads; after it, the stored offset is the
    // only thing carrying the position. Control: it still carries it while it is readable.
    assert_eq!(j.compact().unwrap(), 2, "compaction left rows the derivation can still read");
    assert_eq!(
        j.poll_offset(&p).unwrap(),
        safe.get(),
        "the stored offset stopped carrying the position once the rows were gone"
    );

    drop(j);
    let db = d.path().join("channel.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE channel_meta SET value='not-a-number' WHERE key='cursor_acked'", [])
            .unwrap();
    }
    let j = ChannelJournal::open(&db).unwrap();

    assert!(j.acknowledged().is_err(), "an unreadable cursor read as 'never acknowledged'");
    assert!(j.poll_offset(&p).is_err(), "an unreadable cursor became offset 0");
    assert!(j.acknowledge(u(4), &p).is_err(), "an unreadable cursor disarmed the regression guard");
}

/// Compaction forgets exactly the dispositions that are settled and were never billed.
///
/// `compact`'s SQL used to carry its own hand-typed list, and `command` had fallen out of it while
/// `Disposition::cost_a_turn` — described in its own doc as *"the compaction rule in one
/// predicate"* — says a command never cost a turn. That is the enumeration defect `compact`'s doc
/// warns about, reproduced one paragraph below the warning:
///
/// > an enumeration that was wrong twice, most expensively when the operator-reconciliation alarm
/// > turned out to be age-compactable and a possibly-billed, never-delivered turn was deleted with
/// > nobody having acted on it.
///
/// So the set is asserted against the predicates rather than against another list. A disposition
/// that is settled and unbilled must be forgettable; one that is open or billed must not be.
#[test]
fn compaction_forgets_exactly_what_was_never_billed() {
    let p = Progress::default();
    let every = [
        Disposition::Unclaimed,
        Disposition::Turn(IdempotencyKey::parse("k-1").unwrap()),
        Disposition::NotAddressed,
        Disposition::Unroutable,
        Disposition::Command,
    ];
    for d in &every {
        let forgettable = COMPACTABLE.iter().any(|c| c.tag() == d.tag());
        let settled_and_free = d.openness(&p) == Openness::Settled && !d.cost_a_turn();
        assert_eq!(
            forgettable,
            settled_and_free,
            "{:?}: compaction and the predicates disagree about whether it may be forgotten",
            d.tag()
        );
    }

    // And the same rule through the database, so the SQL is measured rather than the list.
    let (_d, j) = journal();
    j.note_batch(
        &[(u(1), chan("g")), (u(2), chan("g")), (u(3), chan("g")), (u(4), chan("g"))],
        &at(0),
    )
    .unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, &at(0)).unwrap();
    j.disposition(u(2), &Disposition::Unroutable, &at(0)).unwrap();
    j.disposition(u(3), &Disposition::Command, &at(0)).unwrap();
    let key = IdempotencyKey::parse("k-1").unwrap();
    j.disposition(u(4), &Disposition::Turn(key), &at(0)).unwrap();

    let safe = j.safe_cursor(&p).unwrap().expect("all settled");
    j.acknowledge(safe, &p).unwrap();
    assert_eq!(j.compact().unwrap(), 3, "the command row was retained forever");

    // Positive control: the billed row is still there, which is what the rule protects.
    assert!(j.record(u(4)).unwrap().is_some(), "a billed turn was forgotten");
    assert!(j.record(u(3)).unwrap().is_none(), "the command row survived compaction");
}

/// A damaged `schema_version` must be refused, not read as "no version recorded". See the same
/// test in `crates/journal/src/tests.rs` — one rule, three stores, and it was wrong in two.
///
/// This crate already refused, via `parse::<i64>().unwrap_or(-1)`, and reported `found: -1` — a
/// number that was never on disk. It now says what it read.
#[test]
fn a_damaged_schema_version_is_refused_rather_than_read_as_absent() {
    let dir = tempdir::Dir::new();
    let path = dir.path().join("channel.db");
    drop(ChannelJournal::open(&path).expect("open"));

    // The positive control: an untouched store re-opens.
    ChannelJournal::open(&path).expect("an untouched channel journal re-opens");

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE channel_meta SET value='one' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    match ChannelJournal::open(&path) {
        Err(ChannelError::UnreadableVersion { raw }) => assert_eq!(raw, "one"),
        other => panic!("a damaged schema version must be refused, got {other:?}"),
    }
}
