"""Behavior-focused cases split from the bridge crash/recovery suite."""
try:
    from ._bridge_delivery_support import *
except ImportError:
    from _bridge_delivery_support import *

def test_state_paths_resolve_at_call_time_not_import_time():
    """REGRESSION. These were module constants captured at import, so a test that redirected
    BRIDGE_STATE afterwards silently opened the DEFAULT store — and opening it MIGRATES the
    operator's real thread file. A stale read would have been harmless; a side effect on a live
    host is not."""
    d = tempfile.TemporaryDirectory()
    root = pathlib.Path(d.name)
    default = pathlib.Path(os.environ.get("AGENCY_DIR", os.path.expanduser("~/.agency")))
    watched = [default / f"bridge-threads{suffix}" for suffix in (".db", ".db-wal", ".db-shm")]
    before = {path: (path.exists(), path.stat().st_size if path.exists() else None,
                     path.stat().st_mtime_ns if path.exists() else None) for path in watched}
    saved = os.environ.get("BRIDGE_STATE")
    saved_db = os.environ.pop("BRIDGE_STATE_DB", None)
    try:
        os.environ["BRIDGE_STATE"] = str(root / "redirected.json")
        assert tb.state_json_path() == root / "redirected.json"
        assert tb.state_db_path() == root / "redirected.db", tb.state_db_path()
        st = tb.open_state()
        st.close()
        assert (root / "redirected.db").exists(), "open_state did not honour the redirect"
        after = {path: (path.exists(), path.stat().st_size if path.exists() else None,
                        path.stat().st_mtime_ns if path.exists() else None) for path in watched}
        assert after == before, "redirected open_state touched the default operator store"
    finally:
        if saved is None:
            os.environ.pop("BRIDGE_STATE", None)
        else:
            os.environ["BRIDGE_STATE"] = saved
        if saved_db is not None:
            os.environ["BRIDGE_STATE_DB"] = saved_db
        d.cleanup()
    print("  PASS state paths follow BRIDGE_STATE at call time, not at import")


def test_agency_dir_relocates_all_default_bridge_state():
    saved = {key: os.environ.get(key) for key in
             ("AGENCY_DIR", "BRIDGE_STATE", "BRIDGE_STATE_DB")}
    d = tempfile.TemporaryDirectory()
    root = pathlib.Path(d.name)
    try:
        os.environ["AGENCY_DIR"] = str(root)
        os.environ.pop("BRIDGE_STATE", None)
        os.environ.pop("BRIDGE_STATE_DB", None)
        assert tb.state_json_path() == root / "bridge-threads.json"
        assert tb.state_db_path() == root / "bridge-threads.db"
        # There is ONE resolver, and it is the one asserted above. `BridgeState` carried a
        # second: `BRIDGE_STATE_DB or agency_dir("bridge-state.db")`, which ignored BRIDGE_STATE
        # and named a file the product never opens — so `BridgeState()` here used to return
        # `bridge-state.db` while the bridge served from `bridge-threads.db`, and this test
        # asserted that divergence as if it were the relocation working. It now refuses.
        try:
            bs.BridgeState()
            raise AssertionError("BridgeState() guessed a path instead of refusing")
        except bs.StateError as e:
            assert "explicit database path" in str(e), e
        state = bs.BridgeState(tb.state_db_path())
        assert state.path == root / "bridge-threads.db"
        state.close()
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        d.cleanup()
    print("  PASS AGENCY_DIR relocates default bridge JSON and SQLite state")


def test_fetch_is_scoped_to_the_tenants_own_credentials():
    """A response id is not a capability — the server refuses another tenant (measured live in
    multi/verify/test_responses_recovery.py, 404). What this pins offline is the half the bridge
    controls: it must present THAT tenant's own member token, never another's."""
    seen = {}

    class RecordingTurns:
        @staticmethod
        def run(thread, text, speaker=None, idempotency_key=None, budget=None):
            raise AssertionError("recovery must not run a turn")

        @staticmethod
        def fetch(client, response_id):
            seen["slug"] = client.slug
            seen["token"] = client.ironclaw_token
            return "recovered answer"

    d, db = _tmp()
    ic = FakeIronclaw()
    run_crashing(db, ic, [[upd(1)]], at="before-send")
    st = bs.BridgeState(db)
    tg = FakeTelegram([[upd(1)]])
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st), telegram=tg,
                          turns=RecordingTurns(), state=st, clock=_Clock(),
                          log=lambda *_: None, budget_seconds=5)
    b.poll_once()
    assert seen["slug"] == CL.slug and seen["token"] == CL.ironclaw_token, seen
    st.close(); d.cleanup()
    print("  PASS a recovered response is fetched with that tenant's own credentials")


