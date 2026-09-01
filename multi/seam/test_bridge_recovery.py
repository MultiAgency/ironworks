"""Behavior-focused cases split from the bridge crash/recovery suite."""
try:
    from ._bridge_delivery_support import *
except ImportError:
    from _bridge_delivery_support import *

def test_a_clean_turn_is_answered_once_and_recorded():
    d, db = _tmp()
    ic = FakeIronclaw()
    tg, row, cursor, thread = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1, ic.runs
    assert len(tg.sent) == 1, tg.sent
    assert row["state"] == bs.DELIVERED and row["response_id"] == "resp_001"
    assert cursor == 2, cursor
    assert thread["prev"] == "resp_001"
    assert row["idempotency_key"], "no recovery handle was recorded"
    d.cleanup()
    print("  PASS clean turn: 1 model run, 1 send, cursor advanced, pointer committed")


def test_the_cursor_is_persisted_per_update_not_per_batch():
    """The defect in one line. Three updates, crash on the third: the first two must already be
    acknowledged, because a batch is not a unit of durability."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1), upd(2), upd(3)]], at="before-model", nth=3)
    st = bs.BridgeState(db)
    assert st.cursor == 3, f"cursor {st.cursor}: the first two updates were not acknowledged"
    assert st.update_row(1)["state"] in ANSWERED
    assert st.update_row(2)["state"] in ANSWERED
    st.close(); d.cleanup()
    print("  PASS the cursor advances per update, so a mid-batch crash keeps prior work")


def test_b1_crash_after_getupdates_before_journaling():
    """Nothing durable happened, so the update is simply redelivered and handled once."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-note_received")
    assert ic.runs == [], "a turn ran before the update was journaled"
    tg, row, cursor, _ = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1 and len(tg.sent) == 1
    assert row["state"] == bs.DELIVERED and cursor == 2
    d.cleanup()
    print("  PASS B1 crash before journaling -> handled exactly once on redelivery")


def test_b2_crash_after_turn_started_before_the_model_request():
    """The key is durable; whether a turn ran is unknown. The bridge must NOT run another."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-model")
    st = bs.BridgeState(db)
    assert st.update_row(1)["state"] == bs.TURN_STARTED
    key = st.update_row(1)["idempotency_key"]
    assert key, "the recovery handle was not recorded before the request"
    st.close()

    logs = []
    tg, row, cursor, thread = run_clean(db, ic, [[upd(1)]], logs=logs)
    assert ic.runs == [], f"a SECOND model turn was run after the crash: {ic.runs}"
    assert row["state"] == bs.RECOVERY_BLOCKED, row["state"]
    assert row["error_code"] == "turn_outcome_unknown"
    assert cursor == 2, "a blocked update must still advance the cursor, or it replays forever"
    assert thread.get("prev") in (None, ""), "the thread pointer moved without a known answer"
    assert len(tg.sent) == 1 and tg.sent[0][1] == bridge_core.CLIENT_BLOCKED
    assert any("recovery-blocked" in ln for ln in logs), logs
    d.cleanup()
    print("  PASS B2 unknown turn outcome -> RECOVERY_BLOCKED, no second model run")


def test_b3_crash_after_the_model_completes_before_the_id_is_persisted():
    """The worst window: IronClaw ran and was paid for, the bridge never learned the id.
    Measured on the pinned runtime, this is unrecoverable — so it must be BLOCKED, never
    silently re-run."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-commit_turn")
    assert len(ic.runs) == 1, "the model should have run before the crash"
    tg, row, cursor, thread = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1, f"a second turn was billed: {ic.runs}"
    assert row["state"] == bs.RECOVERY_BLOCKED
    assert cursor == 2
    assert len(tg.sent) == 1 and tg.sent[0][1] == bridge_core.CLIENT_BLOCKED
    d.cleanup()
    print("  PASS B3 completed-but-unrecorded -> BLOCKED, never a second billed turn")


