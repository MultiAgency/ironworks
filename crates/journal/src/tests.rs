//! Crash boundaries, derived from the reference corpus (`multi/seam/test_bridge_recovery.py`).
//!
//! Four of the fifteen are in scope for one turn without delivery. B5–B7 — the residual delivery
//! window, the delivered/offset transaction, and the acknowledgement gap — need a channel journal
//! as well as this one, and live in `crates/channel/tests/crash_boundaries.rs`.
//!
//! | Boundary | Crash point | Required behaviour |
//! |---|---|---|
//! | **B1** | before anything is journalled | nothing durable; handled once on retry |
//! | **B2** | key durable, request not sent | outcome UNKNOWN; **never** a second model run |
//! | **B3** | model completed, id not persisted | replay the same key with an identical body |
//! | **B4** | completed and recorded, not delivered | fetch that answer by id; never rerun |
//!
//! B3 is where this implementation deliberately differs: the reference blocks it for operator
//! reconciliation, and ADR 0005 makes it recoverable.

use super::*;
use ironworks_domain::{
    CapabilityProfile, Composition, ContextBinding, CredentialPurpose, CredentialRef, Guidance,
    ModelPin, ModelSelection, NonEmptyReason, Openness, PartRef, PartSource, RuntimeIdentity,
    ScopeEvidence, ScopeFailure, ScopeResolver, ServiceDefinition, ServiceId, ServiceInstance,
};
use std::collections::BTreeMap;

/// The composition these tests pretend to have been built on. Threads record it so a
/// continuation decision can tell "the same conversation" from "a different alloy".
const ALLOY: &str = "aaaaaaaaaaaaaaaa";

/// The one room these tests serve. Threads are keyed by conversation, so a
/// pointer is always asked for by name.
fn room() -> ironworks_domain::ChannelRef {
    ironworks_domain::ChannelRef::new("-100999")
}

const NOW: &str = "2026-08-27T12:00:00+00:00";

/// The same instant, as the type the stores take.
///
/// `now: &str` accepted anything; `ironworks_domain::Utc` is the one parser for the form every
/// timestamp column in this workspace holds — and the form `ORDER BY` rests on where there is one.
fn now() -> ironworks_domain::Utc {
    ironworks_domain::Utc::parse(NOW).expect("the fixture instant is the canonical UTC form")
}

const RETRIEVED: &str = "2026-08-27T11:59:58+00:00";

struct Parts(BTreeMap<String, String>);
impl PartSource for Parts {
    fn read(&self, p: &PartRef) -> Option<&str> {
        self.0.get(p.as_str()).map(String::as_str)
    }
}
struct Store(&'static str);
impl ScopeResolver for Store {
    fn resolve(&self, _: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
        Ok(ScopeEvidence::from_record_store(OrgId::parse(self.0).unwrap(), "http://store.example"))
    }
}

/// A verified instance, because a `ThreadScope` cannot be obtained any other way.
fn scope(slug: &str, org: &'static str) -> ThreadScope {
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
        &format!(
            "<!-- tenant-guidance v1 slug: {slug} service: s -->\n\n{}",
            "content ".repeat(80)
        ),
        slug,
        "s",
    )
    .unwrap();
    ServiceInstance::new(ironworks_domain::InstanceParts {
        slug: Slug::parse(slug).unwrap(),
        definition,
        guidance,
        runtime_identity: RuntimeIdentity::parse("m").unwrap(),
        runtime_credential: CredentialRef::new(CredentialPurpose::RuntimeMember, "env:A"),
        context_binding: ContextBinding::new(CredentialRef::new(
            CredentialPurpose::RecordScope,
            "env:B",
        ))
        .unwrap(),
        model: ModelSelection::from_pin(&ModelPin::parse("M # c").unwrap()),
        requested_capabilities: CapabilityProfile::READ_ONLY_RECORDS,
        claimed_org: None,
        parts: &parts,
    })
    .unwrap()
    .verify(&Store(org))
    .unwrap()
    .thread_scope()
}

