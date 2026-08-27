//! B5–B7: what a process death around **delivery** leaves behind.
//!
//! These live here rather than in either journal because the boundaries are about the two
//! recovering *together*. The turn journal knows an answer exists; the channel journal knows the
//! question is still outstanding; neither, alone, can tell you whether a client is about to read
//! the same answer twice.
//!
//! | Boundary | Crash point | Required behaviour |
//! |---|---|---|
//! | **B5** | after the channel accepted it, before that was recorded | a duplicate, and it must be the **identical** answer — never a new one |
//! | **B6** | inside "delivered *and* the offset advanced" | unrepresentable: the offset is derived, so there is no second write |
//! | **B7** | offset durable, channel not yet told | it redelivers; no rerun, no resend, only an acknowledgement |
//!
//! # How process death is modelled
//!
//! A step returns [`Step::Crashed`] and writes nothing further. That is not a weaker stand-in for
//! a signal — for durable state it is exactly what `SIGKILL` means, and it is stronger than an
//! exception, which a handler could catch and tidy up after. Nothing in this file runs cleanup
//! after a crash, because a killed process does not get to.
//!
//! # What is asserted at every boundary
//!
//! How many model runs happened, how many fetches, how many sends and whether their text was
//! **byte-identical**, the durable delivery state, the derived cursor, and what the channel was
//! told. A count alone would pass for an implementation that sent a second, different answer.

use ironworks_channel::{ChannelError, ChannelJournal, Disposition, UpdateId};
use ironworks_domain::{
    AttemptOutcome, CapabilityProfile, ChannelRef, Composition, ContextBinding, CredentialPurpose,
    CredentialRef, DeliveryState, Guidance, IdempotencyKey, ModelPin, ModelSelection, Openness,
    OrgId, PartRef, PartSource, Repair, ResponseId, RuntimeIdentity, ScopeEvidence, ScopeFailure,
    ScopeResolver, ServiceDefinition, ServiceId, ServiceInstance, Slug, ThreadScope, TurnProgress,
};
use ironworks_journal::{Decision, Journal, Recovery, RequestDigest, TurnOutcome};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The composition these tests pretend to have been built on. Threads record it so a
/// continuation decision can tell "the same conversation" from "a different alloy".
const ALLOY: &str = "aaaaaaaaaaaaaaaa";

/// The one room these tests serve. Threads are keyed by conversation, so a
/// pointer is always asked for by name.
fn room() -> ironworks_domain::ChannelRef {
    ironworks_domain::ChannelRef::new("-100999")
}

// ── a disposable directory ──────────────────────────────────────────────────────────────────

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ironworks-boundaries-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── the world outside the process ───────────────────────────────────────────────────────────

/// The runtime and the channel. Survives restarts, because they are different processes — which
/// is the whole reason a duplicate is possible at all.
#[derive(Default)]
struct World {
    /// Every model execution: the idempotency key it ran under.
    runs: RefCell<Vec<String>>,
    /// response id -> the answer. Fetching is free and always returns the same bytes.
    answers: RefCell<HashMap<String, String>>,
    fetches: RefCell<Vec<String>>,
    /// Everything the channel accepted, in order.
    sent: RefCell<Vec<String>>,
    /// The offset the channel has been told. Below it, nothing is redelivered.
    told: RefCell<Option<u64>>,
}

impl World {
    /// Execute a turn. Each run produces a **different** answer on purpose: if any recovery path
    /// regenerates instead of fetching, the identity assertions fail rather than the count ones.
    fn run(&self, key: &IdempotencyKey) -> ResponseId {
        let mut runs = self.runs.borrow_mut();
        runs.push(key.as_str().to_owned());
        let n = runs.len();
        let id = format!("resp_{n:03}");
        self.answers.borrow_mut().insert(id.clone(), format!("ANSWER#{n}"));
        ResponseId::parse(id).unwrap()
    }
    fn fetch(&self, id: &ResponseId) -> String {
        self.fetches.borrow_mut().push(id.as_str().to_owned());
        self.answers.borrow().get(id.as_str()).cloned().expect("a known response")
    }
    fn accept(&self, text: &str) {
        self.sent.borrow_mut().push(text.to_owned());
    }
    fn tell(&self, offset: UpdateId) {
        self.told.borrow_mut().replace(offset.get());
    }
    /// What the channel will send on the next poll: everything at or above the offset it was
    /// told. This is the redelivery that makes B5 and B7 real rather than hypothetical.
    fn redelivers(&self, all: &[u64]) -> Vec<u64> {
        let from = self.told.borrow().unwrap_or(0);
        all.iter().copied().filter(|id| *id >= from).collect()
    }
    fn runs(&self) -> usize {
        self.runs.borrow().len()
    }
    fn sends(&self) -> Vec<String> {
        self.sent.borrow().clone()
    }
}

