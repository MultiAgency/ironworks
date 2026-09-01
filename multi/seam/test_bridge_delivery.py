"""Behavior-focused cases split from the bridge crash/recovery suite."""
try:
    from ._bridge_delivery_support import *
except ImportError:
    from _bridge_delivery_support import *


def _completed_delivery_fixture():
    d, db = _tmp()
    st = bs.BridgeState(db)
    st.note_received(1, GID, 1)
    st.note_turn_started(1, GID, "key", None)
    th = tb._load_threads(GROUPS, st)[GID]
    th.prev = "resp_existing"
    st.commit_turn(1, GID, th)
    ic = FakeIronclaw()
    ic.responses[th.prev] = "stored answer"
    return d, db, st, th, ic

def test_an_ordinary_turn_exception_is_contained():
    """Explicitly pinned because it was CLAIMED to be broken and was not: an exception in one
    update must not stop the loop, and the next update must still be processed."""
    d, db = _tmp()
    ic = FakeIronclaw()
    boom = [True]

    class T:
        @staticmethod
        def run(thread, text, speaker=None, idempotency_key=None, budget=None):
            if boom[0]:
                boom[0] = False
                raise RuntimeError("account store unreachable")
            return ic.run(thread, text, speaker=speaker, idempotency_key=idempotency_key)

        @staticmethod
        def fetch(client, rid):
            return ic.fetch(client, rid)

    st = bs.BridgeState(db)
    tg = FakeTelegram([[upd(1), upd(2)]])
    logs = []
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=T(), state=st, clock=_Clock(), log=logs.append, budget_seconds=5)
    b.poll_once()
    assert st.update_row(1)["state"] == bs.FAILED_TERMINAL
    assert st.update_row(2)["state"] in ANSWERED, "the loop stopped after one bad update"
    assert st.cursor == 3
    assert tg.sent[0][1] == bridge_core.CLIENT_FAILURE
    assert any("turn error" in ln for ln in logs)
    st.close(); d.cleanup()
    print("  PASS an ordinary turn exception is contained; the next update still runs")


def test_pre_send_failure_is_failed_but_post_send_unknown_is_recovery_blocked():
    class T:
        error = None

        @staticmethod
        def run(*a, **k):
            raise T.error

        @staticmethod
        def fetch(client, rid):
            raise AssertionError("neither failure path has a response id to fetch")

    for sent, want_state, want_text in (
            (False, bs.FAILED_TERMINAL, bridge_core.CLIENT_FAILURE),
            (True, bs.RECOVERY_BLOCKED, bridge_core.CLIENT_BLOCKED)):
        d, db = _tmp()
        T.error = ing.TurnBudgetExceeded("budget boundary", request_sent=sent)
        st = bs.BridgeState(db)
        tg = FakeTelegram([[upd(1)]])
        b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st),
                              telegram=tg, turns=T(), state=st, clock=_Clock(),
                              log=lambda *_: None, budget_seconds=5)
        b.poll_once()
        assert st.update_row(1)["state"] == want_state
        assert tg.sent == [(GID, want_text)], tg.sent
        st.close(); d.cleanup()
    print("  PASS pre-send failure is FAILED; post-send unknown is RECOVERY_BLOCKED")


def test_known_unsent_delivery_retries_the_stored_answer_without_a_model_turn():
    d, db, st, th, ic = _completed_delivery_fixture()
    tg = FakeTelegram([])
    attempts = [0]

    def send(*_):
        attempts[0] += 1
        if attempts[0] == 1:
            raise bridge_core.DeliveryAttemptError(
                "Telegram rejected it", acknowledged_chunks=0, known_not_sent=True)

    tg.send_message = send
    b = tb.TelegramBridge(groups=GROUPS, threads={GID: th}, telegram=tg,
                          turns=_turns(ic, None), state=st, clock=_Clock(),
                          log=lambda *_: None, budget_seconds=5)
    first = b.handle_update(upd(1))
    assert first == bridge_core.OUT_DELIVERY_RETRY
    assert st.update_row(1)["state"] == bs.DELIVERY_RETRY and st.cursor == 2
    assert b.handle_update(upd(1)) == bridge_core.OUT_DELIVERY_RETRY
    assert attempts[0] == 1, "a permanent rejection was retried without operator action"
    assert b.redeliver_reconciled(1) == bridge_core.OUT_REDELIVERED
    assert st.update_row(1)["state"] == bs.ACKED and st.cursor == 2
    assert ic.runs == [] and ic.fetches == ["resp_existing", "resp_existing"]
    st.close(); d.cleanup()
    print("  PASS known-unsent delivery retries the stored answer and never reruns the model")


