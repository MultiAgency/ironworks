"""Cross-tenant bridge scheduling and global Telegram cursor safety."""
import datetime
import sqlite3
import tempfile
import threading
import time

try:
    from . import bridge_core
    from . import bridge_state as bs
    from . import context_ingress as ing
except ImportError:
    import bridge_core
    import bridge_state as bs
    import context_ingress as ing


class Clock:
    def now_iso(self, offset=0):
        return (datetime.datetime.now(datetime.timezone.utc)
                + datetime.timedelta(seconds=offset)).isoformat()

    @staticmethod
    def sleep(_seconds):
        return None


class Telegram:
    def __init__(self, updates):
        self.updates = updates
        self.sent = []

    def get_updates(self, offset=None, timeout=25):
        del offset, timeout
        updates, self.updates = self.updates, []
        return {"result": updates}

    def send_message(self, gid, text):
        self.sent.append((str(gid), text))


class HarnessBridge(bridge_core.Bridge):
    def summoned(self, msg):
        gid = str(msg["chat"]["id"])
        return (gid, msg["text"]) if gid in self.groups else None

    def addressed(self, msg):
        return msg.get("text")


def client(slug, gid):
    return ing.ClientConfig(slug=slug, telegram_group_id=str(gid),
                            ironclaw_token=f"{slug}-member", account_token=f"{slug}-org",
                            persona=f"persona for {slug}")


def update(uid, gid, text=None):
    return {"update_id": uid, "message": {"message_id": uid, "chat": {"id": gid},
            "from": {"first_name": "Sam"}, "text": text or f"message-{uid}"}}


def make_bridge(path, updates, turns, max_workers=4):
    clients = {str(gid): client(slug, gid) for slug, gid in (("alpha", -101), ("beta", -202))}
    state = bs.BridgeState(path)
    threads = {gid: ing.Thread(cfg) for gid, cfg in clients.items()}
    return (HarnessBridge(clients, threads, Telegram(updates), turns, state, Clock(),
                          lambda _s: None, budget_seconds=5, max_workers=max_workers), state)


def test_tenants_overlap_but_global_cursor_waits_for_earlier_update():
    alpha_started = threading.Event()
    release_alpha = threading.Event()
    beta_finished = threading.Event()

    class Turns:
        @staticmethod
        def run(thread, text, **_kwargs):
            if thread.client.slug == "alpha":
                alpha_started.set()
                assert release_alpha.wait(3)
            else:
                assert alpha_started.wait(3)
                beta_finished.set()
            thread.prev = f"response-{text}"
            return f"answer-{text}"

        @staticmethod
        def fetch(_client, response_id):
            return response_id

    with tempfile.TemporaryDirectory() as tmp:
        bridge, state = make_bridge(f"{tmp}/state.db", [update(10, -101), update(11, -202)], Turns())
        polling = threading.Thread(target=bridge.poll_once)
        polling.start()
        assert beta_finished.wait(3), "the later tenant was blocked behind the first tenant"
        deadline = time.monotonic() + 3
        while (state.update_row(11)["state"] != bs.DELIVERED
               and time.monotonic() < deadline):
            time.sleep(0.005)
        assert state.update_row(11)["state"] == bs.DELIVERED
        assert state.update_row(10)["state"] == bs.TURN_STARTED
        assert state.cursor == 10, "a later completion acknowledged an unfinished earlier update"
        release_alpha.set()
        polling.join(3)
        assert not polling.is_alive()
        assert state.cursor == 12
        state.close()


def test_one_tenant_remains_ordered_and_never_overlaps_itself():
    active = 0
    highest_active = 0
    seen = []
    guard = threading.Lock()

    class Turns:
        @staticmethod
        def run(thread, text, **_kwargs):
            nonlocal active, highest_active
            with guard:
                active += 1
                highest_active = max(highest_active, active)
                seen.append(text)
            time.sleep(0.01)
            thread.prev = f"response-{text}"
            with guard:
                active -= 1
            return text

        @staticmethod
        def fetch(_client, response_id):
            return response_id

    with tempfile.TemporaryDirectory() as tmp:
        bridge, state = make_bridge(
            f"{tmp}/state.db", [update(1, -101), update(2, -101), update(3, -101)], Turns())
        assert bridge.poll_once() == 3
        assert seen == ["message-1", "message-2", "message-3"]
        assert highest_active == 1
        assert state.cursor == 4
        state.close()


def test_worker_limit_is_bounded_and_invalid_configuration_fails_fast():
    class Turns:
        @staticmethod
        def run(thread, text, **_kwargs):
            thread.prev = text
            return text

        @staticmethod
        def fetch(_client, response_id):
            return response_id

    with tempfile.TemporaryDirectory() as tmp:
        bridge, state = make_bridge(f"{tmp}/state.db", [], Turns(), max_workers=1000)
        assert bridge.max_workers == 16
        state.close()
        try:
            make_bridge(f"{tmp}/bad.db", [], Turns(), max_workers="many")
        except ValueError as exc:
            assert "BRIDGE_MAX_WORKERS" in str(exc)
        else:
            raise AssertionError("invalid worker configuration was accepted")


def test_one_worker_crash_does_not_lose_a_completed_other_tenant():
    class Crash(BaseException):
        pass

    class Turns:
        @staticmethod
        def run(thread, text, **_kwargs):
            if thread.client.slug == "alpha":
                raise Crash("simulated process boundary")
            thread.prev = f"response-{text}"
            return text

        @staticmethod
        def fetch(_client, response_id):
            return response_id

    with tempfile.TemporaryDirectory() as tmp:
        bridge, state = make_bridge(f"{tmp}/state.db", [update(10, -101), update(11, -202)], Turns())
        try:
            bridge.poll_once()
        except Crash:
            pass
        else:
            raise AssertionError("the simulated process crash was swallowed")
        assert state.update_row(10)["state"] == bs.TURN_STARTED
        assert state.update_row(11)["state"] == bs.DELIVERED
        assert state.cursor == 10
        state.close()


def test_busy_database_waits_then_completes_without_reordering():
    class Turns:
        @staticmethod
        def run(thread, text, **_kwargs):
            thread.prev = f"response-{text}"
            return text

        @staticmethod
        def fetch(_client, response_id):
            return response_id

    with tempfile.TemporaryDirectory() as tmp:
        path = f"{tmp}/state.db"
        bridge, state = make_bridge(path, [update(1, -101), update(2, -202)], Turns())
        blocker = sqlite3.connect(path, isolation_level=None)
        blocker.execute("BEGIN IMMEDIATE")
        polling = threading.Thread(target=bridge.poll_once)
        polling.start()
        time.sleep(0.05)
        assert polling.is_alive(), "the external write lock did not exercise the busy path"
        blocker.execute("COMMIT")
        polling.join(3)
        assert not polling.is_alive()
        assert state.cursor == 3
        assert [state.update_row(uid)["state"] for uid in (1, 2)] == [bs.DELIVERED] * 2
        blocker.close()
        state.close()

if __name__ == "__main__":
    # Discovered, not listed — and PRESENT AT ALL, which is the defect this replaces. The four
    # suites split out of the delivery monolith carried no runner, so `python3 test_bridge_concurrency.py` printed
    # nothing and exited 0: the documented local gate in CONTRIBUTING.md scored silence as
    # success across 5 crash-boundary and delivery tests. `unittest` collects none of them
    # either (bare functions, not TestCase), so pytest was the only thing on any path that ran
    # them. globals() preserves definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL BRIDGE CONCURRENCY TESTS PASS")