def test_health_is_unhealthy_while_a_turn_is_past_its_deadline():
    """The blind spot this replaces: a wedged bridge logs no poll errors (it is not polling)
    and `is-active` reads green, so the old watchdog reported healthy while every tenant was
    deaf. Health has to compare progress against the deadline."""
    now = HEALTH_NOW
    wedged = {"heartbeat_at": iso(-5), "last_poll_ok_at": iso(-600),
              "inflight_update_id": 7, "inflight_gid": GID, "inflight_stage": "turn",
              "inflight_deadline_at": iso(-300), "counts": {}}
    ok, reasons = bridge_core.health(wedged, now, budget_seconds=180)
    assert not ok, "a bridge wedged past its deadline reported healthy"
    assert any("past its deadline" in r for r in reasons), reasons
    assert any("not receiving" in r for r in reasons), reasons

    healthy = {"heartbeat_at": iso(-2), "last_poll_ok_at": iso(-20),
               "inflight_update_id": None, "counts": {}}
    ok2, r2 = bridge_core.health(healthy, now, budget_seconds=180)
    assert ok2, r2

    # Busy is not wedged: inside the budget, a gap in polling is explained.
    busy = {"heartbeat_at": iso(-2), "last_poll_ok_at": iso(-100),
            "inflight_update_id": 8, "inflight_stage": "turn",
            "inflight_deadline_at": iso(+80), "counts": {}}
    ok3, r3 = bridge_core.health(busy, now, budget_seconds=180)
    assert ok3, r3
    print("  PASS health separates wedged from busy, and never reads alive as working")


def test_health_gives_a_starting_bridge_a_grace_window():
    """REGRESSION, found on the production rollout. `last_poll_ok_at` is written when
    getUpdates RETURNS, so a correctly running bridge has no poll recorded for its first
    long-poll cycle. Reporting that as unhealthy fires a false alarm on EVERY restart — and an
    alarm that cries wolf on every restart is worse than none, because it trains the operator
    to ignore the one signal that means the bridge has stopped receiving."""
    now = HEALTH_NOW
    just_started = {"heartbeat_at": iso(-5), "started_at": iso(-5),
                    "last_poll_ok_at": None, "counts": {}}
    ok, why = bridge_core.health(just_started, now, budget_seconds=180)
    assert ok, f"a bridge 5s into its first long poll reported unhealthy: {why}"

    # ...but the grace is bounded: a process that has been up for ages and has still never
    # polled is genuinely broken, and must not hide behind the same rule.
    never_polled = {"heartbeat_at": iso(-5), "started_at": iso(-3600),
                    "last_poll_ok_at": None, "counts": {}}
    ok2, why2 = bridge_core.health(never_polled, now, budget_seconds=180)
    assert not ok2, "a bridge that never polled in an hour reported healthy"
    assert any("ever been recorded" in r for r in why2), why2
    assert any("3600s ago" in r for r in why2), why2

    # No start time at all is not a free pass either.
    unknown = {"heartbeat_at": iso(-5), "last_poll_ok_at": None, "counts": {}}
    ok3, why3 = bridge_core.health(unknown, now, budget_seconds=180)
    assert not ok3, "a snapshot with neither poll nor start time reported healthy"
    print("  PASS a starting bridge gets a bounded grace; a never-polling one does not")