def test_b4_crash_after_turn_completed_before_delivery():
    """The answer is durable. The redelivery must deliver THAT answer, fetched by id."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-send")
    st = bs.BridgeState(db)
    assert st.update_row(1)["state"] == bs.DELIVERY_STARTED
    assert st.update_row(1)["response_id"] == "resp_001"
    assert st.thread_row(GID)["prev"] == "resp_001", "pointer and response id disagree"
    st.close()

    tg, row, cursor, _ = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1, "the model ran again instead of the answer being fetched"
    assert ic.fetches == ["resp_001"], ic.fetches
    assert len(tg.sent) == 1 and tg.sent[0][1] == "ANSWER#1 to what should we look at?"
    assert row["state"] == bs.DELIVERED and cursor == 2
    d.cleanup()
    print("  PASS B4 completed-not-delivered -> the stored answer is fetched and sent")


def test_b5_crash_after_telegram_accepts_before_delivered_is_persisted():
    """THE RESIDUAL WINDOW. Telegram has the message; the bridge never wrote that down. The
    client WILL get a duplicate — and it must be the IDENTICAL one, never a new answer."""
    d, db = _tmp()
    ic = FakeIronclaw()
    tg1, _ = run_crashing(db, ic, [[upd(1)]], at="after-send")
    assert len(tg1.sent) == 1
    first = tg1.sent[0][1]

    tg2, row, cursor, _ = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1, f"a second model turn ran: {ic.runs}"
    assert len(tg2.sent) == 1, tg2.sent
    assert tg2.sent[0][1] == first, ("the duplicate differs from the original — this is the "
                                     "exact failure the tranche exists to remove")
    assert row["state"] == bs.DELIVERED and cursor == 2
    d.cleanup()
    print("  PASS B5 residual window yields an IDENTICAL duplicate, never a new answer")


def test_b6_delivered_and_offset_are_one_transaction():
    """Boundary 6 cannot exist, and this proves it rather than asserting it in prose: a crash
    inside that write leaves NEITHER the delivered state nor the advanced cursor."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-note_delivered")
    st = bs.BridgeState(db)
    row = st.update_row(1)
    assert row["state"] != bs.DELIVERED, "delivered without the cursor advancing"
    assert st.cursor != 2, "cursor advanced without the delivered state"
    st.close(); d.cleanup()
    print("  PASS B6 DELIVERED and the next offset commit atomically (no split state)")


def test_b7_crash_after_offset_persisted_before_telegram_is_acknowledged():
    """The cursor is durable but Telegram has not been told, so it redelivers. No rerun, no
    resend — just advance toward acknowledgement."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="after-note_delivered")
    st = bs.BridgeState(db)
    assert st.cursor == 2 and st.update_row(1)["state"] == bs.DELIVERED
    st.close()

    tg, row, cursor, _ = run_clean(db, ic, [[upd(1)]])
    assert len(ic.runs) == 1 and ic.fetches == [], "recovery work happened for a delivered update"
    assert tg.sent == [], f"an already-delivered update was sent again: {tg.sent}"
    # The restart's first poll carries the durable cursor, which is what acknowledges the
    # update to Telegram — so by the time the (fake) redelivery is handled it has already
    # advanced DELIVERED -> ACKED. Either is correct here; what must not happen is a rerun or
    # a resend, both asserted above.
    assert row["state"] in (bs.DELIVERED, bs.ACKED), row["state"]
    assert cursor == 2
    d.cleanup()
    print("  PASS B7 delivered-but-unacknowledged -> no rerun, no resend")


def test_b8_crash_in_the_middle_of_a_batch():
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1), upd(2), upd(3)]], at="before-model", nth=2)
    assert len(ic.runs) == 1, ic.runs
    tg, _, _, _ = run_clean(db, ic, [[upd(1), upd(2), upd(3)]])
    st = bs.BridgeState(db)
    assert st.update_row(1)["state"] in ANSWERED, st.update_row(1)["state"]
    assert st.update_row(2)["state"] == bs.RECOVERY_BLOCKED, "update 2 was in flight and must block"
    assert st.update_row(3)["state"] in ANSWERED, "update 3 never started and must be answered"
    assert st.cursor == 4
    assert len(ic.runs) == 2, f"expected one rerun (update 3 only), got {ic.runs}"
    st.close(); d.cleanup()
    print("  PASS B8 mid-batch crash: done stays done, in-flight blocks, untouched runs")


def test_b9_crash_during_compaction_loses_nothing():
    d, db = _tmp()
    ic = FakeIronclaw()
    run_clean(db, ic, [[upd(i)] for i in range(1, 6)], polls=6)
    st = bs.BridgeState(db)
    before = st.counts_by_state()
    st.close()
    run_crashing(db, ic, [[]], at="before-compact")
    st = bs.BridgeState(db)
    assert st.counts_by_state() == before, "compaction crash changed durable state"
    assert st.cursor == 6
    st.close(); d.cleanup()
    print("  PASS B9 a crash during compaction leaves the journal intact")


def test_b9b_compaction_never_drops_an_unacknowledged_update():
    """Compaction is bounded by what Telegram has been TOLD, not by what the bridge has done.
    Dropping a row Telegram can still redeliver is the duplicate defect, reintroduced by
    tidiness."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_clean(db, ic, [[upd(1)]])              # delivered; cursor=2 but never acknowledged
    st = bs.BridgeState(db)
    assert st.cursor_acked is None
    assert st.compact(retain=0) == 0, "compacted before Telegram acknowledged anything"
    st.close(); d.cleanup()
    print("  PASS B9b compaction waits for Telegram acknowledgement, not local progress")