def test_partial_or_ambiguous_delivery_requires_explicit_stored_answer_redelivery():
    for acknowledged, code in ((0, "delivery_uncertain"), (1, "delivery_partial")):
        d, db, st, th, ic = _completed_delivery_fixture()
        tg = FakeTelegram([])
        sends = [0]

        def uncertain(*_):
            sends[0] += 1
            raise bridge_core.DeliveryAttemptError(
                "outcome unknown", acknowledged_chunks=acknowledged, known_not_sent=False)

        tg.send_message = uncertain
        b = tb.TelegramBridge(groups=GROUPS, threads={GID: th}, telegram=tg,
                              turns=_turns(ic, None), state=st, clock=_Clock(),
                              log=lambda *_: None, budget_seconds=5)
        outcome = b.handle_update(upd(1))
        row = st.update_row(1)
        assert outcome == bridge_core.OUT_DELIVERY_RECONCILE
        assert row["state"] == bs.DELIVERY_RECONCILE and row["error_code"] == code
        assert row["response_id"] == "resp_existing" and st.cursor == 2
        assert b.handle_update(upd(1)) == bridge_core.OUT_DELIVERY_RECONCILE
        assert sends[0] == 1 and ic.runs == [], "uncertainty caused automatic delivery/model replay"

        tg.send_message = lambda *_: sends.__setitem__(0, sends[0] + 1)
        assert b.redeliver_reconciled(1) == bridge_core.OUT_REDELIVERED
        assert st.update_row(1)["state"] == bs.ACKED
        assert st.cursor == 2, "reconciling an old update moved the global cursor"
        assert ic.runs == [] and ic.fetches == ["resp_existing", "resp_existing"]
        st.close(); d.cleanup()
    print("  PASS ambiguous/partial delivery retains the answer for explicit no-model redelivery")


def test_a_FAILED_redelivery_preserves_the_delivery_evidence_the_next_one_needs():
    """`bridge redeliver` is designed to be retried, so a failed attempt is the NORMAL path.

    `delivery_partial` vs `delivery_uncertain` is the only record of whether Telegram had
    already acknowledged chunks — i.e. whether the operator's next attempt duplicates content
    in a client group. The failure branch wrote "redelivery_failed" over it via note_state's
    COALESCE, so the first failure destroyed the input to the second."""
    for acknowledged, code in ((0, "delivery_uncertain"), (1, "delivery_partial")):
        d, db, st, th, ic = _completed_delivery_fixture()
        tg = FakeTelegram([])

        def uncertain(*_):
            raise bridge_core.DeliveryAttemptError(
                "outcome unknown", acknowledged_chunks=acknowledged, known_not_sent=False)

        tg.send_message = uncertain
        b = tb.TelegramBridge(groups=GROUPS, threads={GID: th}, telegram=tg,
                              turns=_turns(ic, None), state=st, clock=_Clock(),
                              log=lambda *_: None, budget_seconds=5)
        assert b.handle_update(upd(1)) == bridge_core.OUT_DELIVERY_RECONCILE
        assert st.update_row(1)["error_code"] == code

        def boom(*_):
            raise RuntimeError("telegram unreachable")

        tg.send_message = boom
        assert b.redeliver_reconciled(1) == bridge_core.OUT_DELIVERY_RECONCILE
        row = st.update_row(1)
        assert row["state"] == bs.DELIVERY_RECONCILE, row["state"]
        assert row["error_code"] == code, (
            f"a failed redelivery overwrote the delivery evidence with {row['error_code']!r}; "
            "the next attempt can no longer tell whether chunks were already acknowledged")
        assert row["response_id"] == "resp_existing"

        # ...and the retry it exists for still works, with the evidence intact.
        tg.send_message = lambda *_: None
        assert b.redeliver_reconciled(1) == bridge_core.OUT_REDELIVERED
        assert st.update_row(1)["state"] == bs.ACKED
        st.close(); d.cleanup()
    print("  PASS a failed redelivery preserves delivery_partial/delivery_uncertain for the retry")