/// Removed on drop, siblings included — a `-wal` left behind is a database the next run inherits.
///
/// `pub(crate)` so the crate has one temp-database fixture: the sibling `progress` tests use this
/// rather than a second copy of the cleanup list, which is the part that must not drift.
pub(crate) struct Tmp(pub(crate) std::path::PathBuf);
impl Tmp {
    pub(crate) fn new(name: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("ironworks-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Self(p)
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

#[test]
fn an_online_snapshot_is_readable_and_never_overwrites() {
    let live = Tmp::new("snapshot-live");
    let copy = Tmp::new("snapshot-copy");
    let journal = Journal::open(live.path()).unwrap();
    let key = key("snapshot-turn");
    journal
        .begin(
            &scope("acme", "acme-sales"),
            &key,
            RETRIEVED,
            &body("request"),
            &Decision::default(),
            &now(),
        )
        .unwrap();

    journal.snapshot_to(copy.path()).unwrap();
    assert!(Journal::open_readonly(copy.path()).unwrap().turn(&key).unwrap().is_some());
    assert!(matches!(journal.snapshot_to(copy.path()), Err(JournalError::Snapshot { .. })));
}

pub(crate) fn key(v: &str) -> IdempotencyKey {
    IdempotencyKey::parse(v).unwrap()
}
/// The ordinary case: the runtime named the model the request asked for.
fn pinned_model() -> ReportedModel {
    ReportedModel::read(Some("qwen/qwen3.5-397b-a17b"))
}

fn rid(v: &str) -> ResponseId {
    ResponseId::parse(v).unwrap()
}
fn body(v: &str) -> RequestDigest {
    RequestDigest::of(v.as_bytes())
}

/// What a turn decided survives the process that decided it.
///
/// Two-sided, because the failure this closes was a *fresh-looking* rebuild of a chained turn: a
/// test that only checked the fresh case would have passed throughout the whole defect
/// (`docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`).
#[test]
fn a_turns_decision_is_durable_in_both_directions() {
    let t = Tmp::new("decision");
    let j = Journal::open(t.path()).unwrap();
    let sc = scope("acme", "acme-sales");

    // Fresh, book supplied in full: both halves are `None`, and `None` is a value here.
    let fresh = key("t-fresh");
    j.begin(&sc, &fresh, RETRIEVED, &body("A"), &Decision::default(), &now()).unwrap();
    let got = j.turn(&fresh).unwrap().unwrap().decision;
    assert_eq!(got, Decision { previous: None, carried: None });

    // Chained, carrying ten records by thread.
    let chained = key("t-chained");
    let decided = Decision { previous: Some(rid("resp_prior")), carried: Some(10) };
    j.begin(&sc, &chained, RETRIEVED, &body("B"), &decided, &now()).unwrap();
    assert_eq!(j.turn(&chained).unwrap().unwrap().decision, decided);

    // And the two do not bleed into one another: the second write must not have rewritten the
    // first, which is the shape of bug a shared-connection store makes easy.
    assert_eq!(j.turn(&fresh).unwrap().unwrap().decision, Decision::default());
}

/// A key names no tenant, so a row found by key alone may belong to another one.
///
/// `upd-{id}` is minted from a channel update id and one journal holds every tenant's turns, so
/// the same key is *representable* under two instances. It is reached when an operator re-points a
/// channel at another tenant while an update for the first is still in flight: routing then hands
/// the in-flight key to the new tenant, whose recovery would fetch the old tenant's response under
/// its own credential and commit a thread pointing at it.
///
/// `ForeignThread` cannot defend this — it is inside `build_turn`, and a recovery holding a
/// response id returns before a turn is built. Both axes are asserted separately so a check that
/// compared only the slug, or only the org, still fails here.
#[test]
fn a_turn_belongs_only_to_the_instance_and_org_that_wrote_it() {
    let t = Tmp::new("foreign-turn");
    let j = Journal::open(t.path()).unwrap();
    let ours = scope("acme", "acme-sales");
    let k = key("upd-7");
    j.begin(&ours, &k, RETRIEVED, &body("A"), &Decision::default(), &now()).unwrap();

    let record = j.turn(&k).unwrap().expect("the row acme wrote");
    assert!(record.belongs_to(&ours), "the writer must recognise its own turn");

    // A different tenant entirely — the migration case.
    assert!(
        !record.belongs_to(&scope("globex", "globex-sales")),
        "another tenant must not adopt this turn under the same key"
    );
    // Same slug, re-pointed credential: the org moved and the slug did not.
    assert!(
        !record.belongs_to(&scope("acme", "acme-support")),
        "a slug match is not enough — the org is what scoped the records"
    );
    // Same org, different slug: two instances of one organization are still two conversations.
    assert!(
        !record.belongs_to(&scope("acme-eu", "acme-sales")),
        "an org match is not enough — the slug is what routing produced"
    );
}

/// Journal a turn up to `stage`, then drop the handle — the process is gone.
fn crash_at(path: &std::path::Path, sc: &ThreadScope, k: &IdempotencyKey, stage: &str) {
    if stage == "before-begin" {
        return;
    }
    let j = Journal::open(path).unwrap();
    j.begin(sc, k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    match stage {
        "before-dispatch" => {}
        "unknown" => j.record_unknown(k, &now()).unwrap(),
        "accepted" => j.record_accepted(k, &rid("resp_001"), &pinned_model(), &now()).unwrap(),
        "completed" => {
            j.record_accepted(k, &rid("resp_001"), &pinned_model(), &now()).unwrap();
            j.commit_completed(k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
                .unwrap();
        }
        other => panic!("unknown stage {other}"),
    }
}

// ── B1 ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn b1_crash_before_journalling_leaves_nothing_durable() {
    let t = Tmp::new("b1");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "before-begin");

    let j = Journal::open(t.path()).unwrap();
    assert_eq!(j.turn(&k).unwrap(), None, "nothing was journalled, so nothing is recoverable");
    assert!(j.unresolved().unwrap().is_empty());
    assert_eq!(j.prior_thread(&sc, &room()).unwrap(), None, "no pointer moved");
}

// ── B2 ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn b2_a_key_durable_before_dispatch_is_dispatched_with_the_same_key() {
    let t = Tmp::new("b2");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "before-dispatch");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().expect("the recovery handle was recorded before the request");
    assert_eq!(rec.outcome, TurnOutcome::NotSubmitted);
    match recover(&rec) {
        Recovery::Dispatch { key: k2, retrieved_at } => {
            assert_eq!(k2, k, "a retry must reuse the key, or it can create a second turn");
            assert_eq!(retrieved_at, RETRIEVED, "and the timestamp, or the body differs");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn b2_an_ambiguous_dispatch_is_never_treated_as_unsent() {
    let t = Tmp::new("b2u");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "unknown");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().unwrap();
    assert_eq!(rec.outcome, TurnOutcome::SubmittedUnknown);
    assert!(
        !matches!(recover(&rec), Recovery::Dispatch { .. }),
        "dispatching a may-have-run turn bills a second execution"
    );
}

// ── B3: where this implementation improves on the reference ─────────────────────────────

#[test]
fn b3_a_completed_but_unrecorded_turn_replays_the_same_key_with_an_identical_body() {
    let t = Tmp::new("b3");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "unknown");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().unwrap();
    match recover(&rec) {
        Recovery::ReplayIdentical { key: k2, retrieved_at, expect } => {
            assert_eq!(k2, k);
            assert_eq!(retrieved_at, RETRIEVED);
            // The rebuild reuses the stored timestamp, so it reproduces the original bytes.
            assert_eq!(expect, body("REQUEST"));
            assert!(rec.may_replay(&body("REQUEST")));
        }
        other => panic!("expected a replay, got {other:?}"),
    }
}

#[test]
fn b3_a_body_that_cannot_be_rebuilt_identically_may_not_be_replayed() {
    // THIS is the blocked case — not the crash. The runtime refuses a changed body under an
    // existing key, so a mismatch cannot work; checking locally turns a remote 409 into a
    // decision we can act on.
    let t = Tmp::new("b3m");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "unknown");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().unwrap();
    assert!(!rec.may_replay(&body("A DIFFERENT REQUEST")));
}

// ── B4 ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn b4_a_known_response_is_fetched_never_regenerated() {
    let t = Tmp::new("b4");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "accepted");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().unwrap();
    assert_eq!(rec.outcome, TurnOutcome::Accepted { response: rid("resp_001") });
    assert_eq!(recover(&rec), Recovery::Fetch { response: rid("resp_001") });
    assert!(
        !matches!(recover(&rec), Recovery::Dispatch { .. } | Recovery::ReplayIdentical { .. }),
        "a known response id must never license another submission"
    );
}

