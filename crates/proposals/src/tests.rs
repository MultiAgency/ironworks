use super::*;
use ironworks_domain::ChannelRef;

/// The instant these tests record at, as the type the store takes.
///
/// `now: &str` accepted anything; `ironworks_domain::Utc` is the one parser for the form every
/// timestamp column in this workspace holds.
fn now() -> ironworks_domain::Utc {
    ironworks_domain::Utc::parse(AT).expect("the fixture instant is the canonical UTC form")
}

struct Store {
    _dir: std::path::PathBuf,
    store: ProposalStore,
}

impl std::ops::Deref for Store {
    type Target = ProposalStore;
    fn deref(&self) -> &ProposalStore {
        &self.store
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

/// A directory no other test in this process can be handed.
///
/// A counter rather than a clock. The first version keyed on `SystemTime` nanoseconds, and
/// `SystemTime` is coarser than its unit — two tests running in parallel landed in one bucket,
/// shared a store, and one of them found the other's proposal under the same update id. It failed
/// roughly one run in eight, which is worse than failing always: the claim checker reported
/// the suite BLOCKED at random. A counter cannot collide, so this cannot come back.
fn scratch(kind: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ironworks-proposal-{kind}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A store on a fresh directory. Disposable: nothing here touches an operator's file.
fn store() -> Store {
    let dir = scratch("live");
    let store = ProposalStore::open(dir.join("proposals.db")).expect("store opens");
    Store { _dir: dir, store }
}

#[test]
fn an_online_snapshot_is_readable_and_never_overwrites() {
    let source = store();
    let dir = scratch("snapshot");
    let copy = dir.join("proposals.db");
    source.snapshot_to(&copy).unwrap();
    assert!(ProposalStore::open(&copy).is_ok());
    assert!(matches!(source.snapshot_to(&copy), Err(ProposalError::Snapshot { .. })));
    let _ = std::fs::remove_dir_all(dir);
}

fn actor(id: &str) -> ActorRef {
    ActorRef::parse(id).expect("actor")
}
fn contributor(id: &str) -> ContributorId {
    ContributorId::parse(id).expect("contributor")
}
fn slug() -> Slug {
    Slug::parse("multiagency").expect("slug")
}
fn org() -> OrgId {
    OrgId::parse("multiagency-eval").expect("org")
}
fn account() -> AccountId {
    AccountId::parse("ACME-001").expect("account")
}

const NOTE: &str = "Customer requested a September renewal review.";
const AT: &str = "2026-09-01T09:15:00+00:00";

/// The scope every proposal in these tests is made in, owned by the test rather than leaked so
/// the borrow in `NewProposal` has somewhere real to point.
struct Scope {
    slug: Slug,
    org: OrgId,
    account: AccountId,
    channel: ChannelRef,
}

fn scope() -> Scope {
    Scope { slug: slug(), org: org(), account: account(), channel: ChannelRef::new("184223311") }
}

impl Scope {
    fn proposal<'a>(
        &'a self,
        update: u64,
        who: &'a ActorRef,
        as_: &'a ContributorId,
    ) -> NewProposal<'a> {
        NewProposal {
            update,
            channel: &self.channel,
            actor: who,
            contributor: as_,
            slug: &self.slug,
            org: &self.org,
            account: &self.account,
            body: NOTE,
        }
    }

    /// The authority a confirmation would be offered under, matching this scope.
    fn authority<'a>(&'a self, who: &'a ActorRef, as_: &'a ContributorId) -> ConfirmAuthority<'a> {
        ConfirmAuthority {
            channel: &self.channel,
            actor: who,
            slug: &self.slug,
            org: &self.org,
            contributor: as_,
        }
    }
}

/// Show a proposal the way the relay does: durable intent, send, durable outcome.
fn echo(s: &ProposalStore, update: u64, outcome: AttemptOutcome) -> Echo {
    s.begin_echo(update).expect("intent");
    s.record_echo(update, outcome, &now()).expect("outcome")
}

#[test]
fn a_proposal_keeps_the_bytes_that_were_typed() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    let stored = s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    assert_eq!(stored.body, NOTE, "the note is the point; nothing may reshape it");
    assert_eq!(stored.contributor.as_str(), "alice");
    assert_eq!(stored.account.as_str(), "ACME-001");
    assert_eq!(stored.proposed_at, AT);
    assert_eq!(stored.echo, Echo::Pending, "nothing has been shown yet");
}