// ── the process ─────────────────────────────────────────────────────────────────────────────

/// Named crash points, in the order the delivery half executes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    /// Between the channel accepting the message and that becoming durable. **B5.**
    AfterSend,
    /// After delivery is durable, before the channel has been told. **B7.**
    AfterDelivered,
    /// The channel refuses the message outright, having accepted no part of it — what a route
    /// that no longer exists looks like from here. Not a crash: the process survives and must
    /// decide what the refusal means. **B11.**
    SendRefused,
    Never,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Handled,
    AlreadySettled,
    Reconcile(Repair),
    Crashed,
}

/// One process lifetime: it opens the journals, handles what the channel offers, and dies.
struct Process<'w> {
    world: &'w World,
    turns: Journal,
    channel: ChannelJournal,
}

impl<'w> Process<'w> {
    fn boot(world: &'w World, dir: &Dir) -> Self {
        Self {
            world,
            turns: Journal::open(dir.path().join("turns.db")).expect("turn journal"),
            channel: ChannelJournal::open(dir.path().join("channel.db")).expect("channel journal"),
        }
    }

    /// A poll: journal the whole batch before any of it is worked on, then handle each update.
    ///
    /// The order is the point. Every update in the batch is durable and open before the first
    /// one is touched, so a fast tenant finishing update 11 cannot acknowledge past a slow
    /// tenant's update 10 — 10 already exists and already holds the barrier.
    fn poll(&self, batch: &[(u64, &str)], at: At, stop_after: usize) -> Vec<Step> {
        self.poll_inner(batch, at, stop_after, None)
    }

    /// A poll in which the fault applies to the `nth` update only, the rest running normally.
    ///
    /// B8 needs this and `poll` cannot express it: the whole question is what a fault in the
    /// *middle* of a batch does to the updates either side of it, which a fault applied to every
    /// update cannot show.
    fn poll_crashing_at(&self, batch: &[(u64, &str)], at: At, nth: usize) -> Vec<Step> {
        self.poll_inner(batch, at, usize::MAX, Some(nth))
    }

    fn poll_inner(
        &self,
        batch: &[(u64, &str)],
        at: At,
        stop_after: usize,
        only_nth: Option<usize>,
    ) -> Vec<Step> {
        // Acknowledgement first: the offset carried by *this* poll is what tells the channel we
        // are past the previous batch. That is the ACK, and it happens on a later poll than the
        // delivery it acknowledges — which is precisely why DELIVERED and ACKED are two facts.
        if let Ok(Some(safe)) = self.channel.safe_cursor(&self.turns) {
            self.world.tell(safe);
            let _ = self.channel.acknowledge(safe, &self.turns);
        }
        let rows: Vec<(UpdateId, ChannelRef)> =
            batch.iter().map(|(id, ch)| (UpdateId::new(*id), ChannelRef::new(*ch))).collect();
        self.channel.note_batch(&rows, "t0").expect("journal the batch");

        let mut out = Vec::new();
        for (n, (id, _)) in rows.iter().enumerate() {
            if n >= stop_after {
                // A graceful stop: finish the update in hand, leave the rest. Safe precisely
                // because the offset is the lowest open update, not a batch boundary.
                break;
            }
            // A fault aimed at one update leaves its neighbours running normally.
            let step =
                self.handle(*id, if only_nth.is_none_or(|k| k == n) { at } else { At::Never });
            let crashed = step == Step::Crashed;
            out.push(step);
            if crashed {
                break;
            }
        }
        out
    }