#[test]
fn a_completed_turn_delivers_the_stored_answer() {
    let t = Tmp::new("done");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "completed");

    let j = Journal::open(t.path()).unwrap();
    let rec = j.turn(&k).unwrap().unwrap();
    assert_eq!(recover(&rec), Recovery::Deliver { response: rid("resp_001") });
    assert_eq!(
        j.prior_thread(&sc, &room()).unwrap().unwrap().pointer.response().as_str(),
        "resp_001"
    );
}

// ── one thread per conversation ──────────────────────────────────────────────────────────

#[test]
fn two_rooms_of_one_tenant_keep_separate_threads() {
    // Schema 2 keyed threads by instance alone. Nothing read them — `cli` never chains, so
    // every turn is Fresh — so no conversation was ever crossed. The row was still written under
    // the wrong key, and the leak would have surfaced later attached to continuation: a second
    // room's turn chaining from the first room's response, with IronClaw replaying the history.
    let t = Tmp::new("rooms");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();
    let a = ironworks_domain::ChannelRef::new("-100111");
    let b = ironworks_domain::ChannelRef::new("-100222");

    let ka = key("turn-a");
    j.begin(&sc, &ka, RETRIEVED, &body("ASK A"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&ka, &sc.pointer(rid("resp_aaa")), Some(&a), ALLOY, None, &now()).unwrap();

    let kb = key("turn-b");
    j.begin(&sc, &kb, RETRIEVED, &body("ASK B"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&kb, &sc.pointer(rid("resp_bbb")), Some(&b), ALLOY, None, &now()).unwrap();

    assert_eq!(j.prior_thread(&sc, &a).unwrap().unwrap().pointer.response().as_str(), "resp_aaa");
    assert_eq!(
        j.prior_thread(&sc, &b).unwrap().unwrap().pointer.response().as_str(),
        "resp_bbb",
        "the second room must not have overwritten the first, nor inherited it"
    );
    // The positive control for the two above: an unrelated room has no thread at all, so the
    // assertions are about keying rather than about the store being empty.
    assert_eq!(j.prior_thread(&sc, &ironworks_domain::ChannelRef::new("-100333")).unwrap(), None);
}

#[test]
fn a_turn_with_no_conversation_writes_no_thread() {
    // `ironworks turn` answers on a terminal. There is no room to continue in, so a pointer would
    // be one waiting to be handed to whichever caller asked first.
    let t = Tmp::new("terminal");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), None, ALLOY, None, &now()).unwrap();

    // The turn is complete — the commit is not a no-op — and no thread exists for any room.
    assert!(j.turn(&k).unwrap().unwrap().outcome.is_final());
    assert_eq!(j.prior_thread(&sc, &room()).unwrap(), None);
}

#[test]
fn a_thread_records_what_it_was_built_on_and_counts_completed_turns() {
    // Both are inputs to the continuation decision, and both must survive a restart — which is
    // exactly when a conversation is most likely to be picked up again.
    let t = Tmp::new("counts");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();

    for (n, k) in ["turn-1", "turn-2", "turn-3"].iter().enumerate() {
        let k = key(k);
        j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
        j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
            .unwrap();
        let prior = j.prior_thread(&sc, &room()).unwrap().unwrap();
        assert_eq!(prior.turns, n as u32 + 1, "one count per completed turn");
        assert_eq!(prior.alloy, ALLOY);
    }

    // Reopened: the count is on disk, not in memory.
    drop(j);
    let j = Journal::open(t.path()).unwrap();
    assert_eq!(j.prior_thread(&sc, &room()).unwrap().unwrap().turns, 3);
}

#[test]
fn a_composition_change_starts_the_count_again() {
    // The chain is broken by a moved composition, so the turns before it are not this thread's
    // history any more. Carrying the count forward would retire the new thread early, on the
    // strength of turns nobody can continue from.
    let t = Tmp::new("recount");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();

    for k in ["turn-1", "turn-2"] {
        let k = key(k);
        j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
        j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
            .unwrap();
    }
    assert_eq!(j.prior_thread(&sc, &room()).unwrap().unwrap().turns, 2);

    let k = key("turn-3");
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
    j.commit_completed(
        &k,
        &sc.pointer(rid("resp_002")),
        Some(&room()),
        "bbbbbbbbbbbbbbbb",
        None,
        &now(),
    )
    .unwrap();
    let prior = j.prior_thread(&sc, &room()).unwrap().unwrap();
    assert_eq!(prior.turns, 1, "a new composition is a new thread");
    assert_eq!(prior.alloy, "bbbbbbbbbbbbbbbb");
}