def test_delivery_reconciliation_handles_are_never_compacted():
    for delivery_state in (bs.DELIVERY_RETRY, bs.DELIVERY_RECONCILE):
        d, db = _tmp()
        st = bs.BridgeState(db)
        st.note_received(1, GID, 1)
        st.note_turn_started(1, GID, "key", None)
        th = tb._load_threads(GROUPS, st)[GID]
        th.prev = "resp_existing"
        st.commit_turn(1, GID, th)
        st.note_terminal(1, delivery_state, "delivery_evidence")
        for uid in range(2, 12):
            st.note_received(uid, GID, uid)
            st.note_terminal(uid, bs.IGNORED)
        st.meta_set("cursor_acked", 12)
        st.compact(retain=1)
        row = st.update_row(1)
        assert row and row["response_id"] == "resp_existing"
        st.close(); d.cleanup()
    print("  PASS compaction never discards a retained delivery response handle")


def test_deliver_existing_reports_the_actual_ambiguous_outcome():
    d, db, st, th, ic = _completed_delivery_fixture()
    tg = FakeTelegram([])
    tg.send_message = lambda *_: (_ for _ in ()).throw(OSError("telegram down"))
    b = tb.TelegramBridge(groups=GROUPS, threads={GID: th}, telegram=tg,
                          turns=_turns(ic, None), state=st, clock=_Clock(),
                          log=lambda *_: None, budget_seconds=5)
    outcome = b.handle_update(upd(1))
    assert outcome == bridge_core.OUT_DELIVERY_RECONCILE, outcome
    assert st.update_row(1)["state"] == bs.DELIVERY_RECONCILE
    st.close(); d.cleanup()
    print("  PASS _deliver_existing reports the actual ambiguous delivery outcome")


def test_chunk_transport_preserves_acknowledgement_evidence():
    original = tb.tg
    calls = []

    def partial(method, **params):
        calls.append(params["text"])
        if len(calls) == 1:
            return {"ok": True, "result": {"message_id": 1}}
        raise TimeoutError("ack lost")

    tb.tg = partial
    try:
        try:
            tb.send(GID, "a" * 3800 + "\n" + "b" * 20)
            raise AssertionError("partial chunk failure was reported as complete")
        except bridge_core.DeliveryAttemptError as e:
            assert e.acknowledged_chunks == 1 and not e.known_not_sent
    finally:
        tb.tg = original
    print("  PASS chunk transport reports exactly how many chunks Telegram acknowledged")


def test_a_failed_update_is_never_retried_into_a_second_turn():
    d, db = _tmp()
    ic = FakeIronclaw()

    class T:
        @staticmethod
        def run(*a, **k):
            raise RuntimeError("nope")

        @staticmethod
        def fetch(client, rid):
            return ic.fetch(client, rid)

    st = bs.BridgeState(db)
    tg = FakeTelegram([[upd(1)], [upd(1)]])
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=T(), state=st, clock=_Clock(), log=lambda *_: None,
                          budget_seconds=5)
    b.poll_once()
    b.poll_once()
    assert len(tg.sent) == 1, f"a terminal update was re-processed: {tg.sent}"
    st.close(); d.cleanup()
    print("  PASS a terminal update is never re-processed on redelivery")


def test_the_outcome_vocabulary_reaches_the_progress_snapshot():
    """`handle_update`'s return is a CONTRACT, not decoration — assert on it, not on log text.

    It used to be discarded by `poll_once`, which left six of the seven OUT_* constants with no
    reader at all and pushed the tests that wanted an outcome onto log-string matching. This
    pins the whole path: handle_update -> poll_once -> note_progress -> progress_snapshot, which
    is what `ironworks bridge status` prints as `last_outcome`."""
    d, db = _tmp()
    ic = FakeIronclaw()
    chatter = {"update_id": 1, "message": {"message_id": 1, "chat": {"id": int(GID)},
                                           "from": {"first_name": "Sam"}, "text": "just talking"}}
    st = bs.BridgeState(db)
    tg = FakeTelegram([[chatter], [upd(2)]])
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=_turns(ic, None), state=st, clock=_Clock(),
                          log=lambda *_: None, budget_seconds=5)
    b.poll_once()
    assert st.progress_snapshot()["last_outcome"] == bridge_core.OUT_IGNORED, \
        f"an ignored update did not reach the snapshot: {st.progress_snapshot()['last_outcome']!r}"
    b.poll_once()
    assert st.progress_snapshot()["last_outcome"] == bridge_core.OUT_ANSWERED, \
        f"an answered update did not reach the snapshot: {st.progress_snapshot()['last_outcome']!r}"
    st.close(); d.cleanup()
    print("  PASS the outcome vocabulary reaches the progress snapshot")