#[test]
fn a_redelivered_update_finds_its_own_proposal_rather_than_making_a_second() {
    // The channel resends until an offset moves past an update, so this is the ordinary case and
    // not an edge one. A second proposal would mean a second thing to confirm for one request.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    let first = s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("first");
    let again = s
        .propose(
            sc.proposal(41, &alice, &as_alice),
            &ironworks_domain::Utc::parse("2026-09-01T09:20:00+00:00").unwrap(),
        )
        .expect("redelivery is not an error");
    assert_eq!(first, again);
    assert_eq!(
        again.proposed_at, AT,
        "the original moment is kept: a redelivery is not a new proposal"
    );
}

#[test]
fn the_same_update_carrying_different_content_is_a_conflict_and_changes_nothing() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let bob = actor("713300284");
    let as_alice = contributor("alice");
    let as_bob = contributor("bob");
    let original = s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("first");

    let other_account = AccountId::parse("GLOBEX-002").unwrap();
    let other_channel = ChannelRef::new("713300284");
    let cases: Vec<(&str, NewProposal<'_>)> = vec![
        ("actor", NewProposal { actor: &bob, ..sc.proposal(41, &alice, &as_alice) }),
        (
            "conversation",
            NewProposal { channel: &other_channel, ..sc.proposal(41, &alice, &as_alice) },
        ),
        ("contributor", NewProposal { contributor: &as_bob, ..sc.proposal(41, &alice, &as_alice) }),
        ("account", NewProposal { account: &other_account, ..sc.proposal(41, &alice, &as_alice) }),
        (
            "note",
            NewProposal { body: "something else entirely", ..sc.proposal(41, &alice, &as_alice) },
        ),
    ];
    for (field, differing) in cases {
        let err = s.propose(differing, &now()).expect_err(field);
        assert!(matches!(err, ProposalError::Conflict { update: 41, .. }), "{field}: {err}");
        assert_eq!(
            s.proposal(41).unwrap().unwrap(),
            original,
            "{field}: a conflict must change nothing"
        );
    }
}

#[test]
fn only_a_delivered_proposal_is_confirmable() {
    // Rule 1, and the reason it is a type rather than a check: a person who never saw the
    // proposal cannot have approved it, and a later patch must not be able to act as if they had.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    assert!(
        s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_none(),
        "pending is not confirmable"
    );

    echo(&s, 41, AttemptOutcome::Ambiguous { acknowledged_chunks: 0 });
    assert!(
        s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_none(),
        "uncertain is not confirmable"
    );

    // A fresh proposal, because the uncertain one stays uncertain for an operator.
    s.propose(sc.proposal(42, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 42, AttemptOutcome::Complete);
    let ready =
        s.confirmable(sc.authority(&alice, &as_alice)).unwrap().expect("delivered is confirmable");
    assert_eq!(ready.proposal().update, 42);
    assert_eq!(ready.proposal().body, NOTE);
}

#[test]
fn one_actor_can_never_reach_another_actors_proposal() {
    // Rule 2. Bob confirming Alice's words would attribute her statement to him and promote it
    // without her.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let bob = actor("713300284");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);

    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_some());
    let as_bob = contributor("bob");
    assert!(
        s.confirmable(sc.authority(&bob, &as_bob)).unwrap().is_none(),
        "a delivered proposal is still not Bob's"
    );
}

#[test]
fn a_newer_proposal_blocks_an_older_delivered_one() {
    // The echo tells people another /share replaces the proposal, so the newest is the only one
    // they could mean. Searching for the newest DELIVERED row instead would keep yesterday's
    // words alive behind today's, and confirm something the person believes they superseded.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");

    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("first");
    echo(&s, 41, AttemptOutcome::Complete);
    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_some());

    // A newer one that has not been shown. The older delivered proposal must go out of reach.
    s.propose(sc.proposal(42, &alice, &as_alice), &now()).expect("second");
    assert!(
        s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_none(),
        "a pending proposal must hide the delivered one it replaced"
    );

    // And an uncertain newer one hides it too: it may or may not be in the room, so which
    // proposal the person means is exactly what nobody can say.
    echo(&s, 42, AttemptOutcome::Ambiguous { acknowledged_chunks: 0 });
    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_none());

    // Only when the newest itself is delivered is anything confirmable, and it is the newest.
    s.propose(sc.proposal(43, &alice, &as_alice), &now()).expect("third");
    echo(&s, 43, AttemptOutcome::Complete);
    let ready = s.confirmable(sc.authority(&alice, &as_alice)).unwrap().expect("newest");
    assert_eq!(ready.proposal().update, 43);
}