#[test]
fn a_thread_remembers_what_it_was_shown_and_a_degraded_turn_does_not_erase_it() {
    // The `None` case that matters: a turn answered while the store was unreachable supplied
    // nothing, and must leave the mark an earlier turn established. Erasing it would make the
    // next chained turn re-send a book the conversation already holds — or worse, believe it had
    // never been shown one.
    let t = Tmp::new("supplied");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();
    let mark = ironworks_domain::SuppliedMark { catalog: "cat-1".into(), records: 40 };

    let k = key("turn-1");
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, Some(&mark), &now())
        .unwrap();
    assert_eq!(j.prior_thread(&sc, &room()).unwrap().unwrap().supplied, Some(mark.clone()));

    let k2 = key("turn-2");
    j.begin(&sc, &k2, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&k2, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, &now())
        .unwrap();
    let prior = j.prior_thread(&sc, &room()).unwrap().unwrap();
    assert_eq!(prior.supplied, Some(mark), "a turn that supplied nothing kept what was there");
    assert_eq!(prior.turns, 2, "it was still a turn");
}

#[test]
fn a_composition_change_forgets_what_the_old_thread_carried() {
    // The chain is broken, so the records the old thread held are not in the new one. Keeping the
    // mark would tell the next turn the conversation already has a book it has never seen.
    let t = Tmp::new("forget");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();
    let mark = ironworks_domain::SuppliedMark { catalog: "cat-1".into(), records: 40 };

    let k = key("turn-1");
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, Some(&mark), &now())
        .unwrap();

    let k2 = key("turn-2");
    j.begin(&sc, &k2, RETRIEVED, &body("ASK"), &Decision::default(), &now()).unwrap();
    j.commit_completed(
        &k2,
        &sc.pointer(rid("resp_002")),
        Some(&room()),
        "bbbbbbbbbbbbbbbb",
        None,
        &now(),
    )
    .unwrap();
    let prior = j.prior_thread(&sc, &room()).unwrap().unwrap();
    assert_eq!(prior.supplied, None, "a new composition has been shown nothing");
    assert_eq!(prior.turns, 1);
}

// ── the critical transaction ─────────────────────────────────────────────────────────────

#[test]
fn the_response_id_and_the_pointer_commit_together() {
    let t = Tmp::new("atomic");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();

    // Before the commit: neither exists.
    assert_eq!(j.turn(&k).unwrap().unwrap().outcome.response(), None);
    assert_eq!(j.prior_thread(&sc, &room()).unwrap(), None);

    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
        .unwrap();

    // After: both.
    assert!(j.turn(&k).unwrap().unwrap().outcome.is_final());
    assert_eq!(
        j.prior_thread(&sc, &room()).unwrap().unwrap().pointer.response().as_str(),
        "resp_001"
    );
}

#[test]
fn a_refused_commit_writes_neither_half() {
    let t = Tmp::new("half");
    let sc = scope("acme", "acme-sales");
    let other = scope("globex", "globex-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();

    // A pointer from another instance: upstream treats a cross-wired continuation as undefined
    // behaviour rather than refusing it, so this is the only place it can be caught.
    let err = j
        .commit_completed(&k, &other.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
        .unwrap_err();
    assert!(matches!(err, JournalError::ForeignPointer { .. }));

    assert_eq!(j.turn(&k).unwrap().unwrap().outcome, TurnOutcome::NotSubmitted, "turn untouched");
    assert_eq!(j.prior_thread(&sc, &room()).unwrap(), None, "no pointer moved");
    assert_eq!(j.prior_thread(&other, &room()).unwrap(), None);
}

// ── a known response is never unlearned ──────────────────────────────────────────────────

#[test]
fn an_accepted_turn_cannot_regress_to_unknown_or_unsent() {
    let t = Tmp::new("regress");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_accepted(&k, &rid("resp_001"), &pinned_model(), &now()).unwrap();

    for attempt in [j.record_unknown(&k, &now()), j.record_unsent(&k, &now())] {
        assert!(
            matches!(attempt, Err(JournalError::ResponseWouldChange { .. })),
            "regressing past a known response id would license a second billed turn"
        );
    }
    assert_eq!(
        j.turn(&k).unwrap().unwrap().outcome,
        TurnOutcome::Accepted { response: rid("resp_001") }
    );
}

#[test]
fn a_response_id_cannot_be_replaced_by_a_different_one() {
    let t = Tmp::new("swap");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_accepted(&k, &rid("resp_001"), &pinned_model(), &now()).unwrap();
    assert!(matches!(
        j.record_accepted(&k, &rid("resp_999"), &pinned_model(), &now()),
        Err(JournalError::ResponseWouldChange { .. })
    ));
}

// ── durability and inspection ────────────────────────────────────────────────────────────

#[test]
fn begin_is_idempotent_so_a_redelivered_turn_keeps_its_state() {
    let t = Tmp::new("idem");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_unknown(&k, &now()).unwrap();
    j.begin(&sc, &k, "2026-01-01T00:00:00+00:00", &body("OTHER"), &Decision::default(), &now())
        .unwrap();

    let rec = j.turn(&k).unwrap().unwrap();
    assert_eq!(rec.outcome, TurnOutcome::SubmittedUnknown, "a repeat must not reset the state");
    assert_eq!(rec.retrieved_at, RETRIEVED, "nor rewrite what the original body was built from");
}

#[test]
fn state_survives_reopening() {
    let t = Tmp::new("reopen");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "accepted");
    let j = Journal::open(t.path()).unwrap();
    assert_eq!(j.turn(&k).unwrap().unwrap().outcome.response().unwrap().as_str(), "resp_001");
}

#[test]
fn inspection_is_side_effect_free() {
    let t = Tmp::new("ro");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    crash_at(t.path(), &sc, &k, "accepted");

    let ro = Journal::open_readonly(t.path()).unwrap();
    assert!(!ro.is_writable());
    // Reads work.
    assert!(ro.turn(&k).unwrap().is_some());
    assert_eq!(ro.unresolved().unwrap().len(), 1);
    // Writes do not — refused before SQLite is even asked.
    assert!(matches!(ro.record_unknown(&k, &now()), Err(JournalError::ReadOnly)));
    assert!(matches!(
        ro.begin(&sc, &key("t2"), RETRIEVED, &body("X"), &Decision::default(), &now()),
        Err(JournalError::ReadOnly)
    ));
    assert!(matches!(
        ro.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now()),
        Err(JournalError::ReadOnly)
    ));
    // And the state is exactly as it was.
    assert_eq!(
        ro.turn(&k).unwrap().unwrap().outcome,
        TurnOutcome::Accepted { response: rid("resp_001") }
    );
}