    fn handle(&self, id: UpdateId, at: At) -> Step {
        let record = self.channel.record(id).expect("read").expect("journalled by the batch");
        let key = match &record.disposition {
            Disposition::Unclaimed => {
                let key = mint_key(id);
                self.channel
                    .disposition(id, &Disposition::Turn(key.clone()), "t1")
                    .expect("disposition");
                key
            }
            Disposition::Turn(key) => key.clone(),
            // Nothing to do, and nothing this test exercises.
            _ => return Step::AlreadySettled,
        };

        // Drive the turn to a durable response id. B1–B4 are proven in the turn journal's own
        // suite; here it is a precondition, so it is done in as few moves as possible.
        let scope = scope();
        if self.turns.turn(&key).expect("read").is_none() {
            self.turns
                .begin(&scope, &key, "2026-08-27T00:00:00Z", &digest(), &Decision::default(), "t1")
                .expect("begin");
        }
        let mut record = self.turns.turn(&key).expect("read").expect("begun");
        if matches!(record.outcome, TurnOutcome::NotSubmitted)
            && record.delivery.awaits_automatic_work()
        {
            let response = self.world.run(&key);
            self.turns
                .commit_completed(&key, &scope.pointer(response), Some(&room()), ALLOY, None, "t2")
                .expect("commit");
            record = self.turns.turn(&key).expect("read").expect("committed");
        }

        // Delivery. Everything below reads from durable evidence, never from what this process
        // did a moment ago — because after a crash there is no "a moment ago".
        match ironworks_journal::recover(&record) {
            Recovery::Settled { .. } => Step::AlreadySettled,
            Recovery::ReconcileDelivery { repair, .. } => Step::Reconcile(repair),
            Recovery::Deliver { response } => {
                let text = self.world.fetch(&response);
                if at == At::SendRefused {
                    // Nothing was accepted, and the transport can prove it — so this is
                    // `KnownUnsent`, not `Uncertain`. The verdict is then read back off disk
                    // rather than asserted here, because after a restart there is no here.
                    self.turns
                        .record_delivery_attempt(&key, AttemptOutcome::Rejected, "t3")
                        .expect("record the refusal");
                    let after = self.turns.turn(&key).expect("read").expect("a turn");
                    return match ironworks_journal::recover(&after) {
                        Recovery::ReconcileDelivery { repair, .. } => Step::Reconcile(repair),
                        other => panic!("a refused send must park for an operator: {other:?}"),
                    };
                }
                self.world.accept(&text);
                if at == At::AfterSend {
                    // Dead. The channel has the message; nothing here knows it.
                    return Step::Crashed;
                }
                self.turns
                    .record_delivery_attempt(&key, AttemptOutcome::Complete, "t3")
                    .expect("record delivery");
                if at == At::AfterDelivered {
                    // Dead. Delivery is durable, the channel has not been told.
                    return Step::Crashed;
                }
                Step::Handled
            }
            other => panic!("the turn half is a precondition here, not under test: {other:?}"),
        }
    }

    fn cursor(&self) -> Option<UpdateId> {
        self.channel.safe_cursor(&self.turns).expect("cursor")
    }
    fn delivery(&self, id: UpdateId) -> DeliveryState {
        self.turns.turn(&mint_key(id)).expect("read").expect("a turn").delivery
    }
}

// ── fixtures ────────────────────────────────────────────────────────────────────────────────

/// One update, one logical turn. Deriving the key from the update id rather than minting a fresh
/// one is what makes a redelivered update find the turn it already started.
fn mint_key(id: UpdateId) -> IdempotencyKey {
    IdempotencyKey::parse(format!("upd-{id}")).expect("key")
}

fn digest() -> RequestDigest {
    RequestDigest::of(b"a fixed request body")
}

struct Parts(BTreeMap<String, String>);
impl PartSource for Parts {
    fn read(&self, p: &PartRef) -> Option<&str> {
        self.0.get(p.as_str()).map(String::as_str)
    }
}

struct Store;
impl ScopeResolver for Store {
    fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
        Ok(ScopeEvidence::from_record_store(
            OrgId::parse("acme-sales").unwrap(),
            "http://store.example",
        ))
    }
}

