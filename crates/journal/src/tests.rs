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
    ServiceInstance::new(
        Slug::parse(slug).unwrap(),
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
    .verify(&Store(org))
    .unwrap()
    .thread_scope()
}

struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(name: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("ironworks-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
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

fn key(v: &str) -> IdempotencyKey {
    IdempotencyKey::parse(v).unwrap()
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
    j.begin(&sc, &fresh, RETRIEVED, &body("A"), &Decision::default(), NOW).unwrap();
    let got = j.turn(&fresh).unwrap().unwrap().decision;
    assert_eq!(got, Decision { previous: None, carried: None });

    // Chained, carrying ten records by thread.
    let chained = key("t-chained");
    let decided = Decision { previous: Some(rid("resp_prior")), carried: Some(10) };
    j.begin(&sc, &chained, RETRIEVED, &body("B"), &decided, NOW).unwrap();
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
    j.begin(&ours, &k, RETRIEVED, &body("A"), &Decision::default(), NOW).unwrap();

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
    j.begin(sc, k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    match stage {
        "before-dispatch" => {}
        "unknown" => j.record_unknown(k, NOW).unwrap(),
        "accepted" => j.record_accepted(k, &rid("resp_001"), NOW).unwrap(),
        "completed" => {
            j.record_accepted(k, &rid("resp_001"), NOW).unwrap();
            j.commit_completed(k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW)
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
    j.begin(&sc, &ka, RETRIEVED, &body("ASK A"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&ka, &sc.pointer(rid("resp_aaa")), Some(&a), ALLOY, None, NOW).unwrap();

    let kb = key("turn-b");
    j.begin(&sc, &kb, RETRIEVED, &body("ASK B"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&kb, &sc.pointer(rid("resp_bbb")), Some(&b), ALLOY, None, NOW).unwrap();

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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), None, ALLOY, None, NOW).unwrap();

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
        j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
        j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW)
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
        j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
        j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW)
            .unwrap();
    }
    assert_eq!(j.prior_thread(&sc, &room()).unwrap().unwrap().turns, 2);

    let k = key("turn-3");
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
    j.commit_completed(
        &k,
        &sc.pointer(rid("resp_002")),
        Some(&room()),
        "bbbbbbbbbbbbbbbb",
        None,
        NOW,
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
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, Some(&mark), NOW)
        .unwrap();
    assert_eq!(j.prior_thread(&sc, &room()).unwrap().unwrap().supplied, Some(mark.clone()));

    let k2 = key("turn-2");
    j.begin(&sc, &k2, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&k2, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, NOW).unwrap();
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
    j.begin(&sc, &k, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, Some(&mark), NOW)
        .unwrap();

    let k2 = key("turn-2");
    j.begin(&sc, &k2, RETRIEVED, &body("ASK"), &Decision::default(), NOW).unwrap();
    j.commit_completed(
        &k2,
        &sc.pointer(rid("resp_002")),
        Some(&room()),
        "bbbbbbbbbbbbbbbb",
        None,
        NOW,
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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();

    // Before the commit: neither exists.
    assert_eq!(j.turn(&k).unwrap().unwrap().outcome.response(), None);
    assert_eq!(j.prior_thread(&sc, &room()).unwrap(), None);

    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW).unwrap();

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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();

    // A pointer from another instance: upstream treats a cross-wired continuation as undefined
    // behaviour rather than refusing it, so this is the only place it can be caught.
    let err = j
        .commit_completed(&k, &other.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW)
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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.record_accepted(&k, &rid("resp_001"), NOW).unwrap();

    for attempt in [j.record_unknown(&k, NOW), j.record_unsent(&k, NOW)] {
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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.record_accepted(&k, &rid("resp_001"), NOW).unwrap();
    assert!(matches!(
        j.record_accepted(&k, &rid("resp_999"), NOW),
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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.record_unknown(&k, NOW).unwrap();
    j.begin(&sc, &k, "2026-01-01T00:00:00+00:00", &body("OTHER"), &Decision::default(), NOW)
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
    assert!(matches!(ro.record_unknown(&k, NOW), Err(JournalError::ReadOnly)));
    assert!(matches!(
        ro.begin(&sc, &key("t2"), RETRIEVED, &body("X"), &Decision::default(), NOW),
        Err(JournalError::ReadOnly)
    ));
    assert!(matches!(
        ro.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW),
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
        j.begin(&sc, &kk, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
        match stage {
            "unknown" => j.record_unknown(&kk, NOW).unwrap(),
            "accepted" => j.record_accepted(&kk, &rid("resp_001"), NOW).unwrap(),
            _ => {}
        }
    }
    // Completed, but the answer has not been sent. On the turn axis this is finished; on the
    // delivery axis it has not started, and an operator asking "what is outstanding?" is asking
    // about both. Collapsing the two axes is what let a completed-but-never-delivered turn read
    // as resolved.
    let answered = key("d");
    j.begin(&sc, &answered, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&answered, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, NOW)
        .unwrap();

    // Completed and delivered. The only combination that is actually done.
    let done = key("e");
    j.begin(&sc, &done, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.commit_completed(&done, &sc.pointer(rid("resp_002")), Some(&room()), ALLOY, None, NOW)
        .unwrap();
    j.record_delivery_attempt(&done, AttemptOutcome::Complete, NOW).unwrap();

    let unresolved: Vec<String> =
        j.unresolved().unwrap().into_iter().map(|r| r.key.as_str().to_owned()).collect();
    assert_eq!(unresolved, ["a", "b", "c", "d"], "only completed AND delivered drops out");
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
        NOW,
    )
    .unwrap();
    j.commit_completed(&k, &sc.pointer(rid("resp_001")), Some(&room()), ALLOY, None, NOW).unwrap();
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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.record_accepted(&k, &rid("resp_001"), NOW).unwrap();
    assert_eq!(j.openness(&k), Openness::Open, "an accepted turn still has work to do");

    let why = NonEmptyReason::parse("the request body cannot be rebuilt byte-identically").unwrap();
    j.record_operator_required(&k, &why, NOW).unwrap();

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
    j.begin(&sc, &k, RETRIEVED, &body("REQUEST"), &Decision::default(), NOW).unwrap();
    j.record_operator_required(&k, &NonEmptyReason::parse("a reason").unwrap(), NOW).unwrap();
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
