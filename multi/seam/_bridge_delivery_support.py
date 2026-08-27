#!/usr/bin/env python3
"""Crash boundaries: what the bridge does when it dies mid-flight, at every point it can.

Run all cases through `./deploy/ironworks test`.

The defect this suite exists for: the Telegram offset was a local variable, advanced in memory
and only communicated on the NEXT poll, so nothing in a batch was confirmed until the whole
batch finished. A crash — or a restart to pick up a new tenant, which is routine — replayed
every update in it. And because the thread pointer was persisted BEFORE delivery, the replay
did not repeat the answer: it ran a second billed turn, chained onto the answer the client had
already seen, with `supplied` already marked so no records were re-injected. The client got a
second, different, weaker answer to one question.

WHAT IS ASSERTED, for every boundary: how many model turns ran, which idempotency keys were
used, which response ids, how many sends happened and whether their content was IDENTICAL, the
final thread pointer, the final update state, the final persisted cursor, and whether an
operator-visible degraded state was emitted.

A crash here is `Crash`, a BaseException — not an Exception. `except Exception:` handlers in the
bridge must not be able to swallow it, because a real SIGKILL is not catchable either.
"""
import json
import os
import pathlib
import tempfile
os.environ.setdefault("TELEGRAM_BOT_USERNAME", "fixture_bot")

try:
    from . import bridge_core
    from . import bridge_state as bs
    from . import context_ingress as ing
    from . import telegram_bridge as tb
except ImportError:  # direct execution compatibility
    import bridge_core
    import bridge_state as bs
    import context_ingress as ing
    import telegram_bridge as tb

GID = "-100900001"
CL = ing.ClientConfig(slug="testco", ironclaw_token="member-token",
                      account_token="org-token", telegram_group_id=GID,
                      persona="TEST PERSONA (fixture)")
GROUPS = {GID: CL}


class Crash(BaseException):
    """A process death. BaseException on purpose: `except Exception` must not catch it."""


class Tripwire:
    """Fires once, at the named boundary."""

    def __init__(self, at=None, nth=1):
        self.at, self.nth, self.seen, self.fired = at, nth, {}, False

    def __call__(self, name):
        if name != self.at:
            return
        self.seen[name] = self.seen.get(name, 0) + 1
        if self.seen[name] == self.nth:
            self.fired = True
            raise Crash(f"crash at {name}")


class FakeIronclaw:
    """The server. Survives bridge restarts, because it is a different process."""

    def __init__(self):
        self.runs = []            # (idempotency_key, text) per ACCEPTED turn
        self.responses = {}       # response_id -> reply text
        self.fetches = []
        self.fetch_fails = False

    def run(self, thread, text, speaker=None, idempotency_key=None, budget=None, tw=None):
        if tw:
            tw("before-model")
        rid = "resp_%03d" % (len(self.runs) + 1)
        reply = f"ANSWER#{len(self.runs) + 1} to {text}"
        self.runs.append((idempotency_key, text))
        self.responses[rid] = reply
        thread.prev = rid                        # what the real turn() does on success
        thread.ever_supplied = True
        thread.last_turn_at = "2026-08-24T00:00:00+00:00"
        if tw:
            tw("after-model")
        return reply

    def fetch(self, client, response_id):
        self.fetches.append(response_id)
        if self.fetch_fails:
            raise RuntimeError("response unavailable (retention or outage)")
        if response_id not in self.responses:
            raise RuntimeError(f"unknown response {response_id}")
        return self.responses[response_id]


class FakeTelegram:
    def __init__(self, batches, ic=None, tw=None):
        self.batches, self.ic, self.tw = list(batches), ic, tw
        self.sent, self.polls = [], []

    def get_updates(self, offset=None, timeout=25):
        self.polls.append(offset)
        if self.tw:
            self.tw("after-getupdates")
        return {"result": self.batches.pop(0) if self.batches else []}

    def send_message(self, chat_id, text):
        if self.tw:
            self.tw("before-send")
        self.sent.append((str(chat_id), text))
        if self.tw:
            self.tw("after-send")           # Telegram HAS accepted it by now


class StateProxy:
    """Wraps the real store so a crash can be injected between two durable writes."""

    def __init__(self, inner, tw):
        object.__setattr__(self, "_inner", inner)
        object.__setattr__(self, "_tw", tw)

    def __getattr__(self, name):
        attr = getattr(self._inner, name)
        if not callable(attr):
            return attr

        def wrapped(*a, **k):
            self._tw("before-" + name)
            out = attr(*a, **k)
            self._tw("after-" + name)
            return out
        return wrapped


def _turns(ic, tw):
    class T:
        @staticmethod
        def run(thread, text, speaker=None, idempotency_key=None, budget=None):
            return ic.run(thread, text, speaker=speaker,
                          idempotency_key=idempotency_key, budget=budget, tw=tw)

        @staticmethod
        def fetch(client, response_id):
            return ic.fetch(client, response_id)
    return T()


class _Clock:
    _n = [0.0]

    def monotonic(self):
        self._n[0] += 0.001
        return self._n[0]

    def sleep(self, s):
        self._n[0] += s

    def now_iso(self, offset=0):
        return "2026-08-24T00:00:00+00:00"


def boot(db, ic, batches, at=None, nth=1, logs=None):
    """One bridge 'process'. Returns (bridge, telegram, tripwire, state)."""
    tw = Tripwire(at, nth)
    st = bs.BridgeState(db)
    tg = FakeTelegram(batches, ic, tw)
    b = tb.TelegramBridge(groups=GROUPS, threads=tb._load_threads(GROUPS, st),
                          telegram=tg, turns=_turns(ic, tw), state=StateProxy(st, tw),
                          clock=_Clock(), log=(logs.append if logs is not None else lambda *_: None),
                          budget_seconds=5)
    return b, tg, tw, st


def upd(uid, text="what should we look at?", chat=GID, mid=None):
    return {"update_id": uid,
            "message": {"message_id": mid or uid, "chat": {"id": int(chat)},
                        "from": {"first_name": "Sam"},
                        "text": f"@fixture_bot {text}"}}


def run_crashing(db, ic, batches, at, nth=1, logs=None):
    b, tg, tw, st = boot(db, ic, batches, at, nth, logs)
    try:
        b.poll_once()
    except Crash:
        pass
    st.close()
    return tg, tw


def run_clean(db, ic, batches, logs=None, polls=1):
    b, tg, _, st = boot(db, ic, batches, logs=logs)
    for _ in range(polls):
        b.poll_once()
    out = (tg, st.update_row(1), st.cursor, dict(st.thread_row(GID) or {}))
    st.close()
    return out


ANSWERED = (bs.DELIVERED, bs.ACKED)   # DELIVERED, or promoted by a poll that acknowledged it

# A fixed clock for the health tests, and the one offset->timestamp helper they all wanted. It
# was written out five times, each with a function-local `import datetime` and the parameter
# named `offset`, `off` or `o`. Those tests read as timelines; the arithmetic is not the
# interesting part of any of them.
HEALTH_NOW = 1_800_000_000.0


def iso(offset, now=HEALTH_NOW):
    """`now + offset` as a UTC ISO-8601 stamp — a negative offset is "N seconds ago"."""
    import datetime
    return datetime.datetime.fromtimestamp(now + offset, datetime.timezone.utc).isoformat()


def _tmp():
    d = tempfile.TemporaryDirectory()
    return d, pathlib.Path(d.name) / "state.db"


# ── the happy path, so every crash test below has a baseline ──────────────────────────


__all__ = [name for name in globals() if not name.startswith("__")]