/// A verified instance, because a `ThreadScope` cannot be obtained any other way — which is the
/// point of the type, and it holds just as much in a test as in the binary.
fn scope() -> ThreadScope {
    let parts = Parts(BTreeMap::from([
        ("p".to_string(), "PART".to_string()),
        ("t".to_string(), "TAIL".to_string()),
    ]));
    let definition = ServiceDefinition::new(
        ServiceId::parse("s").unwrap(),
        1,
        "r",
        Composition::new(vec![PartRef::new("p")], "# H", PartRef::new("t")).unwrap(),
        CapabilityProfile::READ_ONLY_RECORDS,
        None,
    )
    .unwrap();
    let guidance = Guidance::parse(
        &format!("<!-- tenant-guidance v1 slug: acme service: s -->\n\n{}", "content ".repeat(80)),
        "acme",
        "s",
    )
    .unwrap();
    ServiceInstance::new(
        Slug::parse("acme").unwrap(),
        definition,
        guidance,
        RuntimeIdentity::parse("m").unwrap(),
        CredentialRef::new(CredentialPurpose::RuntimeMember, "env:A"),
        ContextBinding::new(CredentialRef::new(CredentialPurpose::RecordScope, "env:B")).unwrap(),
        ModelSelection::from_pin(&ModelPin::parse("M # c").unwrap()),
        CapabilityProfile::READ_ONLY_RECORDS,
        None,
        &parts,
    )
    .unwrap()
    .verify(&Store)
    .unwrap()
    .thread_scope()
}

// ── B5 ──────────────────────────────────────────────────────────────────────────────────────

/// **The residual window.** The channel has the message; the process never wrote that down. A
/// duplicate is unavoidable — and it must be the *same* answer.
///
/// The journal holds no answer text, so identity is not achieved by caching it. It is achieved by
/// the response id being durable before the send: fetching a fixed id returns the answer that
/// already went out. That is the whole mechanism, and this test is what proves it, because the
/// fake runtime returns a different answer for every execution.
#[test]
fn b5_the_residual_window_yields_an_identical_duplicate_never_a_new_answer() {
    let dir = Dir::new();
    let world = World::default();

    let first = {
        let p = Process::boot(&world, &dir);
        let steps = p.poll(&[(1, "g1")], At::AfterSend, usize::MAX);
        assert_eq!(steps, vec![Step::Crashed]);
        // Nothing about the send is durable, so the update is still open and the offset is behind
        // it. That is what makes the channel send it again.
        assert_eq!(p.delivery(UpdateId::new(1)), DeliveryState::Pending);
        assert_eq!(p.cursor(), Some(UpdateId::new(1)));
        world.sends()
    };
    assert_eq!(first.len(), 1);

    // A new process. The channel redelivers update 1, because it was never told an offset past it.
    let p = Process::boot(&world, &dir);
    assert_eq!(world.redelivers(&[1]), vec![1], "the channel would not have resent it");
    let steps = p.poll(&[(1, "g1")], At::Never, usize::MAX);

    assert_eq!(steps, vec![Step::Handled]);
    assert_eq!(world.runs(), 1, "a second model turn ran for one logical question");
    let sends = world.sends();
    assert_eq!(sends.len(), 2, "the residual window did not produce the expected duplicate");
    assert_eq!(
        sends[0], sends[1],
        "the duplicate differs from the original — a client read two different answers to one \
         question, which is the exact failure this boundary exists to remove"
    );
    assert_eq!(p.delivery(UpdateId::new(1)), DeliveryState::Delivered);
    assert_eq!(p.cursor(), Some(UpdateId::new(2)));
}

// ── B6 ──────────────────────────────────────────────────────────────────────────────────────