#[test]
fn inspection_does_not_create_a_missing_journal() {
    let t = Tmp::new("ro-missing");
    assert!(!t.path().exists());
    assert!(Journal::open_readonly(t.path()).is_err());
    assert!(!t.path().exists(), "read-only inspection created the journal it reported on");
}

#[test]
fn unresolved_lists_everything_that_is_not_final() {
    let t = Tmp::new("unres");
    let sc = scope("acme", "acme-sales");
    let j = Journal::open(t.path()).unwrap();
    for (k, stage) in [("a", "before-dispatch"), ("b", "unknown"), ("c", "accepted")] {
        let kk = key(k);
        j.begin(&sc, &kk, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
        match stage {
            "unknown" => j.record_unknown(&kk, &now()).unwrap(),
            "accepted" => {
                j.record_accepted(&kk, &rid("resp_001"), &pinned_model(), &now()).unwrap()
            }
            _ => {}
        }
    }
    // Completed, but the answer has not been sent. On the turn axis this is finished; on the
    // delivery axis it has not started, and an operator asking "what is outstanding?" is asking
    // about both. Collapsing the two axes is what let a completed-but-never-delivered turn read
    // as resolved.
    let answered = key("d");
    j.begin(&sc, &answered, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&answered, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, &now())
        .unwrap();

    // Completed and delivered. The only combination that is actually done.
    let done = key("e");
    j.begin(&sc, &done, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&done, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, &now())
        .unwrap();
    j.record_delivery_attempt(&done, AttemptOutcome::Complete, &now()).unwrap();

    // Settled for an operator, reached from `completed` — a turn that answered with something
    // undeliverable. It has released the cursor and it is still outstanding, and those are
    // different obligations: releasing must never silence the alarm (§E-18 against §E-9).
    let parked = key("f");
    j.begin(&sc, &parked, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.commit_completed(&parked, &sc.pointer(rid("resp_003")), Some(&room()), ALLOY, None, &now())
        .unwrap();
    j.record_operator_required(&parked, &NonEmptyReason::parse("undeliverable").unwrap(), &now())
        .unwrap();
    assert_eq!(j.openness(&parked), Openness::Settled, "it must not still hold the cursor");

    let unresolved: Vec<String> =
        j.unresolved().unwrap().into_iter().map(|r| r.key.as_str().to_owned()).collect();
    assert_eq!(unresolved, ["a", "b", "c", "d", "f"], "only completed AND delivered drops out");
}

#[test]
fn an_unknown_schema_version_refuses_to_open() {
    let t = Tmp::new("ver");
    {
        let j = Journal::open(t.path()).unwrap();
        let conn = j.conn.lock().unwrap();
        conn.execute("UPDATE meta SET value='99' WHERE key='schema_version'", []).unwrap();
    }
    // Bound to the constant rather than a literal: this test failed on the version-3 bump for no
    // reason but its own hardcoded 2, which is the drift it exists to catch in the store.
    assert!(matches!(
        Journal::open(t.path()),
        Err(JournalError::Version { found: 99, expected }) if expected == SCHEMA_VERSION
    ));
    assert!(matches!(
        Journal::open_readonly(t.path()),
        Err(JournalError::Version { found: 99, expected }) if expected == SCHEMA_VERSION
    ));
}

fn attribution() -> CapabilityAttribution {
    CapabilityAttribution {
        profile_sha256: Sha256Digest::digest(b"profile", b"one"),
        policy_sha256: Sha256Digest::digest(b"policy", b"one"),
        catalogue_sha256: Sha256Digest::digest(b"catalogue", b"one"),
        observed_at: Utc::parse("2026-09-06T12:00:00+00:00").unwrap(),
    }
}

#[test]
fn attributed_turns_round_trip_all_four_fields_as_one_value() {
    let t = Tmp::new("attributed");
    let journal = Journal::open(t.path()).unwrap();
    let key = key("attributed-turn");
    let expected = attribution();
    journal
        .begin_attributed(
            &scope("acme", "acme-sales"),
            &key,
            AttributedAdmission {
                retrieved_at: RETRIEVED,
                digest: &body("REQUEST"),
                decision: &Decision::default(),
                attribution: &expected,
                now: &now(),
            },
        )
        .unwrap();
    assert_eq!(journal.turn(&key).unwrap().unwrap().attribution, Some(expected));
}

#[test]
fn version_seven_migrates_existing_turns_to_an_unclaimed_model() {
    // One step back is migratable, so this moves with SCHEMA_VERSION: it used to drop the four
    // confinement columns and assert a v6 journal came back explicitly unattributed. A v6 journal
    // is now refused rather than migrated, which is the same one-step policy applied a version
    // later, not a new one.
    let t = Tmp::new("migrate-v7");
    let key = key("historical-turn");
    {
        let journal = Journal::open(t.path()).unwrap();
        journal
            .begin(
                &scope("acme", "acme-sales"),
                &key,
                RETRIEVED,
                &body("REQUEST"),
                &Decision::default(),
                &now(),
            )
            .unwrap();
        let conn = journal.conn.lock().unwrap();
        conn.execute_batch(
            "ALTER TABLE turns DROP COLUMN reported_model;
             UPDATE meta SET value='7' WHERE key='schema_version';",
        )
        .unwrap();
    }

    let migrated = Journal::open(t.path()).unwrap();
    // A row written before the column existed claims nothing, and `Absent` is what that is. It is
    // deliberately not the placeholder: the runtime never said "reborn" here, it said nothing.
    assert_eq!(migrated.turn(&key).unwrap().unwrap().reported_model, ReportedModel::Absent);
    let conn = migrated.conn.lock().unwrap();
    let version: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION.to_string());
}

/// The state a crash between the migration and its version write used to leave: the column added,
/// version 7 still recorded, and every later open refused with `duplicate column name`, for good.
/// The trigger stands in for the crash — it fails the version write and nothing else.
#[test]
fn a_version_seven_migration_interrupted_before_its_version_write_reopens_and_migrates() {
    let t = Tmp::new("migrate-v7-interrupted");
    {
        let journal = Journal::open(t.path()).unwrap();
        let conn = journal.conn.lock().unwrap();
        conn.execute_batch(
            "ALTER TABLE turns DROP COLUMN reported_model;
             UPDATE meta SET value='7' WHERE key='schema_version';
             CREATE TRIGGER crash_before_the_version BEFORE UPDATE ON meta
             BEGIN SELECT RAISE(ABORT, 'crashed before the version write'); END;",
        )
        .unwrap();
    }
    let has_column = |conn: &Connection| {
        conn.prepare("SELECT 1 FROM pragma_table_info('turns') WHERE name='reported_model'")
            .unwrap()
            .exists([])
            .unwrap()
    };
    let version = |conn: &Connection| -> String {
        conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| row.get(0))
            .unwrap()
    };

    assert!(Journal::open(t.path()).is_err(), "the version write was made to fail");
    {
        let conn = Connection::open(t.path()).unwrap();
        assert!(!has_column(&conn), "the ALTER must go down with the version write");
        assert_eq!(version(&conn), "7");
        conn.execute_batch("DROP TRIGGER crash_before_the_version").unwrap();
    }

    let migrated = Journal::open(t.path()).expect("the interrupted migration re-runs cleanly");
    let conn = migrated.conn.lock().unwrap();
    // The positive control for the absence above: the same probe sees the column once it is there.
    assert!(has_column(&conn));
    assert_eq!(version(&conn), SCHEMA_VERSION.to_string());
}