def test_an_unaddressed_message_is_terminal_and_costs_nothing():
    d, db = _tmp()
    ic = FakeIronclaw()
    chatter = {"update_id": 1, "message": {"message_id": 1, "chat": {"id": int(GID)},
                                           "from": {"first_name": "Sam"}, "text": "just talking"}}
    st = bs.BridgeState(db)
    tg = FakeTelegram([[chatter]])
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=_turns(ic, None), state=st, clock=_Clock(),
                          log=lambda *_: None, budget_seconds=5)
    assert b.handle_update(chatter) == bridge_core.OUT_IGNORED
    assert st.update_row(1)["state"] == bs.IGNORED
    assert st.cursor == 2 and ic.runs == [] and tg.sent == []
    st.close(); d.cleanup()
    print("  PASS an unaddressed message is terminal, advances the cursor, costs no turn")


def test_a_response_that_cannot_be_fetched_blocks_rather_than_regenerates():
    """Retention is unmeasured (test_responses_recovery.py Q5), so retrieval is fallible. When
    it fails the bridge must block, not quietly run a new turn."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-send")
    ic.fetch_fails = True
    tg, row, cursor, _ = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1, "a new turn was generated when retrieval failed"
    assert row["state"] == bs.RECOVERY_BLOCKED and row["error_code"] == "response_unavailable"
    assert tg.sent[0][1] == bridge_core.CLIENT_BLOCKED and cursor == 2
    d.cleanup()
    print("  PASS an unfetchable response blocks; it never regenerates an answer")


def test_the_journal_holds_no_client_content():
    """The whole journal, byte for byte, must not contain the message or the reply."""
    d, db = _tmp()
    ic = FakeIronclaw()
    secret_q = "zzq-confidential-question-marker"
    run_clean(db, ic, [[upd(1, text=secret_q)]])
    raw = db.read_bytes().decode("latin-1")
    assert secret_q not in raw, "the client's message text is in the durable journal"
    assert "ANSWER#1" not in raw, "the generated reply is in the durable journal"
    assert CL.persona not in raw, "model-visible instructions were stored instead of their hash"
    assert CL.ironclaw_token not in raw and CL.account_token not in raw, "a credential is stored"
    d.cleanup()
    print("  PASS the journal carries identifiers only — no message, reply, or credential")


def test_graceful_stop_leaves_the_rest_of_the_batch_unacknowledged():
    """A SIGTERM mid-batch must finish the update in hand and leave the others for the next
    process — which is safe precisely because the cursor is per-update."""
    d, db = _tmp()
    ic = FakeIronclaw()
    st = bs.BridgeState(db)
    tg = FakeTelegram([[upd(1), upd(2), upd(3)]])
    logs = []
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=_turns(ic, None), state=st, clock=_Clock(), log=logs.append,
                          budget_seconds=5)
    original = b.handle_update

    def stop_after_first(u):
        out = original(u)
        b.stopping = True
        return out
    b.handle_update = stop_after_first
    b.poll_once()
    assert st.update_row(1)["state"] == bs.DELIVERED
    assert st.update_row(2)["state"] == bs.RECEIVED
    assert st.update_row(3)["state"] == bs.RECEIVED
    assert st.cursor == 2, "the unhandled updates were acknowledged anyway"
    assert any("unacknowledged" in ln for ln in logs), logs
    st.close(); d.cleanup()
    print("  PASS a graceful stop finishes one update and leaves the rest for the next process")


def test_acknowledgement_turns_delivered_into_acked():
    d, db = _tmp()
    ic = FakeIronclaw()
    run_clean(db, ic, [[upd(1)], []], polls=2)
    st = bs.BridgeState(db)
    assert st.update_row(1)["state"] == bs.ACKED, st.update_row(1)["state"]
    assert st.cursor_acked == 2
    st.close(); d.cleanup()
    print("  PASS a later poll carrying the offset marks the update ACKED")

if __name__ == "__main__":
    # Discovered, not listed — and PRESENT AT ALL, which is the defect this replaces. The four
    # suites split out of the delivery monolith carried no runner, so `python3 test_bridge_delivery.py` printed
    # nothing and exited 0: the documented local gate in CONTRIBUTING.md scored silence as
    # success across 14 crash-boundary and delivery tests. `unittest` collects none of them
    # either (bare functions, not TestCase), so pytest was the only thing on any path that ran
    # them. globals() preserves definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL BRIDGE DELIVERY TESTS PASS")