/// **The boundary that cannot exist.**
///
/// The reference implementation stores the offset, so "delivered" and "the offset advanced" are
/// two writes it must make atomic; its test crashes inside that transaction and asserts neither
/// landed. Here the offset is derived from the rows, so there is no second write to be
/// non-atomic with, and the split state has no representation.
///
/// A test cannot assert the absence of a method, so it asserts the consequence: over every
/// durable combination of turn outcome and delivery evidence, the derived cursor and the
/// delivered state agree — because one is computed from the other.
#[test]
fn b6_delivered_and_the_offset_cannot_disagree() {
    let outcomes = ["not_submitted", "submitted_unknown", "accepted", "completed"];
    let deliveries = [
        DeliveryState::Pending,
        DeliveryState::KnownUnsent,
        DeliveryState::Uncertain { acknowledged_chunks: 0 },
        DeliveryState::Uncertain { acknowledged_chunks: 2 },
        DeliveryState::Delivered,
    ];
    for outcome in outcomes {
        for delivery in deliveries {
            let dir = Dir::new();
            let world = World::default();
            let p = Process::boot(&world, &dir);
            let id = UpdateId::new(1);
            let key = mint_key(id);
            p.channel.note_batch(&[(id, ChannelRef::new("g1"))], "t0").expect("batch");
            p.channel.disposition(id, &Disposition::Turn(key.clone()), "t1").expect("disposition");

            let scope = scope();
            p.turns
                .begin(&scope, &key, "2026-08-27T00:00:00Z", &digest(), &Decision::default(), "t1")
                .expect("begin");
            match outcome {
                "submitted_unknown" => p.turns.record_unknown(&key, "t2").expect("unknown"),
                "accepted" => p
                    .turns
                    .record_accepted(&key, &ResponseId::parse("resp_001").unwrap(), "t2")
                    .expect("accepted"),
                "completed" => p
                    .turns
                    .commit_completed(
                        &key,
                        &scope.pointer(ResponseId::parse("resp_001").unwrap()),
                        Some(&room()),
                        ALLOY,
                        None,
                        "t2",
                    )
                    .expect("completed"),
                _ => {}
            }
            let attempt = match delivery {
                DeliveryState::Pending => None,
                DeliveryState::KnownUnsent => Some(AttemptOutcome::Rejected),
                DeliveryState::Uncertain { acknowledged_chunks } => {
                    Some(AttemptOutcome::Ambiguous { acknowledged_chunks })
                }
                DeliveryState::Delivered => Some(AttemptOutcome::Complete),
            };
            if let Some(attempt) = attempt {
                p.turns.record_delivery_attempt(&key, attempt, "t3").expect("delivery");
            }

            let stored = p.turns.turn(&key).expect("read").expect("a turn").delivery;
            assert_eq!(stored, delivery, "delivery evidence did not round-trip");
            let past = p.cursor() == Some(UpdateId::new(2));
            let settled = p.turns.openness(&key) == Openness::Settled;
            assert_eq!(
                past, settled,
                "the offset and the update's settlement disagree at ({outcome}, {delivery:?}) — \
                 which is the split state B6 is about, and it should have no representation"
            );
        }
    }
}

// ── B7 ──────────────────────────────────────────────────────────────────────────────────────

/// **The acknowledgement gap.** Delivery is durable and the channel has not been told, so it
/// sends the question again. There must be no rerun and no resend — only an acknowledgement.
#[test]
fn b7_delivered_but_unacknowledged_yields_no_rerun_and_no_resend() {
    let dir = Dir::new();
    let world = World::default();

    {
        let p = Process::boot(&world, &dir);
        assert_eq!(p.poll(&[(1, "g1")], At::AfterDelivered, usize::MAX), vec![Step::Crashed]);
        assert_eq!(p.delivery(UpdateId::new(1)), DeliveryState::Delivered);
        // Locally finished: the derived offset is past it.
        assert_eq!(p.cursor(), Some(UpdateId::new(2)));
        // But the channel was never told, so it is not acknowledged and will redeliver.
        assert!(!p.channel.is_acknowledged(UpdateId::new(1)).expect("acked"));
    }
    assert_eq!(world.runs(), 1);
    assert_eq!(world.sends().len(), 1);
    assert_eq!(world.redelivers(&[1]), vec![1], "the channel would not have resent it");

    let p = Process::boot(&world, &dir);
    let steps = p.poll(&[(1, "g1")], At::Never, usize::MAX);

    assert_eq!(steps, vec![Step::AlreadySettled]);
    assert_eq!(world.runs(), 1, "an already-delivered update ran a second model turn");
    assert_eq!(world.sends().len(), 1, "an already-delivered update was sent again");
    assert_eq!(world.fetches.borrow().len(), 1, "recovery work happened for a delivered update");
    // This poll carried the offset, which is what turns DELIVERED into ACKED.
    assert!(p.channel.is_acknowledged(UpdateId::new(1)).expect("acked"));
    assert_eq!(p.channel.acknowledged().expect("acked"), Some(UpdateId::new(2)));
}

// ── the batch, the barrier, and shutdown ────────────────────────────────────────────────────