/// The journal outlives a crash, so anything in it outlives a crash. It must hold identifiers
/// and nothing else.
#[test]
fn the_journal_holds_no_content() {
    let t = Tmp::new("content");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    // The body these sentinels stand for is never handed to the journal — only its digest.
    j.begin(
        &sc,
        &k,
        RETRIEVED,
        &body("SECRET-TOKEN-VALUE and a MESSAGE-FROM-A-PERSON"),
        &Decision::default(),
        &now(),
    )
    .unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, &now())
        .unwrap();
    drop(j);

    let bytes = std::fs::read(t.path()).unwrap();
    let haystack = String::from_utf8_lossy(&bytes);
    for sentinel in ["SECRET-TOKEN-VALUE", "MESSAGE-FROM-A-PERSON"] {
        assert!(!haystack.contains(sentinel), "{sentinel} reached the journal");
    }
    // What it does hold: identifiers.
    assert!(haystack.contains("turn-1") && haystack.contains("resp_001"));
}

// ── automatic work stops, the alarm does not ────────────────────────────────────────────────

/// The state that keeps one stuck conversation from deafening the channel, and the alarm that
/// must outlive it. Both halves matter: settling it releases the shared cursor, and `unresolved`
/// is where an operator still finds it.
#[test]
fn operator_required_settles_the_cursor_and_keeps_the_alarm() {
    let t = Tmp::new("opreq");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_accepted(&k, &rid("resp_001"), &pinned_model(), &now()).unwrap();
    assert_eq!(j.openness(&k), Openness::Open, "an accepted turn still has work to do");

    let why = NonEmptyReason::parse("the request body cannot be rebuilt byte-identically").unwrap();
    j.record_operator_required(&k, &why, &now()).unwrap();

    let record = j.turn(&k).unwrap().unwrap();
    // The response id survives. It and the delivery evidence beside it are how a client's "I got
    // two answers" report is diagnosed days later.
    assert_eq!(
        record.outcome,
        TurnOutcome::OperatorRequired { response: Some(rid("resp_001")), reason: why.clone() }
    );
    // Settled, so the shared cursor is released — the whole point of the state.
    assert_eq!(j.openness(&k), Openness::Settled);
    // And still outstanding, so the alarm is not silenced by the same move.
    let unresolved: Vec<String> =
        j.unresolved().unwrap().into_iter().map(|r| r.key.as_str().to_owned()).collect();
    assert_eq!(unresolved, ["turn-1"]);
    assert!(matches!(recover(&record), Recovery::OperatorRequired { .. }));
}