#[test]
fn authority_that_has_moved_since_makes_a_proposal_unreachable() {
    // A registry edit that re-homed a conversation, or a credential that now resolves to another
    // organization, must not make yesterday's words promotable into somewhere they were never
    // meant for. Every axis is re-verified rather than remembered.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);
    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_some());

    let other_channel = ChannelRef::new("713300284");
    let other_slug = Slug::parse("someone-else").unwrap();
    let other_org = OrgId::parse("someone-else-org").unwrap();
    let other_contributor = contributor("bob");
    let base = sc.authority(&alice, &as_alice);
    for (axis, moved) in [
        ("conversation", ConfirmAuthority { channel: &other_channel, ..base }),
        ("tenant", ConfirmAuthority { slug: &other_slug, ..base }),
        ("organization", ConfirmAuthority { org: &other_org, ..base }),
        ("contributor", ConfirmAuthority { contributor: &other_contributor, ..base }),
    ] {
        assert!(
            s.confirmable(moved).unwrap().is_none(),
            "a moved {axis} must put the proposal out of reach"
        );
    }
}

#[test]
fn a_send_the_channel_proved_never_left_stays_pending_so_a_retry_is_safe() {
    // The one outcome that licenses a free retry, and the store has to agree with the transport
    // about it: nothing was delivered, the person has seen nothing.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    assert_eq!(echo(&s, 41, AttemptOutcome::Rejected), Echo::Pending);
    assert_eq!(s.proposal(41).unwrap().unwrap().echo, Echo::Pending);
    // And the retry is allowed, unlike a second attempt after any other outcome.
    assert!(s.proposal(41).unwrap().unwrap().echo.may_attempt());
    assert_eq!(echo(&s, 41, AttemptOutcome::Complete), Echo::Delivered);
}

#[test]
fn a_delivered_proposal_refuses_a_second_echo_rather_than_sending_another_copy() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);
    let err = s.begin_echo(41).unwrap_err();
    assert!(matches!(err, ProposalError::AlreadyEchoed { update: 41 }), "{err}");
    // And a bare outcome with no intent behind it is refused too: a send without a durable
    // intent is exactly the window `begin_echo` exists to close.
    let err = s.record_echo(41, AttemptOutcome::Complete, &now()).unwrap_err();
    assert!(matches!(err, ProposalError::AlreadyEchoed { update: 41 }), "{err}");
}

#[test]
fn an_uncertain_echo_is_preserved_for_an_operator_and_never_resent_here() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    let outcome = echo(&s, 41, AttemptOutcome::Ambiguous { acknowledged_chunks: 1 });
    assert_eq!(outcome, Echo::Uncertain { acknowledged_chunks: 1 });
    assert!(outcome.needs_operator());
    assert!(!outcome.may_attempt(), "an uncertain proposal is never re-sent automatically");
    let waiting = s.awaiting_operator().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].update, 41);
    assert_eq!(waiting[0].body, NOTE, "the words are kept; that is what makes it settleable");
}

#[test]
fn a_proposal_survives_reopening_the_store() {
    // It exists to outlive the process between two messages, so this is the property rather than
    // a detail of it.
    let dir = scratch("reopen");
    let path = dir.join("proposals.db");
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = ProposalStore::open(&path).unwrap();
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        echo(&s, 41, AttemptOutcome::Complete);
    }
    {
        let s = ProposalStore::open(&path).unwrap();
        let ready = s
            .confirmable(sc.authority(&alice, &as_alice))
            .unwrap()
            .expect("still confirmable after a restart");
        assert_eq!(ready.proposal().body, NOTE);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_store_is_owner_only() {
    // The same posture as the journals, and here the file holds what people wrote before they
    // decided to share it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let s = store();
        let path = s._dir.join("proposals.db");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the proposal store must not be readable by anyone else");
    }
}

#[test]
fn an_echo_recorded_against_no_proposal_is_refused() {
    let s = store();
    let err = s.record_echo(999, AttemptOutcome::Complete, &now()).unwrap_err();
    assert!(matches!(err, ProposalError::NoSuchProposal { update: 999 }), "{err}");
    let err = s.begin_echo(999).unwrap_err();
    assert!(matches!(err, ProposalError::NoSuchProposal { update: 999 }), "{err}");
}

// ── crash boundaries around the echo ────────────────────────────────────────────────────────
//
// The window each one models is the gap between a byte reaching Telegram and a fact reaching
// disk. The rule they all serve: conservative false ambiguity is acceptable, and an automatic
// duplicate is not. Every state below is asserted through a REOPENED store, because a state that
// only holds in memory is not the one a redelivery would find.

/// Reopen the store, the way the next relay cycle does after a crash.
fn reopened(path: &std::path::Path) -> ProposalStore {
    ProposalStore::open(path).expect("the store reopens")
}

/// A store on a named path, so a test can drop it and open it again.
fn crash_store() -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = scratch("crash");
    let path = dir.join("proposals.db");
    (dir, path)
}

