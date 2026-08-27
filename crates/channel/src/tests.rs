use super::*;
use std::collections::HashMap;

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
    j.note_batch(&[(u(7), chan("g1")), (u(8), chan("g2")), (u(9), chan("g1"))], "t0").unwrap();
    // Nothing has been routed, let alone answered. The offset must be behind all three, which is
    // the entire reason the batch is journalled before a worker starts.
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(7)));
}

#[test]
fn a_redelivered_update_keeps_its_original_row() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.settled("k-7");
    j.note_batch(&[(u(7), chan("g1"))], "t0").unwrap();
    j.disposition(u(7), &Disposition::Turn(key), "t1").unwrap();

    // The channel sends 7 again, because it was never told an offset past it.
    let fresh = j.note_batch(&[(u(7), chan("g1"))], "t2").unwrap();
    assert_eq!(fresh, 0, "a redelivery inserted a second row");
    let row = j.record(u(7)).unwrap().unwrap();
    assert_eq!(row.received_at, "t0", "a redelivery reset the arrival time");
    assert!(row.disposition.cost_a_turn(), "a redelivery reset the disposition to unclaimed");
}

// ── the one-way disposition (E-4) ───────────────────────────────────────────────────────────

#[test]
fn an_update_that_cost_a_turn_can_never_be_relabelled_as_noise() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.open("k-1");
    j.note_batch(&[(u(1), chan("g1"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::Turn(key), "t1").unwrap();

    // The tenant leaves the registry mid-turn. Routing now says "nobody owns this channel" —
    // and that must not be allowed to rewrite the history of an update with a possibly-billed
    // answer behind it.
    let err = j.disposition(u(1), &Disposition::NotAddressed, "t2").unwrap_err();
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
    j.note_batch(&[(u(1), chan("dm"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::Command, "t1").unwrap();

    let record = j.record(u(1)).unwrap().expect("the row survives the round trip");
    assert_eq!(record.disposition, Disposition::Command);
    assert_eq!(record.settled_at.as_deref(), Some("t1"));
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
    j.note_batch(&[(u(1), chan("dm"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::Command, "t1").unwrap();
    let err = j.disposition(u(1), &Disposition::NotAddressed, "t2").unwrap_err();
    assert!(
        matches!(err, ChannelError::AlreadyDispositioned { existing: "command", .. }),
        "{err:?}"
    );
}

#[test]
fn a_disposition_needs_an_update_to_attach_to() {
    let (_d, j) = journal();
    let err = j.disposition(u(4), &Disposition::NotAddressed, "t0").unwrap_err();
    assert!(matches!(err, ChannelError::NoSuchUpdate { id: 4 }), "{err:?}");
}

// ── the cross-tenant barrier, through the durable path ──────────────────────────────────────

#[test]
fn a_faster_tenant_cannot_acknowledge_a_slower_tenants_unfinished_work() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let alpha = p.open("k-alpha");
    let beta = p.settled("k-beta");
    j.note_batch(&[(u(10), chan("alpha")), (u(11), chan("beta"))], "t0").unwrap();
    j.disposition(u(10), &Disposition::Turn(alpha), "t1").unwrap();
    j.disposition(u(11), &Disposition::Turn(beta), "t1").unwrap();

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
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g")), (u(3), chan("g"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, "t1").unwrap();
    j.disposition(u(2), &Disposition::Unroutable, "t1").unwrap();
    j.disposition(u(3), &Disposition::Turn(key), "t1").unwrap();
    assert_eq!(j.safe_cursor(&p).unwrap(), Some(u(3)));
    assert!(!j.record(u(1)).unwrap().unwrap().disposition.cost_a_turn());
}

// ── acknowledgement is not delivery ─────────────────────────────────────────────────────────

#[test]
fn an_offset_may_not_be_acknowledged_past_the_barrier() {
    let (_d, j) = journal();
    let mut p = Progress::default();
    let key = p.open("k-5");
    j.note_batch(&[(u(5), chan("g")), (u(6), chan("g"))], "t0").unwrap();
    j.disposition(u(5), &Disposition::Turn(key), "t1").unwrap();
    j.disposition(u(6), &Disposition::NotAddressed, "t1").unwrap();

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
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, "t1").unwrap();
    j.disposition(u(2), &Disposition::NotAddressed, "t1").unwrap();
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
    j.note_batch(&[(u(1), chan("g"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::Turn(key), "t1").unwrap();

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
    j.note_batch(&[(u(1), chan("g"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, "t1").unwrap();
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
    j.note_batch(&batch, "t0").unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, "t1").unwrap();
    j.disposition(u(2), &Disposition::Turn(key), "t1").unwrap();
    j.disposition(u(3), &Disposition::Unroutable, "t1").unwrap();
    j.disposition(u(4), &Disposition::NotAddressed, "t1").unwrap();
    j.disposition(u(5), &Disposition::NotAddressed, "t1").unwrap();
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
    j.note_batch(&batch, "t0").unwrap();
    for n in 1..=4 {
        j.disposition(u(n), &Disposition::NotAddressed, "t1").unwrap();
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
    j.note_batch(&batch, "t0").unwrap();
    for n in 1..=4 {
        j.disposition(u(n), &Disposition::NotAddressed, "t1").unwrap();
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
    j.note_batch(&[(u(1), chan("g")), (u(2), chan("g"))], "t0").unwrap();
    j.disposition(u(1), &Disposition::NotAddressed, "t1").unwrap();
    j.disposition(u(2), &Disposition::NotAddressed, "t1").unwrap();
    j.acknowledge(u(3), &p).unwrap();

    j.note_batch(&[(u(3), chan("g"))], "t2").unwrap();
    let key = p.open("k-1");
    j.disposition(u(3), &Disposition::Turn(key), "t3").unwrap();
    assert_eq!(j.poll_offset(&p).unwrap(), 3);
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
    j.note_batch(&[(u(1), chan("g"))], "t0").unwrap();
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
        vec!["ironworks-domain", "rusqlite", "thiserror"],
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
    j.note_batch(&[(u(1), chan("g"))], "t0").unwrap();

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