/// A turn can complete and still hold nothing a client may be shown — a control token in the
/// answer, which `ironworks_ironclaw::malformed_answer` gates on. Settling that must release the
/// cursor **without** discarding the continuation the same transaction made durable.
///
/// This is the one thing that path could plausibly break. `commit_completed` writes the response
/// id and the thread pointer together or neither (§E-1); `record_operator_required` then moves the
/// turn axis to `operator_required`, and if the thread went with it, the conversation would lose
/// its history because one of its answers was undeliverable. The two axes are independent and this
/// pins that they stay so.
#[test]
fn a_completed_turn_whose_answer_is_undeliverable_settles_without_losing_its_thread() {
    let t = Tmp::new("undeliverable");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-malformed");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_accepted(&k, &rid("resp_777"), &pinned_model(), &now()).unwrap();
    // Completed with a room, so a `threads` row exists to be preserved or lost.
    j.commit_completed(&k, &sc.pointer(rid("resp_777")), Some(&room()), ALLOY, None, &now())
        .unwrap();

    // The positive control, before anything settles: a completed-but-undelivered turn IS open, so
    // the assertion below is about the settle rather than about a row that was never open.
    assert_eq!(j.openness(&k), Openness::Open, "a completed, undelivered turn still owes a send");
    let before = j.prior_thread(&sc, &room()).unwrap().expect("the turn committed a thread");
    assert_eq!(before.pointer.response().as_str(), "resp_777");

    let why = NonEmptyReason::parse(
        "the runtime returned a model control token instead of an answer. The response is \
         complete and will read the same on every cycle.",
    )
    .unwrap();
    j.record_operator_required(&k, &why, &now()).unwrap();

    // Settled: the cursor is released, which is the whole point.
    assert_eq!(j.openness(&k), Openness::Settled);
    let record = j.turn(&k).unwrap().unwrap();
    assert_eq!(
        record.outcome,
        TurnOutcome::OperatorRequired { response: Some(rid("resp_777")), reason: why },
        "the response id is half the evidence and must survive the settle"
    );

    // The thread is untouched. The turn ran and its answer is real history in the runtime's own
    // conversation, whatever we decided about showing it.
    let after = j.prior_thread(&sc, &room()).unwrap().expect("settling orphaned the thread");
    assert_eq!(after.pointer.response().as_str(), "resp_777");
    assert_eq!(after.alloy, before.alloy);

    // And the alarm outlives the release (§E-9 keeps it, §E-18 releases the cursor).
    let unresolved: Vec<String> =
        j.unresolved().unwrap().into_iter().map(|r| r.key.as_str().to_owned()).collect();
    assert_eq!(unresolved, ["turn-malformed"]);
    assert!(matches!(recover(&record), Recovery::OperatorRequired { .. }));
}

#[test]
fn an_alarm_with_no_content_cannot_be_constructed() {
    // The reason IS the alarm. A blank one still shows in a count and still fails a gate, while
    // telling whoever is paged nothing — which reads like an alarm somebody already handled.
    for blank in ["", "   ", "\n\t "] {
        assert!(NonEmptyReason::parse(blank).is_err(), "{blank:?}");
    }
    assert_eq!(NonEmptyReason::parse("  a real reason  ").unwrap().as_str(), "a real reason");
}

/// A row that lost its reason is refused rather than read as a blank one. The construction check
/// cannot cover this: the corruption happens after construction, on disk.
#[test]
fn an_operator_required_row_without_its_reason_is_refused() {
    let t = Tmp::new("noreason");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_operator_required(&k, &NonEmptyReason::parse("a reason").unwrap(), &now()).unwrap();
    {
        let conn = j.conn.lock().unwrap();
        conn.execute("UPDATE turns SET operator_reason=NULL WHERE key='turn-1'", []).unwrap();
    }
    assert!(j.turn(&k).is_err(), "an alarm with no content was read back as valid");
}

#[cfg(unix)]
fn mode_of(p: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).expect("stat the store").permissions().mode() & 0o777
}

/// The turn journal names every organization served and every turn outstanding. It carries no
/// message text — that is asserted separately — but the client list is itself the thing worth
/// keeping off a shared machine.
#[cfg(unix)]
#[test]
fn the_turn_journal_is_owner_only() {
    let t = Tmp::new("mode");
    let _j = Journal::open(t.path()).expect("open");
    assert_eq!(mode_of(t.path()), 0o600, "the turn journal is group- or world-readable");
}

/// B10 for the turn journal, where the guidance matters most: this file is the only record of
/// which turns were billed.
#[test]
fn a_turn_journal_that_is_not_a_database_refuses_and_says_not_to_delete_it() {
    let t = Tmp::new("corrupt");
    std::fs::write(t.path(), b"this is not a database".repeat(10)).expect("write");
    let err = Journal::open(t.path()).expect_err("a corrupt journal was opened");
    assert!(matches!(err, JournalError::NotADatabase { .. }), "{err:?}");
    assert!(err.to_string().contains("do NOT delete it blind"), "{err}");
}

/// The outcome and its reason become durable together, or neither does.
///
/// `record_operator_required` used to write twice: `set_outcome` committed
/// `outcome='operator_required'` and released the lock, and a second `UPDATE` then wrote
/// `operator_reason`. A crash in that gap left a row `row_to_record` refuses — *"operator-required
/// row without a reason"* — and because `unresolved` collects into one `Result`, that single row
/// made the **whole operator alarm list** unreadable. A state whose entire purpose is to raise an
/// alarm could silence every alarm at once.
///
/// The two writes now share a transaction, so this code cannot produce that row. **Refusing to
/// read one that exists anyway is deliberate**, and the schema says so: *"it is the operator-visible
/// alarm, so nothing may age it away and a row that has lost it is refused rather than read as a
/// blank."* The half-written state is asserted here as unreachable, not as survivable.
#[test]
fn the_operator_reason_is_durable_with_its_outcome_or_not_at_all() {
    let t = Tmp::new("opreq-atomic");
    let sc = scope("acme", "acme-sales");
    let k = key("turn-1");
    let j = Journal::open(t.path()).unwrap();
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), &now()).unwrap();
    j.record_accepted(&k, &rid("resp_001"), &pinned_model(), &now()).unwrap();

    // Before: neither half is set, so the pair below is a move and not a coincidence.
    assert_eq!(columns(t.path(), &k), (Some("accepted".to_owned()), None));

    let why = NonEmptyReason::parse("the request body cannot be rebuilt byte-identically").unwrap();
    j.record_operator_required(&k, &why, &now()).unwrap();

    // After: both halves moved, in one commit. Read straight from the columns rather than through
    // `row_to_record`, which refuses the mixed state and so could not tell these two apart.
    assert_eq!(
        columns(t.path(), &k),
        (Some("operator_required".to_owned()), Some(why.as_str().to_owned())),
        "the outcome and its reason did not move together"
    );
    assert_eq!(j.unresolved().unwrap().len(), 1, "the alarm is not listed");

    // A failure before the transaction commits leaves both halves alone. `NoSuchTurn` is the
    // reachable one, and the control is that the row above is still exactly as it was.
    let absent = key("turn-absent");
    assert!(j.record_operator_required(&absent, &why, &now()).is_err());
    assert_eq!(
        columns(t.path(), &k),
        (Some("operator_required".to_owned()), Some(why.as_str().to_owned())),
        "a failed call on another key disturbed this row"
    );
}