#[test]
fn a_crash_after_the_send_intent_but_before_the_send_never_sends_again() {
    // The intent landed and nothing left. The message is NOT in the room — and nothing here can
    // establish that, which is the whole point: `attempting` is deliberately ambiguous, so the
    // cost of closing the duplicate window is a proposal a person has to look at.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        s.begin_echo(41).unwrap();
        // ── crash ──
    }
    let s = reopened(&path);
    let after = s.proposal(41).unwrap().unwrap();
    assert_eq!(after.echo, Echo::Attempting);
    assert!(!after.echo.may_attempt(), "a redelivery must not send a second copy");
    assert!(after.echo.needs_operator());
    assert_eq!(s.awaiting_operator().unwrap().len(), 1);
    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crash_after_the_send_but_before_the_outcome_never_sends_again() {
    // The dangerous one, and the reason the intent is durable at all. Telegram accepted the echo
    // and the process died before recording it. Without `attempting` the redelivery would find a
    // pending proposal and put a second copy of the same words in the room.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        s.begin_echo(41).unwrap();
        // the send happens here, and Telegram accepts it
        // ── crash before record_echo ──
    }
    let s = reopened(&path);
    assert_eq!(s.proposal(41).unwrap().unwrap().echo, Echo::Attempting);
    assert!(!s.proposal(41).unwrap().unwrap().echo.may_attempt());
    // A redelivery cannot even begin: the store refuses a second attempt rather than relying on
    // the caller to have checked.
    assert!(matches!(s.begin_echo(41), Err(ProposalError::AlreadyEchoed { update: 41 })));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crash_after_a_delivered_outcome_but_before_the_disposition_never_sends_again() {
    // The outcome is durable and the channel journal is not, so the update is still unclaimed and
    // will be redelivered. The person HAS seen the proposal, so the correct behaviour is to send
    // nothing, settle, and not raise an alarm.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        echo(&s, 41, AttemptOutcome::Complete);
        // ── crash before the channel journal settles the update ──
    }
    let s = reopened(&path);
    let after = s.proposal(41).unwrap().unwrap();
    assert_eq!(after.echo, Echo::Delivered);
    assert!(!after.echo.may_attempt());
    assert!(!after.echo.needs_operator(), "a delivered proposal is nobody's to settle");
    // And it is confirmable, which is what the person is about to do.
    assert!(s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_positively_rejected_send_may_safely_be_retried_after_a_crash() {
    // The one outcome that licenses a free retry, across a restart. The channel PROVED nothing
    // left, so the person has seen nothing and the proposal returns to pending — which is what
    // lets the redelivery finish the job rather than stranding it.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        assert_eq!(echo(&s, 41, AttemptOutcome::Rejected), Echo::Pending);
        // ── crash ──
    }
    let s = reopened(&path);
    let after = s.proposal(41).unwrap().unwrap();
    assert_eq!(after.echo, Echo::Pending);
    assert!(after.echo.may_attempt(), "nothing was delivered, so a retry cannot duplicate");
    assert!(!after.echo.needs_operator(), "nothing is unresolved");
    assert!(s.awaiting_operator().unwrap().is_empty());
    // The retry finds the same proposal and completes it. One proposal, one echo.
    assert_eq!(echo(&s, 41, AttemptOutcome::Complete), Echo::Delivered);
    assert_eq!(s.proposal(41).unwrap().unwrap().proposed_at, AT, "still the original moment");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── the confirmation lane ───────────────────────────────────────────────────────────────────
//
// The append's version of the echo's rule, one layer up and with a sharper cost: the effect is
// not a message in a room, it is immutable shared evidence. Every state below exists to make
// "we tried and cannot say" different from "we have not tried".

/// Confirm a proposal the way the relay does: durable intent, request, durable outcome.
fn append_ok(s: &ProposalStore, update: u64, activity: &str, org: &OrgId) {
    s.begin_append(update, activity, "2026-09-01", update + 1000).expect("intent");
    s.record_commit(update, activity, org).expect("commit");
}

#[test]
fn a_confirmation_begins_only_from_unconfirmed_and_records_what_it_will_send() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    assert_eq!(s.proposal(41).unwrap().unwrap().confirm, Confirmation::Unconfirmed);

    s.begin_append(41, "share-abc", "2026-09-01", 42).expect("intent");
    let after = s.proposal(41).unwrap().unwrap();
    // The whole immutable request is recoverable from the row: the activity id and the date
    // here, the account, body, contributor and org already on it. That is what makes a retry
    // provably the same bytes rather than a second write.
    assert_eq!(
        after.confirm,
        Confirmation::Appending {
            activity_id: "share-abc".into(),
            occurred_at: "2026-09-01".into(),
            confirm_update: 42,
        }
    );
    assert!(!after.confirm.may_begin(), "a second /confirm must not begin a second append");
    assert!(after.confirm.needs_operator());

    let err = s.begin_append(41, "share-abc", "2026-09-01", 43).unwrap_err();
    assert!(matches!(err, ProposalError::AlreadyConfirmed { update: 41 }), "{err}");
}