def test_b10_unreadable_and_unknown_version_journals_fail_closed():
    d = tempfile.TemporaryDirectory()
    root = pathlib.Path(d.name)
    corrupt = root / "corrupt.db"
    corrupt.write_bytes(b"this is not a database" * 10)
    try:
        bs.BridgeState(corrupt)
        raise AssertionError("a corrupt store was opened")
    except bs.StateError as e:
        assert "do NOT delete it blind" in str(e), e

    future = root / "future.db"
    st = bs.BridgeState(future)
    st.meta_set("schema_version", "99")
    st.close()
    try:
        bs.BridgeState(future)
        raise AssertionError("an unknown schema version was accepted")
    except bs.StateError as e:
        assert "99" in str(e) and "Refusing to run" in str(e), e

    legacy = root / "bridge-threads.json"
    legacy.write_text(json.dumps({GID: {"prev": "r", "supplied": ["A-1"]}}))
    try:
        bs.migrate_from_json(legacy, root / "from-legacy.db")
        raise AssertionError("a pre-versioning JSON file was migrated silently")
    except bs.LegacyStateError:
        pass
    d.cleanup()
    print("  PASS B10 corrupt, unknown-version and pre-versioning state all fail closed")


def test_b10b_the_store_is_0600():
    d, db = _tmp()
    st = bs.BridgeState(db)
    st.close()
    import stat
    assert stat.S_IMODE(db.stat().st_mode) == 0o600, oct(db.stat().st_mode)
    d.cleanup()
    print("  PASS B10b the store is not group- or world-readable")