def test_health_tolerates_intermittent_poll_timeouts():
    """Production observation: ~148 long-poll read timeouts over four days with no outage. The
    health model must distinguish transport noise from loss of forward progress — it reads
    PROGRESS timestamps, never error counts, so a bridge that keeps completing polls stays
    healthy however many individual polls failed in between."""
    now = HEALTH_NOW
    noisy_but_progressing = {"heartbeat_at": iso(-3), "started_at": iso(-345600),
                             "last_poll_ok_at": iso(-30), "counts": {}}
    ok, why = bridge_core.health(noisy_but_progressing, now, budget_seconds=180)
    assert ok, f"intermittent timeouts were treated as an outage: {why}"
    print("  PASS intermittent poll timeouts do not trip health while polls still complete")


def test_health_is_unhealthy_when_the_store_cannot_be_read_or_has_blocked_work():
    now = HEALTH_NOW
    ok, reasons = bridge_core.health({"unreadable": "DatabaseError"}, now, 180)
    assert not ok and "could not be read" in reasons[0], reasons
    ok2, r2 = bridge_core.health({}, now, 180)
    assert not ok2, "an absent snapshot must not read as healthy"

    blocked = {"heartbeat_at": iso(-2), "last_poll_ok_at": iso(-10),
               "counts": {bs.RECOVERY_BLOCKED: 2}}
    ok3, r3 = bridge_core.health(blocked, now, 180)
    assert not ok3 and any("RECOVERY_BLOCKED" in r for r in r3), r3
    print("  PASS an unreadable store, a missing snapshot, and blocked work are all unhealthy")


def test_the_turn_budget_bounds_a_single_update():
    """Tier 2: one wall-clock budget for the whole turn, replacing a nested retry stack whose
    product was a 15-30 minute ceiling nobody chose."""
    import time as _t
    token = ing._TURN_CTX.set({"key": "k", "deadline": _t.monotonic() - 1})
    try:
        try:
            ing._check_budget(request_sent=True)
            raise AssertionError("an expired budget did not raise")
        except ing.TurnBudgetExceeded as e:
            assert e.request_sent is True, "the caller cannot tell whether a turn was billed"
    finally:
        ing._TURN_CTX.reset(token)
    # ...and with no budget in force (the dev oracle, the proofs) nothing is bounded.
    assert ing._remaining() is None
    print("  PASS the turn budget is enforced and carries whether the request was sent")


def test_liveness_distinguishes_running_from_recently_healthy():
    """A stopped bridge must not report healthy just because its heartbeat is young.

    OBSERVED, not theorised: during a restart on the operator workstation `ironworks doctor`
    printed `[x] bridge.progress` while no bridge process existed at all. Nothing was broken --
    HEARTBEAT_GRACE_SECONDS is 120 so a busy turn is not a false alarm, and for those 120
    seconds a stopped bridge and a working one write an identical snapshot. On a systemd host
    that window is covered by `Restart=always`; where nothing restarts the process it is the
    whole difference between a signal and a reassurance.

    So liveness became an explicit input rather than something inferred from freshness."""
    now = HEALTH_NOW
    fresh = {"heartbeat_at": iso(-5), "last_poll_ok_at": iso(-5),
             "started_at": iso(-600), "pid": 4242, "counts": {}}

    # 1. running -> PASS
    ok, why = bridge_core.health(fresh, now, 180, pid_alive=True)
    assert ok, why

    # 2. stopped INSIDE the heartbeat grace -> FAIL. This is the defect being closed.
    ok, why = bridge_core.health(fresh, now, 180, pid_alive=False)
    assert not ok, "a stopped bridge passed on a five-second-old heartbeat"
    assert "no bridge process is running here" in why[0], why
    assert "not evidence" in why[0] or "before stopping" in why[0], \
        "the reason must say the heartbeat is stale evidence, or an operator reads it as fine"

    # 3. stale heartbeat -> FAIL even when the process IS alive (a wedged loop; unchanged)
    stale = {"heartbeat_at": iso(-900), "last_poll_ok_at": iso(-900),
             "started_at": iso(-1800), "pid": 4242, "counts": {}}
    ok, why = bridge_core.health(stale, now, 180, pid_alive=True)
    assert not ok and any("heartbeat" in r for r in why), why

    # 4. liveness undetermined -> exactly the pre-existing behaviour, both directions
    assert bridge_core.health(fresh, now, 180, pid_alive=None)[0] is True
    assert bridge_core.health(stale, now, 180, pid_alive=None)[0] is False
    assert bridge_core.health(fresh, now, 180)[0] is True, "the argument must stay optional"
    print("  PASS liveness separates 'running' from 'recently healthy'")