/// A SIGTERM mid-batch finishes the update in hand and leaves the rest for the next process.
/// Nothing here implements that: it is the derived offset doing its job.
#[test]
fn a_graceful_stop_leaves_the_rest_of_the_batch_for_the_next_process() {
    let dir = Dir::new();
    let world = World::default();
    {
        let p = Process::boot(&world, &dir);
        let steps = p.poll(&[(1, "g1"), (2, "g1"), (3, "g1")], At::Never, 1);
        assert_eq!(steps, vec![Step::Handled]);
        assert_eq!(p.cursor(), Some(UpdateId::new(2)), "the unhandled updates were acknowledged");
    }
    assert_eq!(world.runs(), 1);

    // The next process is offered the whole batch again and picks up exactly where this one left
    // off: update 1 is settled, 2 and 3 were journalled by the first poll and are still open.
    let p = Process::boot(&world, &dir);
    let steps = p.poll(&[(1, "g1"), (2, "g1"), (3, "g1")], At::Never, usize::MAX);
    assert_eq!(steps, vec![Step::AlreadySettled, Step::Handled, Step::Handled]);
    assert_eq!(world.runs(), 3, "update 1 was answered twice");
    assert_eq!(world.sends().len(), 3);
    assert_eq!(p.cursor(), Some(UpdateId::new(4)));
}

/// The batch is durable before any of it is worked on, so a crash before the first update leaves
/// every one of them queued rather than lost.
#[test]
fn a_crash_before_the_first_update_leaves_the_whole_batch_queued() {
    let dir = Dir::new();
    let world = World::default();
    {
        let p = Process::boot(&world, &dir);
        assert!(p.poll(&[(4, "g1"), (5, "g2"), (6, "g1")], At::Never, 0).is_empty());
        assert_eq!(p.cursor(), Some(UpdateId::new(4)), "an unworked batch moved the offset");
    }
    assert_eq!(world.runs(), 0);
    assert_eq!(world.sends().len(), 0);

    let p = Process::boot(&world, &dir);
    // Note the *empty* batch: nothing is redelivered, and the queued work is still found. Without
    // journalling the batch up front, these three would only exist in a dead process's memory.
    assert_eq!(p.channel.records().expect("rows").len(), 3);
    let steps = p.poll(&[], At::Never, usize::MAX);
    assert!(steps.is_empty());
    assert_eq!(p.cursor(), Some(UpdateId::new(4)), "queued work stopped holding the barrier");
}

/// Compaction is bounded by what the channel has been *told*, and never forgets an update that
/// cost a model turn.
#[test]
fn compaction_is_bounded_below_the_acknowledged_offset() {
    let dir = Dir::new();
    let world = World::default();
    let p = Process::boot(&world, &dir);
    let batch: Vec<(u64, &str)> = vec![(1, "g1"), (2, "g1")];
    p.poll(&batch, At::Never, usize::MAX);

    // Delivered locally, but the channel has not been told — so both are still redeliverable and
    // forgetting either is how one gets answered twice.
    assert_eq!(p.channel.acknowledged().expect("acked"), None);
    assert_eq!(p.channel.compact().expect("compact"), 0);

    // A later poll carries the offset. Now they are acknowledged — and still kept, because they
    // cost model turns, and their ids are how a duplicate report is diagnosed later.
    p.poll(&[], At::Never, usize::MAX);
    assert_eq!(p.channel.acknowledged().expect("acked"), Some(UpdateId::new(3)));
    assert_eq!(
        p.channel.compact().expect("compact"),
        0,
        "an update that cost a turn was forgotten"
    );
    assert_eq!(p.channel.records().expect("rows").len(), 2);
}