def test_b11_an_in_flight_row_whose_group_left_the_registry_is_not_IGNORED():
    """ROUTING DECIDES WHETHER TO ANSWER, NOT WHAT ALREADY HAPPENED.

    Crash at before-send: the row is DELIVERY_STARTED with a durable response_id, i.e. an answer
    that exists and was very likely billed. Restart with that group absent from the registry
    snapshot — the operator move `_save_threads` describes ("moving a client env aside,
    restarting, and putting it back"), a deprovision-while-in-flight, or one malformed env that
    `load_clients` drops.

    That state is not terminal and not a delivery-recovery state, so it fell through to the
    summon test and `summoned()` returned None. The row became IGNORED: no error_code, cursor
    advanced past it, uncountable by `health()`, `doctor` green, and unrecoverable afterwards.
    The one thing the update demonstrably was NOT is "not addressed to us"."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-send")
    st = bs.BridgeState(db)
    assert st.update_row(1)["state"] == bs.DELIVERY_STARTED, st.update_row(1)["state"]
    assert st.update_row(1)["response_id"], "no answer to lose — wrong crash boundary"
    st.close()

    logs = []
    b, _tg, _tw, st = boot(db, ic, [[upd(1)]], logs=logs)
    b.groups = {}                      # the registry no longer routes this group
    b.threads = {}
    out = b.handle_update(upd(1))
    row = st.update_row(1)
    st.close(); d.cleanup()

    assert out == "recovery-blocked", out
    assert row["state"] == bs.RECOVERY_BLOCKED, row["state"]
    assert row["error_code"] == "route_lost_in_flight", row["error_code"]
    assert any("recovery-blocked" in line for line in logs), logs
    print("  PASS B11 an in-flight row whose route vanished is RECOVERY_BLOCKED, never IGNORED")


def test_b11b_a_DELIVERED_row_whose_route_vanished_keeps_its_delivery_evidence():
    """The other half, and it must NOT raise an alarm. A DELIVERED row is not in flight — the
    answer went out and only the Telegram offset is unacknowledged — so recording it
    RECOVERY_BLOCKED would be a false alarm, and recording it IGNORED would overwrite
    `delivered_at` and the response id. Those two fields are how a duplicate-delivery report
    from a client is diagnosed days later, which is the reason compaction retains terminal rows
    at all. DELIVERED needs no routing, so it is answered before routing is consulted."""
    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="after-note_delivered")
    st = bs.BridgeState(db)
    before = dict(st.update_row(1))
    st.close()
    assert before["state"] == bs.DELIVERED, before["state"]

    b, _tg, _tw, st = boot(db, ic, [[upd(1)]])
    b.groups = {}
    b.threads = {}
    out = b.handle_update(upd(1))
    after = dict(st.update_row(1))
    st.close(); d.cleanup()

    assert out == "already-delivered", out
    assert after["state"] == bs.DELIVERED, after["state"]
    assert after["delivered_at"] == before["delivered_at"], "the delivery timestamp was rewritten"
    assert after["response_id"] == before["response_id"], "the answer's id was lost"
    print("  PASS B11b a delivered row whose route vanished keeps its delivery evidence")


def test_b12_a_RECOVERY_BLOCKED_row_survives_compaction():
    """The alarm must outlive the noise. `health()` derives its "needs operator reconciliation"
    reason purely from `counts_by_state()`, so age-compacting the row DELETES the alarm — on a
    bridge doing ~600 updates between operator checks, the record that a turn may have been
    billed and never delivered vanished and the gate went green with nobody having acted.
    DELIVERY_RETRY and DELIVERY_RECONCILE were already excluded for this exact reason."""
    d, db = _tmp()
    st = bs.BridgeState(db)
    st.note_received(1000, GID, 1000)
    st.note_terminal(1000, bs.RECOVERY_BLOCKED, "turn_outcome_unknown")
    for uid in range(1001, 1600):
        st.note_received(uid, GID, uid)
        st.note_terminal(uid, bs.IGNORED)
    st.mark_cursor_acked()
    st.compact()
    row = st.update_row(1000)
    counts = st.counts_by_state()
    st.close(); d.cleanup()

    assert row is not None, "the RECOVERY_BLOCKED row was compacted away"
    assert row["state"] == bs.RECOVERY_BLOCKED, row["state"]
    assert counts.get(bs.RECOVERY_BLOCKED) == 1, counts
    print("  PASS B12 RECOVERY_BLOCKED is never age-compacted, so health() keeps reporting it")


if __name__ == "__main__":
    # Discovered, not listed — and PRESENT AT ALL, which is the defect this replaces. The four
    # suites split out of the delivery monolith carried no runner, so `python3 test_bridge_recovery.py` printed
    # nothing and exited 0: the documented local gate in CONTRIBUTING.md scored silence as
    # success across 14 crash-boundary and delivery tests. `unittest` collects none of them
    # either (bare functions, not TestCase), so pytest was the only thing on any path that ran
    # them. globals() preserves definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL BRIDGE RECOVERY TESTS PASS")
