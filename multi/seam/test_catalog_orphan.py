#!/usr/bin/env python3
"""A catalogued account whose context 404s must not be re-fetched on every turn, forever.

Run: python3 test_catalog_orphan.py   (from multi/seam)

THE DEFECT. `/list_accounts` and `/get_account_context` are two reads of the same store, and
they can disagree — a row deleted between them, a partial seed, a botched migration. When they
did, the seam dropped the `None` and never recorded the account, so it re-targeted on the very
next turn and the one after that. One wasted round trip per turn per orphan, forever, silently
breaking the "the book costs one fetch per thread" bound that the widening fallback depends on.
The old code said so in a comment and left it, because the fix is a CHOICE.

THE CHOICE MADE. A VERSIONED negative result. The 404 is recorded against the catalog version
that produced it, so:
  * an unchanged catalog row is not asked for again — the cost is bounded;
  * a CHANGED catalog row is asked for again — any repair in the store moves `updated_at`, so
    it heals by itself, with no operator action and no cache to flush;
  * with no version at all (an older Account Service), a bounded attempt count applies,
    because there is no event to key a retry on and "never again" cannot self-heal.

And two things it must NOT do: leak the inconsistency into the client's turn (they asked a
business question; our store disagreeing with itself is our problem), or count as context
having been supplied (which would defeat the data-starvation recovery).
"""
import os
try:
    from . import account_service as asvc
    from . import context_ingress as ing
except ImportError:
    import account_service as asvc
    import context_ingress as ing


def _seam(name):
    """Import a sibling seam module under BOTH invocations, from inside a test body.

    The module top above carries this as a try/except; the imports inside test bodies were
    missed when `multi/seam/__init__.py` landed, so `pytest multi/seam` — the per-subsystem
    command `CONTRIBUTING.md` documents — failed on them while the full `pytest -q` passed,
    because `deploy/lib/test_ironworks_cli.py` loads the console and `deploy/ironworks` puts
    `multi/seam` on `sys.path`. The seam suites were green on another directory's side
    effect rather than on their own imports."""
    import importlib
    try:
        return importlib.import_module("." + name, __package__)
    except (ImportError, TypeError):      # script invocation: no package context
        return importlib.import_module(name)


CL = ing.ClientConfig(slug="testco", ironclaw_token="t", account_token="a",
                      persona="TEST PERSONA (fixture)")

V1 = "2026-08-01T00:00:00+00:00"
V2 = "2026-08-02T00:00:00+00:00"


def _ctx(aid, version):
    return {"record_id": aid, "account": {"name": aid, "updated_at": version},
            "contacts": [], "activities": []}


class Harness:
    """Stubs the two store reads and the IronClaw post, and counts what was asked for."""

    def __init__(self, catalog, resolvable):
        self.catalog = catalog                 # [{account_id, name, updated_at}]
        self.resolvable = resolvable           # {aid: version} — everything else 404s
        self.fetched = []                      # every _get_context call, in order
        self.inputs = []                       # every envelope sent to the model
        self._saved = None

    def __enter__(self):
        self._saved = (asvc._svc, ing._get_context, ing._post_ironclaw)
        asvc._svc = lambda p, client=None: (
            {"accounts": self.catalog, "org": "o"} if "list_accounts" in p else {})

        def get_context(aid, client=None):
            self.fetched.append(aid)
            v = self.resolvable.get(aid)
            return _ctx(aid, v) if v is not None else None      # None == the service 404'd

        ing._get_context = get_context
        ing._post_ironclaw = lambda body, client=None: (
            self.inputs.append(body["input"]),
            {"id": "resp", "output": [{"type": "message",
                                       "content": [{"type": "output_text", "text": "ok"}]}]})[1]
        return self

    def __exit__(self, *a):
        asvc._svc, ing._get_context, ing._post_ironclaw = self._saved


def test_an_orphan_is_asked_for_once_not_every_turn():
    cat = [{"account_id": "GOOD-1", "name": "Good One", "updated_at": V1},
           {"account_id": "GHOST-9", "name": "Ghost Nine", "updated_at": V1}]
    with Harness(cat, {"GOOD-1": V1}) as h:
        th = ing.Thread(CL)
        for _ in range(4):
            ing.turn(th, "what should we look at?")
    assert h.fetched.count("GHOST-9") == 1, \
        f"the orphan was fetched {h.fetched.count('GHOST-9')} times across 4 turns"
    assert h.fetched.count("GOOD-1") == 1, "the healthy account was re-fetched — inject-once broke"
    print("  PASS an orphan costs one fetch per thread, not one per turn")