def test_the_bridge_records_and_clears_its_pid():
    """The check is only as good as the fact it reads. Recorded at startup so a reader can find
    the process, and CLEARED on a clean stop so its absence is positive evidence rather than a
    stale number pointing at whatever now holds that pid."""
    d, db = _tmp()
    try:
        b, _tg, _tw, st = boot(str(db), FakeIronclaw(), [[]])
        b.stopping = True                    # run() records, then unwinds immediately
        b.run()
        snap = st.progress_snapshot()
        assert snap["started_at"], "run() recorded no start"
        assert snap["pid"] in (None, "None"), \
            f"pid survived a clean stop ({snap['pid']!r}) -- a later reader would trust a corpse"
        st.close()
    finally:
        d.cleanup()
    print("  PASS the bridge records its pid at startup and clears it on a clean stop")


def test_resetting_a_conversation_keeps_the_delivery_journal():
    """`drop_thread` deletes the journal as well, which is right for deprovisioning and wrong
    for everything else. Wanting a clean thread while keeping the record of what was delivered
    had no supported operation at all -- it was done by editing the store by hand.

    The two must stay distinguishable: the journal is the evidence that a turn was answered,
    and it is exactly what an operator needs AFTER a reset, not before it."""
    d, db = _tmp()
    try:
        ic = FakeIronclaw()
        run_clean(str(db), ic, [[upd(1)]])                 # one real turn -> thread + journal
        st = bs.BridgeState(str(db))
        assert st.thread_row(GID) is not None and st.update_row(1) is not None

        n = st.reset_thread(GID)
        assert n == 1, "reset removed no thread row"
        assert st.thread_row(GID) is None, "the conversation pointer survived a reset"
        assert st.update_row(1) is not None, \
            "reset_thread destroyed the delivery journal -- that is drop_thread's job, not this one"
        assert st.cursor is not None, "the Telegram cursor must survive a conversation reset"

        # ...and resetting a group that never spoke is a no-op, not an error
        assert st.reset_thread("-100999999") == 0
        st.close()
    finally:
        d.cleanup()
    print("  PASS reset_thread clears the conversation and keeps the journal")


def test_a_departing_bridge_does_not_clear_its_replacements_pid():
    """The restart race, found by observation rather than reasoning.

    A stopping bridge may still be unwinding a 25-second long poll while its replacement has
    already recorded its own pid. A blind `pid=None` on the way out then wipes the LIVE
    process's entry -- and the liveness check degrades silently to the pre-fix blind behaviour,
    which is worse than never having added it, because the store now looks deliberately empty.
    Only the owner of the slot may release it."""
    d, db = _tmp()
    try:
        b, _tg, _tw, st = boot(str(db), FakeIronclaw(), [[]])

        def replacement_claims_the_slot_mid_poll():
            # what a restart looks like from inside the departing process: while it is still in
            # its long poll, the new process starts and records ITS pid over ours.
            st.note_progress(pid=999999)
            b.stopping = True

        b.poll_once = replacement_claims_the_slot_mid_poll
        b.run()                               # ...then we unwind and try to clean up
        assert str(st.progress_snapshot()["pid"]) == "999999", \
            "a departing bridge cleared the pid its replacement had already recorded"
        st.close()
    finally:
        d.cleanup()
    print("  PASS a departing bridge leaves its replacement's pid alone")

if __name__ == "__main__":
    # Discovered, not listed — and PRESENT AT ALL, which is the defect this replaces. The four
    # suites split out of the delivery monolith carried no runner, so `python3 test_bridge_operations.py` printed
    # nothing and exited 0: the documented local gate in CONTRIBUTING.md scored silence as
    # success across 12 crash-boundary and delivery tests. `unittest` collects none of them
    # either (bare functions, not TestCase), so pytest was the only thing on any path that ran
    # them. globals() preserves definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL BRIDGE OPERATIONS TESTS PASS")