#[test]
fn a_committed_confirmation_records_the_organization_the_store_answered_with() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    append_ok(&s, 41, "share-abc", &org());

    let after = s.proposal(41).unwrap().unwrap();
    assert_eq!(
        after.confirm,
        Confirmation::Committed {
            activity_id: "share-abc".into(),
            org: org(),
            confirm_update: 1041,
        }
    );
    assert!(!after.confirm.needs_operator(), "a committed append is nobody's to settle");
    // And the proposal itself is retained: it is the evidence of what was confirmed.
    assert_eq!(after.body, NOTE);
    assert_eq!(after.contributor.as_str(), "alice");
}

#[test]
fn a_contested_append_is_permanent_and_names_what_differed() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    s.begin_append(41, "share-abc", "2026-09-01", 42).unwrap();
    s.record_contest(41, "share-abc", "body").unwrap();

    let after = s.proposal(41).unwrap().unwrap();
    assert_eq!(
        after.confirm,
        Confirmation::Contested {
            activity_id: "share-abc".into(),
            differing_field: "body".into(),
            confirm_update: 42,
        }
    );
    assert!(after.confirm.needs_operator());
    assert!(!after.confirm.may_begin(), "a conflict is never retried automatically");
    assert_eq!(s.awaiting_operator().unwrap().len(), 1);
}

#[test]
fn an_outcome_naming_a_different_activity_than_the_intent_is_refused() {
    // The two halves have to agree or the row would record a fiction: an append that committed
    // one id while the intent named another is a bug upstream, not a state to persist.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    s.begin_append(41, "share-abc", "2026-09-01", 42).unwrap();
    let err = s.record_commit(41, "share-somethingelse", &org()).unwrap_err();
    assert!(matches!(err, ProposalError::Unusable { .. }), "{err}");
    assert!(matches!(s.proposal(41).unwrap().unwrap().confirm, Confirmation::Appending { .. }));
}

#[test]
fn a_crash_between_the_append_and_its_outcome_leaves_a_replayable_intent() {
    // The window the durable intent exists for. The POST may have landed; nothing here can tell.
    // What survives is the exact request, so the retry is the identical bytes the endpoint is
    // idempotent on — never a second, differently-shaped write.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        echo(&s, 41, AttemptOutcome::Complete);
        s.begin_append(41, "share-abc", "2026-09-01", 42).unwrap();
        // ── crash after the POST, before its outcome ──
    }
    let s = reopened(&path);
    let after = s.proposal(41).unwrap().unwrap();
    let Confirmation::Appending { activity_id, occurred_at, .. } = &after.confirm else {
        panic!("a crashed append must stay appending, got {:?}", after.confirm);
    };
    assert_eq!(activity_id, "share-abc");
    assert_eq!(occurred_at, "2026-09-01");
    // Everything else the request needs is on the row already, unchanged.
    assert_eq!(after.body, NOTE);
    assert_eq!(after.account.as_str(), "ACME-001");
    assert_eq!(after.org, org());
    assert!(after.confirm.needs_operator());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_success_reply_has_the_same_duplicate_protection_as_the_proposal_echo() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    append_ok(&s, 41, "share-abc", &org());

    assert_eq!(s.proposal(41).unwrap().unwrap().reply, Echo::Pending);
    s.begin_reply(41).expect("intent");
    assert_eq!(s.proposal(41).unwrap().unwrap().reply, Echo::Attempting);
    // A crash here never re-sends: attempting does not attempt.
    assert!(!s.proposal(41).unwrap().unwrap().reply.may_attempt());
    assert!(matches!(s.begin_reply(41), Err(ProposalError::AlreadyEchoed { .. })));

    assert_eq!(s.record_reply(41, AttemptOutcome::Complete).unwrap(), Echo::Delivered);
    assert!(!s.proposal(41).unwrap().unwrap().reply.may_attempt());
}

#[test]
fn a_reply_the_channel_proved_never_left_may_be_sent_again() {
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    append_ok(&s, 41, "share-abc", &org());
    s.begin_reply(41).unwrap();
    assert_eq!(s.record_reply(41, AttemptOutcome::Rejected).unwrap(), Echo::Pending);
    assert!(s.proposal(41).unwrap().unwrap().reply.may_attempt());
}