/// `(outcome, operator_reason)` straight from the table.
///
/// Deliberately not via `Journal::turn`: `row_to_record` refuses a row whose reason is missing, so
/// reading through it cannot distinguish "both written" from "outcome written, reason not" — which
/// is the exact pair the test above exists to tell apart.
fn columns(path: &std::path::Path, k: &IdempotencyKey) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT outcome, operator_reason FROM turns WHERE key=?1",
        params![k.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

/// A damaged `schema_version` must be refused, not read as "no version recorded".
///
/// The `None` arm of `Journal::open` stamps the current version onto whatever it finds, so a
/// value that folded into `None` on a parse failure would declare a database of unknown shape to
/// be this one. `crates/channel/src/lib.rs::parse_acked` refuses one field over for the same
/// reason, and this journal is the record of which turns ran and which answers were billed.
#[test]
fn a_damaged_schema_version_is_refused_rather_than_read_as_absent() {
    let tmp = Tmp::new("schema-version");
    let journal = Journal::open(tmp.path()).unwrap();
    drop(journal);

    // The positive control, first: an untouched store re-opens. Without it this test would pass
    // against an `open` that refused everything.
    Journal::open(tmp.path()).expect("an untouched journal re-opens");

    let conn = rusqlite::Connection::open(tmp.path()).unwrap();
    conn.execute("UPDATE meta SET value='seven' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    match Journal::open(tmp.path()) {
        Err(JournalError::UnreadableVersion { raw }) => assert_eq!(raw, "seven"),
        other => panic!("a damaged schema version must be refused, got {other:?}"),
    }
}

/// The decode does not depend on the order the columns are selected in.
///
/// # What this is protecting
///
/// `TURN_COLUMNS` used to be written out twice — once in `Journal::turn` and once in
/// `Journal::unresolved` — while `row_to_record` read them as `r.get(0)` … `r.get(n)`. Nothing
/// coupled the lists to the offsets. `instance` and `org` are adjacent and both `TEXT`, so
/// swapping that one pair in one of the two lists would have compiled, run, and filed a turn under
/// the wrong tenant, silently.
///
/// Selecting the same columns **reversed** is the cheapest statement of the property: it fails
/// against any positional decode and passes against a decode that asks by name.
///
/// The list is checked against `turns` itself rather than against a written-down count. A column
/// added to the table and not to `TURN_COLUMNS` is the other half of the same defect — `reported_model`
/// arrived that way, and `row_to_record` failed with `InvalidColumnName` only because the decode
/// asks by name.
#[test]
fn a_turn_decodes_the_same_whatever_order_its_columns_are_selected_in() {
    let t = Tmp::new("column-order");
    let j = Journal::open(t.path()).unwrap();
    let sc = scope("acme", "acme-sales");
    let k = key("t-order");
    j.begin(&sc, &k, RETRIEVED, &body("A"), &Decision::default(), &now()).unwrap();
    let expected = j.turn(&k).unwrap().expect("the turn was written");

    let conn = rusqlite::Connection::open(t.path()).unwrap();
    let reversed: String = {
        let mut names: Vec<&str> = super::TURN_COLUMNS.split(',').map(str::trim).collect();
        let mut declared: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('turns')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        declared.sort();
        let mut selected: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        selected.sort();
        assert_eq!(
            selected, declared,
            "TURN_COLUMNS and the turns table must name the same columns"
        );
        names.reverse();
        names.join(",")
    };
    let got = conn
        .query_row(
            &format!("SELECT {reversed} FROM turns WHERE key=?1"),
            rusqlite::params![k.as_str()],
            super::row_to_record,
        )
        .expect("the reversed selection must decode");

    assert_eq!(got.instance, expected.instance);
    assert_eq!(got.org, expected.org);
    assert_eq!(got.key, expected.key);
    assert_eq!(got.request_digest, expected.request_digest);
    assert_eq!(got.retrieved_at, expected.retrieved_at);

    // The pair that made this worth writing: they are adjacent, both TEXT, and both parse — so a
    // transposition is invisible to the compiler and to every other assertion above.
    assert_ne!(got.instance.as_str(), got.org.as_str(), "the fixture must distinguish the two");
}

#[test]
fn the_reported_model_round_trips_and_a_placeholder_is_not_a_name() {
    // Acceptance is the only moment the model is knowable (§G-2), so this is the only write that
    // can carry it — and the three cases must survive the round trip as themselves. Collapsing
    // `Placeholder` into a name is the defect this column exists to prevent: it would record
    // "reborn" as the model that served a tenant.
    let t = Tmp::new("reported-model");
    let j = Journal::open(t.path()).unwrap();
    let sc = scope("acme", "acme-sales");

    for (name, model) in [
        ("ironworks-a-00000000000000a1", ReportedModel::read(Some("qwen/qwen3.5-397b-a17b"))),
        ("ironworks-a-00000000000000a2", ReportedModel::Placeholder),
        ("ironworks-a-00000000000000a3", ReportedModel::Absent),
    ] {
        let k = key(name);
        j.begin(&sc, &k, RETRIEVED, &body(name), &Decision::default(), &now()).unwrap();
        j.record_accepted(&k, &rid("resp_001"), &model, &now()).unwrap();
        let back = j.turn(&k).unwrap().unwrap().reported_model;
        assert_eq!(back, model, "{name} did not round trip");
        assert_eq!(back.is_measurement(), matches!(model, ReportedModel::Named(_)));
    }
}