/// **B8** — a crash in the middle of a batch: done stays done, the crashed one never runs again,
/// and the update behind it is answered.
///
/// The reference implementation states the third claim as "untouched runs" and it is the one a
/// batch-shaped design gets wrong: if the batch were the unit of durability, update 3 would be
/// lost with update 2. Here it is journalled before any of them is worked on, so it is open, it
/// holds the barrier, and the next process is handed it.
///
/// The middle update resolves differently from the reference, and better. There it becomes
/// `RECOVERY_BLOCKED`. Here the answer is already durable, so recovery *fetches* it — one extra
/// send of byte-identical text, never a second model execution. That is B5's guarantee arriving
/// inside B8, and it is why the assertion below is on the answer's identity rather than on a
/// blocked state.
#[test]
fn b8_a_crash_mid_batch_leaves_the_done_done_and_the_untouched_still_owed() {
    let world = World::default();
    let dir = Dir::new();
    let batch = [(1u64, "g"), (2, "g"), (3, "g")];

    let first = Process::boot(&world, &dir);
    let steps = first.poll_crashing_at(&batch, At::AfterSend, 1);
    assert_eq!(steps, vec![Step::Handled, Step::Crashed], "the batch stopped at the crash");
    assert_eq!(world.runs(), 2, "update 3 was never started");
    // The barrier is the crashed update, not the batch boundary: 2 is the lowest open one.
    assert_eq!(first.cursor(), Some(UpdateId::new(2)));
    drop(first);

    // A new process, handed the whole batch again because nothing below 2 was acknowledged.
    let second = Process::boot(&world, &dir);
    let redelivered: Vec<(u64, &str)> =
        world.redelivers(&[1, 2, 3]).into_iter().map(|id| (id, "g")).collect();
    assert_eq!(redelivered.len(), 3, "the channel was told an offset it should not have been");
    let steps = second.poll(&redelivered, At::Never, usize::MAX);

    assert_eq!(
        steps,
        vec![Step::AlreadySettled, Step::Handled, Step::Handled],
        "done did not stay done, or the untouched update did not run"
    );
    assert_eq!(world.runs(), 3, "a turn was executed twice");
    assert_eq!(
        world.sends(),
        vec!["ANSWER#1", "ANSWER#2", "ANSWER#2", "ANSWER#3"],
        "the residual duplicate was not identical, or update 3 went unanswered"
    );
    assert_eq!(second.delivery(UpdateId::new(1)), DeliveryState::Delivered);
    assert_eq!(second.delivery(UpdateId::new(3)), DeliveryState::Delivered);
    assert_eq!(second.cursor(), Some(UpdateId::new(4)), "the batch did not clear");
}

/// **B11** — an update whose route vanished while its turn was in flight is parked for an
/// operator, never relabelled as noise, and never re-run.
///
/// The reference implementation reaches this through routing: the row is not terminal and not a
/// delivery-recovery state, so it fell through to the addressing test and was recorded `IGNORED`
/// — no error code, cursor advanced past it, uncountable, unrecoverable. Here that outcome is
/// unreachable for two independent reasons, and this test pins both plus the alarm that replaces
/// the missing error code.
#[test]
fn b11_a_route_that_vanished_mid_turn_parks_the_update_and_never_relabels_it() {
    let world = World::default();
    let dir = Dir::new();
    let p = Process::boot(&world, &dir);

    // The turn ran; the channel then refuses the answer, having accepted no part of it.
    let steps = p.poll(&[(1u64, "g")], At::SendRefused, usize::MAX);
    assert_eq!(world.runs(), 1);

    // The alarm. `ResendSafe` rather than `ResendMayDuplicate` is the diagnostic half: it tells
    // the operator that saying yes cannot put a second copy in a client's room.
    assert_eq!(steps, vec![Step::Reconcile(Repair::ResendSafe)]);
    assert!(Repair::ResendSafe.requires_operator(), "a refused send resent itself");
    assert_eq!(p.delivery(UpdateId::new(1)), DeliveryState::KnownUnsent);

    // Never noise. The disposition is write-once, so routing cannot rewrite the history of an
    // update with an answer behind it — this is the `IGNORED` outcome being unrepresentable
    // rather than merely declined.
    let err = p
        .channel
        .disposition(UpdateId::new(1), &Disposition::NotAddressed, "t9")
        .expect_err("an update that cost a turn was relabelled as noise");
    assert!(matches!(err, ChannelError::AlreadyDispositioned { existing: "turn", .. }), "{err:?}");

    // And parked is not open: one update awaiting a human must not deafen the bridge to every
    // conversation behind it. E-9 keeps the alarm; E-18 releases the cursor.
    assert_eq!(p.cursor(), Some(UpdateId::new(2)));
    drop(p);

    // A restart re-reads the same evidence and does not act on it either.
    let again = Process::boot(&world, &dir);
    let steps = again.poll(&[(1u64, "g")], At::Never, usize::MAX);
    assert_eq!(steps, vec![Step::Reconcile(Repair::ResendSafe)]);
    assert_eq!(world.runs(), 1, "the parked turn was re-run");
    assert!(world.sends().is_empty(), "a parked turn was delivered without an operator");
}