#[test]
fn a_version_1_store_is_refused_and_kept_rather_than_migrated() {
    // A version-1 file holds proposals and no confirmation lane, so it cannot say whether its
    // words were already appended. Reading it under these semantics risks appending twice;
    // rewriting it decides on its owner's behalf that unconfirmed words are now unconfirmable.
    // Refusing is the only option that loses nothing.
    let dir = scratch("v1");
    let path = dir.join("proposals.db");
    {
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch(
            "CREATE TABLE proposal_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO proposal_meta(key,value) VALUES('schema_version','1');",
        )
        .expect("seed a version-1 store");
    }
    let err = ProposalStore::open(&path).expect_err("an older schema must be refused");
    assert!(matches!(err, ProposalError::Version { found: 1, expected: 3 }), "{err}");
    assert!(err.to_string().contains("Refusing to run"), "{err}");
    // Kept, not cleared: the file is still there for an operator.
    assert!(path.exists(), "a refused store must not be deleted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_version_2_store_migrates_and_keeps_every_proposal_it_held() {
    // The other half of the version policy, and the reason the two differ is whether an existing
    // row can be misread. A v1 row cannot say whether its words were appended. A v2 row says
    // everything a v3 row says — `refused` is a state only this build writes, so no v2 row carries
    // it and nothing changes meaning. Refusing here would strand someone's unconfirmed words
    // behind an operator ticket to buy nothing.
    let dir = scratch("v2-migrate");
    let path = dir.join("proposals.db");
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = ProposalStore::open(&path).expect("a fresh store");
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("propose");
        echo(&s, 41, AttemptOutcome::Complete);
    }
    {
        // Wind it back to what a version-2 file is: the version row, and no column.
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch(
            "ALTER TABLE proposals DROP COLUMN refused_code;
             UPDATE proposal_meta SET value='2' WHERE key='schema_version';",
        )
        .expect("wind back to version 2");
    }

    let s = ProposalStore::open(&path).expect("a version-2 store migrates rather than refusing");
    let kept = s.proposal(41).expect("read").expect("the proposal survived");
    assert_eq!(kept.body, NOTE, "a migration must not touch a row");
    assert_eq!(kept.echo, Echo::Delivered, "nor any state it had reached");
    assert_eq!(kept.confirm, Confirmation::Unconfirmed);
    assert!(
        s.confirmable(sc.authority(&alice, &as_alice)).unwrap().is_some(),
        "and the words are still confirmable, which is the whole reason not to refuse"
    );

    // The column is really there: the migration ran rather than merely being reported.
    s.begin_append(41, "share-abc", "2026-09-01", 42).expect("begin");
    s.record_refusal(41, "share-abc", "unauthorized").expect("refuse");
    let after = s.proposal(41).expect("read").expect("still there");
    assert!(matches!(&after.confirm, Confirmation::Refused { code, .. } if code == "unauthorized"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The state a crash between the migration and its version write used to leave: the column added,
/// version 2 still recorded, and every later open refused with `duplicate column name` — someone's
/// unconfirmed words stranded behind exactly the operator ticket the migration exists to avoid.
/// The trigger stands in for the crash: it fails the version write and nothing else.
#[test]
fn a_version_2_migration_interrupted_before_its_version_write_reopens_and_migrates() {
    let dir = scratch("v2-interrupted");
    let path = dir.join("proposals.db");
    drop(ProposalStore::open(&path).expect("a fresh store"));
    {
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch(
            "ALTER TABLE proposals DROP COLUMN refused_code;
             UPDATE proposal_meta SET value='2' WHERE key='schema_version';
             CREATE TRIGGER crash_before_the_version BEFORE UPDATE ON proposal_meta
             BEGIN SELECT RAISE(ABORT, 'crashed before the version write'); END;",
        )
        .expect("wind back to version 2, with the version write rigged to fail");
    }
    let has_column = |conn: &rusqlite::Connection| {
        conn.prepare("SELECT 1 FROM pragma_table_info('proposals') WHERE name='refused_code'")
            .unwrap()
            .exists([])
            .unwrap()
    };
    let version = |conn: &rusqlite::Connection| -> String {
        conn.query_row("SELECT value FROM proposal_meta WHERE key='schema_version'", [], |r| {
            r.get(0)
        })
        .unwrap()
    };

    assert!(ProposalStore::open(&path).is_err(), "the version write was made to fail");
    {
        let conn = rusqlite::Connection::open(&path).expect("open");
        assert!(!has_column(&conn), "the ALTER must go down with the version write");
        assert_eq!(version(&conn), "2");
        conn.execute_batch("DROP TRIGGER crash_before_the_version").expect("disarm");
    }

    drop(ProposalStore::open(&path).expect("the interrupted migration re-runs cleanly"));
    let conn = rusqlite::Connection::open(&path).expect("open");
    // The positive control for the absence above: the same probe sees the column once it is there.
    assert!(has_column(&conn));
    assert_eq!(version(&conn), SCHEMA_VERSION.to_string());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_recovering_append_reads_its_request_from_the_row_not_from_a_derivation() {
    // The persisted intent is the SOURCE of a replay, not a receipt for one. Both fields are
    // read back — the activity id and the date — because a replay is only a replay at an
    // idempotent endpoint if it carries the same bytes, and a derivation that moved between
    // versions would otherwise turn recovery into a conflict.
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    {
        let s = reopened(&path);
        s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
        echo(&s, 41, AttemptOutcome::Complete);
        s.begin_append(41, "share-persisted", "2026-09-01", 42).unwrap();
        // ── crash ──
    }
    let s = reopened(&path);
    let Confirmation::Appending { activity_id, occurred_at, confirm_update } =
        s.proposal(41).unwrap().unwrap().confirm
    else {
        panic!("a crashed append stays appending");
    };
    // Every part of the request survives: these two here, and the account, body, contributor and
    // organization already on the row.
    assert_eq!(activity_id, "share-persisted");
    assert_eq!(occurred_at, "2026-09-01");
    assert_eq!(confirm_update, 42, "which /confirm began it, so its reply can be completed");
    let row = s.proposal(41).unwrap().unwrap();
    assert_eq!(row.body, NOTE);
    assert_eq!(row.account.as_str(), "ACME-001");
    assert_eq!(row.contributor.as_str(), "alice");
    assert_eq!(row.org, org());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_echo_lane_and_the_reply_lane_do_not_share_state() {
    // A delivered echo must not make the success reply look already sent, or a person whose
    // proposal was shown would never learn it was shared.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).unwrap();
    echo(&s, 41, AttemptOutcome::Complete);
    append_ok(&s, 41, "share-abc", &org());
    let row = s.proposal(41).unwrap().unwrap();
    assert_eq!(row.echo, Echo::Delivered);
    assert_eq!(row.reply, Echo::Pending, "the success reply has not been sent");
    assert!(row.reply.may_attempt());
}

#[test]
fn a_refused_append_is_terminal_and_is_never_replayed() {
    // The defect this state exists for. A permanent refusal used to be left `appending`, which is
    // the state that says "replay the identical request" — so every later /confirm re-sent a
    // request the store had already decided about. `appending` now means only what it says, and a
    // second attempt on a refused proposal is refused by the store rather than by a comment.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);

    s.begin_append(41, "share-abc", "2026-09-01", 42).expect("intent");
    s.record_refusal(41, "share-abc", "unknown_account").expect("the store decided");

    let after = s.proposal(41).unwrap().unwrap();
    let Confirmation::Refused { activity_id, code, confirm_update } = &after.confirm else {
        panic!("expected a refusal, got {:?}", after.confirm);
    };
    assert_eq!(activity_id, "share-abc");
    assert_eq!(code, "unknown_account", "the store's own word survives for the operator queue");
    assert_eq!(*confirm_update, 42, "and which /confirm asked");

    // Terminal on both axes: nothing may begin a second append, and nothing settles it again.
    assert!(!after.confirm.may_begin(), "a decided request is not begun again");
    let err = s.begin_append(41, "share-abc", "2026-09-01", 43).expect_err("no second attempt");
    assert!(matches!(err, ProposalError::AlreadyConfirmed { update: 41 }), "{err}");
    let err = s.record_commit(41, "share-abc", &org()).expect_err("no re-settling");
    assert!(matches!(err, ProposalError::AlreadyConfirmed { update: 41 }), "{err}");
}

#[test]
fn a_refused_append_reaches_the_operator_queue() {
    // It is exactly as unresolved as a conflict: a person asked for something, nothing was
    // written, and only an operator can change that. Leaving it out would hide the case the
    // state was added to surface.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);
    s.begin_append(41, "share-abc", "2026-09-01", 42).expect("intent");
    s.record_refusal(41, "share-abc", "unauthorized").expect("refused");

    let waiting = s.awaiting_operator().expect("queue");
    assert_eq!(waiting.len(), 1, "a refused append is somebody's to look at");
    assert_eq!(waiting[0].update, 41);
    assert!(waiting[0].confirm.needs_operator());
}

#[test]
fn a_committed_proposal_is_not_dragged_into_the_operator_queue_by_the_new_state() {
    // The positive control for the two above. A queue that listed every settled proposal would
    // satisfy both of them and be useless — the point is which rows are NOT in it.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    echo(&s, 41, AttemptOutcome::Complete);
    s.begin_append(41, "share-abc", "2026-09-01", 42).expect("intent");
    s.record_commit(41, "share-abc", &org()).expect("committed");
    s.begin_reply(41).expect("reply intent");
    s.record_reply(41, AttemptOutcome::Complete).expect("reply sent");
    assert!(s.awaiting_operator().expect("queue").is_empty(), "a finished promotion needs nobody");
}

#[test]
fn every_confirmation_state_has_exactly_one_stored_spelling() {
    // `tag` is the one mapping from a value to what SQLite holds, and the reader matches the same
    // constants. A variant added later fails to compile in `tag`; this is what says the constants
    // and the reader did not drift apart in the meantime.
    let org = org();
    let states = [
        (Confirmation::Unconfirmed, "unconfirmed"),
        (
            Confirmation::Appending {
                activity_id: "share-abc".into(),
                occurred_at: "2026-09-01".into(),
                confirm_update: 42,
            },
            "appending",
        ),
        (
            Confirmation::Committed {
                activity_id: "share-abc".into(),
                org: org.clone(),
                confirm_update: 42,
            },
            "committed",
        ),
        (
            Confirmation::Contested {
                activity_id: "share-abc".into(),
                differing_field: "body".into(),
                confirm_update: 42,
            },
            "contested",
        ),
        (
            Confirmation::Refused {
                activity_id: "share-abc".into(),
                code: "unauthorized".into(),
                confirm_update: 42,
            },
            "refused",
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (state, spelling) in &states {
        assert_eq!(state.tag(), *spelling);
        assert!(seen.insert(state.tag()), "{spelling} is spelled the same as another state");
    }
    assert_eq!(seen.len(), 5, "a state with no spelling would reach SQLite unnamed");
}

#[test]
fn a_row_this_build_cannot_read_is_refused_rather_than_coerced() {
    // The downgrade direction, which is why the schema version does not need to police it. An
    // older binary meeting `refused` cannot read the tag — and must not read it as `unconfirmed`,
    // which would offer to append words the store has already decided about.
    let s = store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");
    s.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    {
        let conn = rusqlite::Connection::open(s._dir.join("proposals.db")).expect("open");
        conn.execute("UPDATE proposals SET confirm='from-the-future' WHERE update_id=41", [])
            .expect("write a tag this build has never met");
    }
    let err = s.proposal(41).expect_err("an unreadable state is not a default");
    assert!(
        matches!(err, ProposalError::Unusable { update: 41, what: "confirmation state" }),
        "{err}"
    );
}

/// Two relay processes at the same redelivery must not both acquire the intent.
///
/// `begin_echo` reads the row, judges it in Rust, then writes an `UPDATE` with no state predicate.
/// The `Mutex<Connection>` serialises that pair within one process and says nothing about two —
/// and `relay` is one cycle per process, with other subcommands opening the same file. Both
/// handles below read `Pending`, both pass `may_attempt`, and both write `attempting`.
///
/// The consequence is the one the module doc calls "the one outcome this whole path exists to
/// prevent": each process believes it holds the intent and sends, and the person is shown the same
/// proposal twice.
#[test]
fn two_handles_cannot_both_acquire_the_echo_intent() {
    let (dir, path) = crash_store();
    let sc = scope();
    let alice = actor("184223311");
    let as_alice = contributor("alice");

    let one = reopened(&path);
    one.propose(sc.proposal(41, &alice, &as_alice), &now()).expect("stored");
    let two = reopened(&path);

    // Both observe the same pre-state. This is the window, made explicit rather than raced for.
    assert_eq!(one.proposal(41).unwrap().unwrap().echo, Echo::Pending);
    assert_eq!(two.proposal(41).unwrap().unwrap().echo, Echo::Pending);

    one.begin_echo(41).expect("the first acquires the intent");
    let second = two.begin_echo(41);
    assert!(
        matches!(second, Err(ProposalError::AlreadyEchoed { update: 41 })),
        "both processes acquired the intent and both would send: {second:?}"
    );

    // Positive control: the refusal is the store's, not the handle's. `two` is still usable, and
    // still sees the state the first handle committed.
    assert_eq!(two.proposal(41).unwrap().unwrap().echo, Echo::Attempting);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A damaged `schema_version` must be refused, not read as "no version recorded". See the same
/// test in `crates/journal/src/tests.rs` — one rule, three stores, and it was wrong in two.
#[test]
fn a_damaged_schema_version_is_refused_rather_than_read_as_absent() {
    let dir = scratch("schema-version");
    let path = dir.join("proposals.db");
    drop(ProposalStore::open(&path).unwrap());

    // The positive control: an untouched store re-opens.
    ProposalStore::open(&path).expect("an untouched proposal store re-opens");

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE proposal_meta SET value='three' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    match ProposalStore::open(&path) {
        Err(ProposalError::UnreadableVersion { raw }) => assert_eq!(raw, "three"),
        other => panic!("a damaged schema version must be refused, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