def test_a_moved_catalog_row_is_asked_for_again():
    """The negative result is versioned, so a repair in the store heals it with no operator
    action. This is the whole reason it is not a permanent suppression."""
    cat = [{"account_id": "GHOST-9", "name": "Ghost Nine", "updated_at": V1}]
    resolvable = {}
    with Harness(cat, resolvable) as h:
        th = ing.Thread(CL)
        ing.turn(th, "q1")
        ing.turn(th, "q2")
        assert h.fetched.count("GHOST-9") == 1, "asked again with nothing changed"
        # the store is repaired: the row now resolves, and its catalog version moved
        cat[0]["updated_at"] = V2
        resolvable["GHOST-9"] = V2
        ing.turn(th, "q3")
        assert h.fetched.count("GHOST-9") == 2, "a MOVED catalog row was not re-asked for"
        assert th.supplied.get("GHOST-9") == V2, "the healed account was not recorded as supplied"
        assert "GHOST-9" not in th.orphans, "the spent negative result was not cleared"
        ing.turn(th, "q4")
        assert h.fetched.count("GHOST-9") == 2, "re-fetched after healing — inject-once broke"
    print("  PASS a moved catalog row is re-asked for exactly once, and then heals")


def test_a_versionless_orphan_is_bounded_not_infinite():
    """An Account Service that emits no `updated_at` gives nothing to key a retry on. Asking
    forever is the defect; never asking again cannot self-heal. A small bound is the honest
    third option."""
    cat = [{"account_id": "GHOST-9", "name": "Ghost Nine"}]          # no updated_at
    with Harness(cat, {}) as h:
        th = ing.Thread(CL)
        for _ in range(8):
            ing.turn(th, "q")
    n = h.fetched.count("GHOST-9")
    assert n == ing.ORPHAN_MAX_UNVERSIONED_ATTEMPTS, \
        f"expected {ing.ORPHAN_MAX_UNVERSIONED_ATTEMPTS} bounded attempts, got {n}"
    assert n < 8, "unbounded: the orphan was asked for on every turn"
    print(f"  PASS a version-less orphan is asked for {n} times, then stops")


def test_the_client_never_sees_the_inconsistency():
    """Two reads of our own store disagreeing is an operator fact. The client asked a business
    question and gets a business answer; nothing about record ids or 404s enters the turn."""
    cat = [{"account_id": "GOOD-1", "name": "Good One", "updated_at": V1},
           {"account_id": "GHOST-9", "name": "Ghost Nine", "updated_at": V1}]
    with Harness(cat, {"GOOD-1": V1}) as h:
        ing.turn(ing.Thread(CL), "what should we look at?")
    envelope = h.inputs[0]
    assert "GOOD-1" in envelope, "the healthy account did not reach the model"
    for leak in ("GHOST-9", "404", "orphan", "catalog inconsistency"):
        assert leak not in envelope, f"{leak!r} reached the client-visible turn"
    print("  PASS the envelope carries the healthy record and nothing about the orphan")


def test_an_orphan_only_turn_does_not_count_as_context_supplied():
    """`ever_supplied` gates the data-starvation recovery: a thread that has had NO context must
    not chain to its data-starved history once records arrive. A 404 supplies nothing, so it
    must not flip that flag — otherwise the recovery is disabled by a store inconsistency."""
    cat = [{"account_id": "GHOST-9", "name": "Ghost Nine", "updated_at": V1}]
    with Harness(cat, {}):
        th = ing.Thread(CL)
        ing.turn(th, "q1")
        assert th.ever_supplied is False, "a 404-only turn marked the thread as having context"
        assert th.supplied == {}, "an unfetchable account was recorded as supplied"
    print("  PASS an orphan-only turn leaves ever_supplied False")


def test_the_orphan_record_survives_a_bridge_restart():
    """In-memory only, the fix would be undone by every restart: each one would re-ask for every
    orphan. The bridge persists the record with the rest of the thread state.

    The state path is passed in rather than set through the environment: reassigning the
    bridge's module-level STATE_PATH would leak into every test that ran after this one, which
    is exactly the process-global-mutable-state failure this file is not allowed to introduce.
    """
    import json
    import pathlib
    import tempfile
    os.environ.setdefault("TELEGRAM_BOT_USERNAME", "fixture_bot")
    bstate = _seam("bridge_state")
    tb = _seam("telegram_bridge")

    with tempfile.TemporaryDirectory() as d:
        st = bstate.BridgeState(pathlib.Path(d) / "state.db")
        gid = "-100900001"
        th = ing.Thread(CL)
        th.orphans = {"GHOST-9": (V1, 1)}
        th.last_turn_at = "2026-08-20T09:00:00+00:00"
        tb._save_threads({gid: th}, state=st)

        row = st.thread_row(gid)
        assert json.loads(row["orphans"]) == {"GHOST-9": [V1, 1]}, dict(row)
        assert row["last_turn_at"] == "2026-08-20T09:00:00+00:00", dict(row)

        back = tb._load_threads({gid: CL}, state=st)[gid]
        assert back.orphans == {"GHOST-9": (V1, 1)}, back.orphans
        assert back.last_turn_at == "2026-08-20T09:00:00+00:00"
        st.close()
    print("  PASS the orphan record and last-turn time survive a restart")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL CATALOG-ORPHAN TESTS PASS")
