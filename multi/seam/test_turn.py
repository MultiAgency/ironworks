#!/usr/bin/env python3
"""One turn, end to end, against a fake instance. Run: python3 test_turn.py (from multi/seam)

WHAT A TURN OWES, and what each test pins. A turn must be billed at most once however the
network behaves; it must not chain onto a response the instance no longer has; it must leave NO
bookkeeping behind when it fails, because a `supplied` mark on a turn that never reached the
model makes the analyst permanently blind to those records; it must widen rather than guess when
nothing was named; and it must carry the tenant's persona and the tenant's credentials — and
only those — on every request.

THE FAILURES THESE STAND FOR ARE REAL, not hypothetical. A retry that created a second accepted
turn. A rejected continuity that reused its idempotency key and replayed the wrong answer. A
lost previous_response_id that killed the turn instead of self-healing. A store outage that
killed the turn instead of telling the model the records were briefly unavailable. Each
docstring below names its own.

SCOPE. The deterministic halves are tested where they live and with no instance at all:
`test_envelope.py` (which records, and how rendered), `test_registry.py` (who may be served).
The bridge owns `test_telegram_bridge.py` (routing and state) and the behavior-focused
`test_bridge_*` suites (delivery, recovery, operations, and concurrency).
"""
import contextlib, io, os, json, urllib.request
# This suite drives the seam against a FAKE instance, so it configures one outright.
# Not an import prop: `context_ingress` resolves IRONCLAW_API on use, so this is the
# value under test. Assigned, not `setdefault`, so a configured box cannot leak a real
# instance into a hermetic unit suite.
os.environ["IRONCLAW_API"] = "http://test.invalid"
try:
    from . import account_service as asvc
    from . import context_ingress as ing
except ImportError:
    import account_service as asvc
    import context_ingress as ing

# The one explicit test client: there is no ambient default client or persona any more
# (the env-pair fallback was removed) — every thread names its client.
CL = ing.ClientConfig(slug="testco", ironclaw_token="test-token",
                      account_token="test-account-token", persona="TEST PERSONA (fixture)")


class _Resp:
    def __init__(self, d): self._d = json.dumps(d).encode()
    def read(self): return self._d
    def __enter__(self): return self
    def __exit__(self, *a): pass


def test_retry_cannot_duplicate_a_turn():
    """An ambiguous post-send failure must NOT produce two accepted turns: the retry reuses the
    SAME idempotency-key, so IronClaw replays the prior result (at-most-once)."""
    keys = []
    orig = urllib.request.urlopen

    def fake_urlopen(req, timeout=None):
        keys.append(req.get_header("Idempotency-key"))
        if len(keys) == 1:
            raise TimeoutError("ambiguous post-send timeout (model may have processed)")
        return _Resp({"id": "resp_final", "output": []})

    urllib.request.urlopen = fake_urlopen
    try:
        d = ing._post_ironclaw({"model": "m", "input": "hi"}, CL)
    finally:
        urllib.request.urlopen = orig
    assert d["id"] == "resp_final", d
    assert len(keys) == 2, f"expected one retry, got {len(keys)} attempts"
    assert keys[0] and keys[0] == keys[1], f"retry must reuse the idempotency key: {keys}"
    print(f"  PASS retry-idempotent: 2 attempts, same key {keys[0][:8]}… -> server dedups (at-most-once)")


def test_a_down_instance_is_an_ordinary_failure_not_a_blocked_recovery():
    """REGRESSION. `sent` was raised the moment the Request OBJECT existed, which opens no
    socket — so a refused connection exhausted the retries and raised TurnOutcomeUnknown with
    request_sent=True. `bridge_core._run_turn` turns that into RECOVERY_BLOCKED: terminal,
    never replayed, needing an operator. An instance that is simply down would have made every
    message in every group permanently blocked work instead of a failure the tenant can retry.

    The classification is by EVIDENCE, in both directions: a refused port or an unresolvable
    host proves nothing was sent; a timeout after connect proves nothing either way and must
    still be treated as sent, because that half is what stops a second billed turn."""
    orig = urllib.request.urlopen
    slept = []
    orig_sleep = ing.time.sleep
    ing.time.sleep = slept.append

    def post(exc):
        def fake_urlopen(req, timeout=None):
            raise exc
        urllib.request.urlopen = fake_urlopen
        try:
            ing._post_ironclaw({"model": "m", "input": "hi"}, CL, attempts=2)
        except BaseException as e:
            return e
        raise AssertionError("the fake instance answered")

    try:
        refused = post(urllib.error.URLError(ConnectionRefusedError(61, "Connection refused")))
        unresolvable = post(urllib.error.URLError(
            __import__("socket").gaierror(8, "nodename nor servname provided")))
        ambiguous = post(TimeoutError("no answer after the request was written"))
    finally:
        urllib.request.urlopen = orig
        ing.time.sleep = orig_sleep

    for e, label in ((refused, "connection refused"), (unresolvable, "unresolvable host")):
        assert not isinstance(e, ing.TurnOutcomeUnknown), f"{label} raised {type(e).__name__}"
        assert getattr(e, "request_sent", False) is False, label
    assert isinstance(ambiguous, ing.TurnOutcomeUnknown), type(ambiguous).__name__
    assert ambiguous.request_sent is True
    assert len(slept) == 3, f"each case must still retry once: {slept}"
    print("  PASS a down instance fails terminally; only an ambiguous send blocks recovery")


def test_rejected_continuity_uses_a_deterministic_distinct_fresh_key():
    """Removing previous_response_id changes the body, so the pinned runtime needs a new key."""
    seen = []
    orig = urllib.request.urlopen

    def fake_urlopen(req, timeout=None):
        body = json.loads(req.data)
        key = req.get_header("Idempotency-key")
        seen.append((key, body))
        if len(seen) == 1:
            raise urllib.error.HTTPError(req.full_url, 404, "unknown previous response", {}, None)
        return _Resp({"id": "resp_fresh", "output": []})

    urllib.request.urlopen = fake_urlopen
    token = ing._TURN_CTX.set({"key": "durable-update-key", "deadline": None})
    try:
        th = ing.Thread(CL)
        th.prev = "resp_gone"
        body = {"model": "m", "input": "hi", "previous_response_id": th.prev}
        first = ing._dispatch(dict(body), CL, th)
        expected = __import__("hashlib").sha256(
            b"durable-update-key\0fresh-thread").hexdigest()
        th.prev = "resp_gone"
        seen.clear()
        second = ing._dispatch(dict(body), CL, th)
    finally:
        ing._TURN_CTX.reset(token)
        urllib.request.urlopen = orig
    assert first["id"] == second["id"] == "resp_fresh"
    assert len(seen) == 2, seen
    assert seen[0][0] == "durable-update-key"
    assert seen[1][0] == expected and seen[1][0] != seen[0][0], seen
    assert "previous_response_id" in seen[0][1] and "previous_response_id" not in seen[1][1]
    print("  PASS rejected continuity changes both body and deterministic idempotency key")


def test_staleness_is_measured_not_asked_for():
    """Inject-once holds; a record whose `updated_at` MOVED is re-fetched automatically; and no
    phrasing forces a re-fetch of an unchanged record.

    Replaces the REFRESH_RE test. That keyword list ("what changed", "refresh", "latest on")
    guessed intent from prose and failed both ways: widen the resolved set and a bare
    "what changed?" re-fetched the whole book; tighten name matching and "refresh <multi-word
    account>" matched nothing, so an explicit refresh silently no-oped and the account stayed
    pinned to its first copy for the life of the thread. Data goes stale whether or not anyone
    asks, so freshness is now driven by the catalog's version, not by what the user typed.
    """
    fetches = []
    stamp = {"v": "2026-08-20T10:00:00"}
    saved = _save_seam()
    asvc._svc = lambda p, client=None: ({"accounts": [{"account_id": "NW-001", "name": "Northwind Labs",
                                          "domain": "northwind-labs.example",
                                          "updated_at": stamp["v"]}],
                           "org": "multiagency-sales"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: (fetches.append(aid) or
                                    {"record_id": aid, "account": {"name": "Northwind Labs"},
                                     "contacts": [], "activities": [], "missing": []})
    ing._post_ironclaw = lambda body, client=None:{"id": "r", "output": [
        {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}
    try:
        th = ing.Thread(CL)

        fetches.clear(); ing.turn(th, "Tell me about Northwind")
        assert fetches == ["NW-001"], f"first mention must fetch: {fetches}"
        assert th.supplied == {"NW-001": stamp["v"]}, f"version not recorded: {th.supplied}"

        # unchanged record: nothing re-fetches, whatever the user types
        for q in ("Why Northwind?", "What changed?", "refresh Northwind",
                  "any updates on Northwind?", "latest on Northwind"):
            fetches.clear(); ing.turn(th, q)
            assert fetches == [], f"unchanged record must not re-fetch on {q!r}: {fetches}"

        # the record MOVES in the store -> the next turn re-reads it, unprompted
        stamp["v"] = "2026-08-20T18:30:00"
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == ["NW-001"], f"a moved record must be re-fetched automatically: {fetches}"
        assert th.supplied == {"NW-001": stamp["v"]}, f"new version not recorded: {th.supplied}"

        # ...and having caught up, it settles again
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == [], f"re-fetch must not repeat once caught up: {fetches}"
    finally:
        _restore_seam(saved)
    print("  PASS staleness measured: inject-once holds; a moved record re-reads itself; "
          "no phrase re-fetches an unchanged one")


def test_unknown_sent_version_refetches_once_instead_of_pinning_forever():
    """A supplied version of None means UNKNOWN — re-fetch once — never "never again".

    None is written two ways, both routine: a turn served before the Account Service emitted
    `updated_at`, and the pre-versioning state migration in telegram_bridge.py, which sets every
    id to None BY DESIGN. `_moved` used to require `sent_v is not None`, so either one pinned the
    account to its first copy for the LIFE of the thread — and bridge-threads.json persists, so
    no restart cleared it. That is the same failure the test above says freshness replaced, so it
    has to stay pinned by a test of its own or it will be reintroduced as a null-guard.
    """
    fetches = []
    stamp = {"v": None}                      # None = Account Service not yet emitting the column
    saved = _save_seam()

    def svc(p, client=None):
        if "list_accounts" not in p:
            return {}
        row = {"account_id": "NW-001", "name": "Northwind Labs", "domain": "northwind-labs.example"}
        if stamp["v"] is not None:
            row["updated_at"] = stamp["v"]
        return {"accounts": [row], "org": "multiagency-sales"}

    asvc._svc = svc
    ing._get_context = lambda aid, client=None: (fetches.append(aid) or
                                    {"record_id": aid, "account": {"name": "Northwind Labs"},
                                     "contacts": [], "activities": [], "missing": []})
    ing._post_ironclaw = lambda body, client=None: {"id": "r", "output": [
        {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}
    try:
        th = ing.Thread(CL)

        fetches.clear(); ing.turn(th, "Tell me about Northwind")
        assert th.supplied == {"NW-001": None}, f"unknown version must record None: {th.supplied}"

        # neither side has a version: unknown == unknown, so this must NOT re-fetch every turn
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == [], f"no version on either side must not re-fetch: {fetches}"

        # the service starts emitting updated_at -> heal in exactly ONE fetch
        stamp["v"] = "2026-08-20T10:00:00"
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == ["NW-001"], f"unknown sent version must re-fetch once: {fetches}"
        assert th.supplied == {"NW-001": stamp["v"]}, f"real version not recorded: {th.supplied}"

        # ...and it settles: healing is one fetch, not a per-turn storm
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == [], f"healing must not repeat: {fetches}"

        # and a genuine later edit is still caught — the whole point of the mechanism
        stamp["v"] = "2026-08-20T23:59:00"
        fetches.clear(); ing.turn(th, "Why Northwind?")
        assert fetches == ["NW-001"], f"a moved record must still re-fetch after healing: {fetches}"
    finally:
        _restore_seam(saved)
    print("  PASS unknown version heals: a migrated/pre-column thread re-reads once, then settles")


def test_ironclaw_body_carries_no_secret_or_org_selector():
    """The IronClaw request must never carry the account token or an org selector."""
    captured = {}
    orig = urllib.request.urlopen

    def fake_urlopen(req, timeout=None):
        captured["body"] = json.loads(req.data.decode())
        captured["headers"] = {k.lower(): v for k, v in req.headers.items()}
        return _Resp({"id": "r", "output": []})

    urllib.request.urlopen = fake_urlopen
    try:
        ing._post_ironclaw({"model": "m", "input": "USER REQUEST\nhi", "previous_response_id": "p"}, CL)
    finally:
        urllib.request.urlopen = orig
    body_keys = set(captured["body"].keys())
    assert body_keys <= {"model", "instructions", "input", "previous_response_id"}, body_keys
    blob = json.dumps(captured)
    assert CL.account_token not in blob, "account token leaked into IronClaw request"
    assert "x-org-id" not in blob.lower() and "org_id" not in captured["body"], "org selector in IronClaw request"
    print("  PASS no-secret-to-ironclaw: body keys ⊆ {model,instructions,input,previous_response_id}; "
          "no token/org selector")


def test_in_progress_turn_polled_to_completion():
    """A tool-using turn returns `in_progress` with no message yet; turn() must poll the response
    to terminal instead of relaying an empty reply."""
    saved = _save_seam()
    orig = urllib.request.urlopen
    polls = []

    def fake_urlopen(req, timeout=None):
        polls.append(req.full_url)
        assert req.full_url.endswith("/v1/responses/resp_1"), req.full_url
        return _Resp({"id": "resp_1", "status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "done"}]}]})

    asvc._svc = lambda p, client=None: ({"accounts": [], "org": "multiagency-sales"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: None
    ing._post_ironclaw = lambda body, client=None:{"id": "resp_1", "status": "in_progress",
                                       "output": [{"type": "function_call"}]}
    urllib.request.urlopen = fake_urlopen
    try:
        th = ing.Thread(CL)
        text, _ = ing.turn(th, "heavy tool-using request")
    finally:
        _restore_seam(saved)
        urllib.request.urlopen = orig
    assert text == "done", f"expected polled final text, got {text!r}"
    assert len(polls) == 1 and th.prev == "resp_1"
    print("  PASS in-progress-poll: turn() polls GET /v1/responses/{id} to terminal, no empty reply")


def test_failed_turn_does_not_mark_context_supplied():
    """If the IronClaw call fails, the fetched context was never delivered — it must NOT be
    marked supplied, or the retry (and every later turn) silently loses that account's data."""
    saved = _save_seam()
    asvc._svc = lambda p, client=None: ({"accounts": [{"account_id": "NW-001", "name": "Northwind Labs",
                                                      "domain": "n.com"}],
                                        "org": "o"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Northwind Labs"},
                                                 "contacts": [], "activities": [], "missing": []}
    calls = {"n": 0}

    def flaky_post(body, client=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("ironclaw down")
        return {"id": "r2", "output": [{"type": "message",
                "content": [{"type": "output_text", "text": "ok"}]}]}

    ing._post_ironclaw = flaky_post
    try:
        th = ing.Thread(CL)
        try:
            ing.turn(th, "tell me about Northwind")
            raise AssertionError("expected the failed post to raise")
        except RuntimeError:
            pass
        assert th.supplied == {}, f"failed turn must not mark supplied: {th.supplied}"
        assert th.prev is None
        text, supplied = ing.turn(th, "tell me about Northwind")   # retry succeeds
        assert supplied == ["NW-001"] and set(th.supplied) == {"NW-001"} and th.prev == "r2"
    finally:
        _restore_seam(saved)
    print("  PASS supplied-after-success: failed turn leaves no bookkeeping; retry delivers context")


def test_persona_sent_every_turn():
    """Hosted-MT bakes no persona: `instructions` must carry it on EVERY turn (once-only drifts —
    multi/verify/test_injection*.py), identically, and it must never contain the account token."""
    bodies = []
    saved = _save_seam()
    asvc._svc = lambda p, client=None: ({"accounts": [], "org": "multiagency-sales"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: None
    ing._post_ironclaw = lambda body, client=None:(bodies.append(body) or {"id": "r", "output": []})
    try:
        th = ing.Thread(CL)
        ing.turn(th, "hello")
        ing.turn(th, "follow-up")
    finally:
        _restore_seam(saved)
    assert len(bodies) == 2
    assert all(b.get("instructions") for b in bodies), "persona missing from a turn"
    assert bodies[0]["instructions"] == bodies[1]["instructions"], "persona differs across turns"
    assert bodies[0]["instructions"] == CL.persona, "instructions is not the client's persona"
    assert CL.account_token not in bodies[0]["instructions"], "token in persona"
    print("  PASS persona-every-turn: instructions present and identical on both turns; no token")


def test_per_client_routing():
    """Every request a thread makes must carry ITS client's credentials — and one client's tokens
    must never appear anywhere in another client's requests."""
    A = ing.ClientConfig(slug="acme", ironclaw_token="ic-token-A", account_token="acct-token-A",
                         persona="persona-A")
    B = ing.ClientConfig(slug="bravo", ironclaw_token="ic-token-B", account_token="acct-token-B",
                         persona="persona-B")
    reqs = []
    orig = urllib.request.urlopen

    def fake_urlopen(req, timeout=None):
        reqs.append({"url": req.full_url,
                     "headers": {k.lower(): v for k, v in req.headers.items()},
                     "body": req.data.decode() if req.data else ""})
        if "list_accounts" in req.full_url:
            return _Resp({"accounts": [], "org": "org-" + req.get_header("X-service-token")[-1]})
        return _Resp({"id": "r", "output": []})

    urllib.request.urlopen = fake_urlopen
    try:
        ing.turn(ing.Thread(A), "hello from A")
        ing.turn(ing.Thread(B), "hello from B")
    finally:
        urllib.request.urlopen = orig
    a_reqs, b_reqs = reqs[:2], reqs[2:]
    assert len(reqs) == 4, [r["url"] for r in reqs]
    for got, c in ((a_reqs, A), (b_reqs, B)):
        svc, ic = got
        assert svc["headers"]["x-service-token"] == c.account_token, (c.slug, svc["headers"])
        assert ic["headers"]["authorization"] == "Bearer " + c.ironclaw_token, (c.slug, ic["headers"])
    for other, own in ((A, b_reqs), (B, a_reqs)):
        blob = json.dumps(own)
        assert other.ironclaw_token not in blob and other.account_token not in blob, \
            f"{other.slug}'s tokens leaked into the other client's requests"
    print("  PASS per-client routing: each thread's requests carry its own tokens; no cross-leak")


def test_speaker_subject_disambiguation():
    """Session-1 fix: the sender's name is attribution only — never resolved as an account, and
    kept structurally distinct from the message. The resolver inspects only the message content."""
    saved = _save_seam()
    fetches, inputs = [], []
    # TWO accounts deliberately: with a one-account book, "the no-target fallback supplied the
    # whole book" and "the speaker's name resolved as an account" produce an identical fetch
    # list, and case 2 below could no longer tell them apart.
    asvc._svc = lambda p, client=None: ({"accounts": [{"account_id": "NW-001", "name": "Northwind Labs",
                                          "domain": "northwind-labs.example"},
                                         {"account_id": "AC-002", "name": "Alder Cope",
                                          "domain": "alder-cope.example"}],
                           "org": "multiagency-sales"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: (fetches.append(aid) or
                                    {"record_id": aid, "account": {"name": "Northwind Labs"},
                                     "contacts": [], "activities": [], "missing": []})
    ing._post_ironclaw = lambda body, client=None:(inputs.append(body["input"]) or {"id": "r", "output": []})
    try:
        # 1. speaker "Sam" + message names no account + book already loaded -> no fetch, no phantom
        th = ing.Thread(CL); th.supplied = {"NW-001": None, "AC-002": None}; fetches.clear()
        ing.turn(th, "They are apparently piloting a competitor.", speaker="Sam")
        assert fetches == [], f"(1) speaker/message must not fetch: {fetches}"
        assert inputs[-1].startswith("SPEAKER: Sam"), "(1) speaker not labeled as metadata"
        assert not inputs[-1].startswith("Sam:"), "(1) bare name prefix still present"

        # 2. speaker name EQUALS a real account name; message names no account. The speaker must
        # not select an account: the only acceptable outcomes are nothing, or the WHOLE book via
        # the no-target fallback. Fetching NW-001 alone would mean the speaker resolved.
        fetches.clear()
        ing.turn(ing.Thread(CL), "I heard they are piloting a competitor.", speaker="Northwind")
        assert sorted(fetches) in ([], ["AC-002", "NW-001"]), \
            f"(2) speaker=='Northwind' selected accounts — it must never resolve: {fetches}"

        # 3. account deliberately named in the message -> resolves to it, and to nothing else.
        # The full name is what makes this deliberate: a lone "Northwind" widens to the book
        # instead (see test_resolver_word_boundaries), which is safe but is not what this
        # test is pinning — the point here is that the MESSAGE selects while the SPEAKER cannot.
        fetches.clear()
        ing.turn(ing.Thread(CL), "Northwind Labs is apparently piloting a competitor.", speaker="Sam")
        assert fetches == ["NW-001"], f"(3) named account in message must resolve: {fetches}"

        # 4. multi-human attribution preserved across distinct speakers
        th4 = ing.Thread(CL)
        ing.turn(th4, "quick note", speaker="Pat")
        ing.turn(th4, "another note", speaker="Sam")
        assert inputs[-2].startswith("SPEAKER: Pat") and inputs[-1].startswith("SPEAKER: Sam"), \
            "(4) multi-human attribution lost"

        # frozen behavior: no-speaker path unchanged (message stands alone when no context)
        assert ing.build_envelope("hi", [], "org") == "hi", "no-speaker no-context envelope changed"
    finally:
        _restore_seam(saved)
    print("  PASS speaker/subject: name never resolves as account (1,2); named account resolves (3); "
          "attribution preserved (4); no-speaker path frozen")


def test_data_starved_thread_recovers_when_data_appears():
    """A thread that got NO context (empty org -> the model tells the user it has no records) must
    not stay ANCHORED to that stance once the org is seeded: the seam drops the stale
    previous_response_id so the newly-available context lands on a FRESH IronClaw thread, instead
    of chaining to the data-starved history the model keeps repeating (the live proof-a case)."""
    saved = _save_seam()
    state = {"accts": []}       # empty org first; seeded between turns
    asvc._svc = lambda p, client=None: ({"accounts": state["accts"], "org": "o"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Northwind Labs"},
                                                 "contacts": [], "activities": [], "missing": []}
    posts = []
    def fake_post(body, client=None):
        posts.append(body)
        return {"id": f"resp_{len(posts)}", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}
    ing._post_ironclaw = fake_post
    try:
        th = ing.Thread(CL)
        _, s1 = ing.turn(th, "which prospects should we focus on?")      # empty org
        assert s1 == [] and th.prev == "resp_1" and th.ever_supplied is False
        assert "previous_response_id" not in posts[0]

        state["accts"] = [{"account_id": "NW-001", "name": "Northwind Labs", "domain": "n.com"}]  # seeded

        _, s2 = ing.turn(th, "which prospects should we focus on?")      # context now available
        assert s2 == ["NW-001"], s2
        assert "previous_response_id" not in posts[1], "stale data-starved thread not dropped -> model anchors"
        assert th.prev == "resp_2" and th.ever_supplied is True
    finally:
        _restore_seam(saved)
    print("  PASS data-starved recovery: empty-org thread drops its stale prev once context appears")


def test_turn_failed_status_leaves_no_bookkeeping():
    """A response that returns TERMINAL status 'failed' (no exception raised) must behave like
    the raise path: no supplied-marking, no thread.prev advance — the turn never happened."""
    saved = _save_seam()
    asvc._svc = lambda p, client=None: ({"accounts": [{"account_id": "NW-001", "name": "Northwind Labs",
                                                      "domain": "n.com"}],
                                        "org": "o"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Northwind Labs"},
                                                 "contacts": [], "activities": [], "missing": []}
    ing._post_ironclaw = lambda body, client=None: {"id": "r_failed", "status": "failed", "output": []}
    try:
        th = ing.Thread(CL)
        try:
            ing.turn(th, "tell me about Northwind")
            raise AssertionError("terminal-'failed' status must raise")
        except RuntimeError as e:
            assert "did not complete" in str(e), e
        assert th.supplied == {}, f"failed-status turn must not mark supplied: {th.supplied}"
        assert th.prev is None and th.ever_supplied is False
    finally:
        _restore_seam(saved)
    print("  PASS failed-status turn: raises; no supplied-marking, no thread.prev advance")


def test_turn_poll_timeout_leaves_no_bookkeeping():
    """_await_completion returns the last snapshot when the poll deadline expires with the run
    still in_progress; turn() must treat that as UNKNOWN — not as a failure, and not by
    relaying an empty reply and marking the context supplied.

    It raised a bare RuntimeError, which `bridge_core._run_turn` reads through
    `getattr(e, "request_sent", False)` — False — and records as FAILED_TERMINAL +
    CLIENT_FAILURE: terminal, response id discarded, and the client told a turn failed that is
    still running and already billed. One second later the same fact raises
    TurnBudgetExceeded(request_sent=True) and becomes RECOVERY_BLOCKED, so which verdict an
    operator saw was a race between the 150s poll deadline and the 180s turn budget."""
    saved, saved_await = _save_seam(), ing._await_completion
    asvc._svc = lambda p, client=None: ({"accounts": [{"account_id": "NW-001", "name": "Northwind Labs",
                                                      "domain": "n.com"}],
                                        "org": "o"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Northwind Labs"},
                                                 "contacts": [], "activities": [], "missing": []}
    ing._post_ironclaw = lambda body, client=None: {"id": "r_slow", "status": "in_progress", "output": []}
    ing._await_completion = lambda d, client=None, **kw: d      # deadline expired: last snapshot
    try:
        th = ing.Thread(CL)
        try:
            ing.turn(th, "tell me about Northwind")
            raise AssertionError("poll-timeout (still in_progress) must raise")
        except ing.TurnOutcomeUnknown as e:
            assert "r_slow" in str(e), "the recoverable response id was not named"
            assert e.request_sent is True, (
                "a still-running, already-billed turn was reported as 'no request was sent', "
                "which the bridge records as a terminal CLIENT_FAILURE")
        assert th.supplied == {} and th.prev is None and th.ever_supplied is False

        # A genuinely FAILED response is still a plain failure, not an unknown outcome.
        ing._post_ironclaw = lambda body, client=None: {"id": "r_bad", "status": "failed",
                                                        "output": []}
        th2 = ing.Thread(CL)
        try:
            ing.turn(th2, "tell me about Northwind")
            raise AssertionError("a failed response must raise")
        except ing.TurnOutcomeUnknown as e:
            raise AssertionError(
                "a FAILED turn was misreported as an unknown outcome") from e
        except RuntimeError as e:
            assert "did not complete" in str(e), e
    finally:
        _restore_seam(saved); ing._await_completion = saved_await
    print("  PASS poll-timeout turn: still-in_progress is UNKNOWN and recoverable; failed is failed")


def test_client_without_persona_refuses_to_serve():
    """There is no usable default persona. A hand-built ClientConfig that never composed
    one must refuse to serve, at Thread creation."""
    bare = ing.ClientConfig(slug="bare", ironclaw_token="t", account_token="a")
    assert bare.persona == "", "ClientConfig grew a usable persona default again"
    try:
        ing.Thread(bare)
        raise AssertionError("personaless client served a Thread")
    except RuntimeError as e:
        assert "persona" in str(e) and "bare" in str(e)
    try:
        ing.Thread(None)
        raise AssertionError("Thread with no client must fail closed")
    except RuntimeError as e:
        assert "no client" in str(e)
    print("  PASS no-default-persona: personaless config and clientless Thread both refuse")


# THE SEAM ENTRY POINTS THESE TESTS STUB, named once. The triple was hand-written at twelve
# sites plus `_stub_turn` below, and a sibling file (test_catalog_orphan.Harness) spells it a
# third way — so "which functions does a turn actually reach out through?" had fourteen answers
# that all had to be edited together. The seam split proved the point: `_svc` moved to
# account_service, and every one of those spellings had to change.
#
# Deliberately a save/restore PAIR rather than a context manager: the call sites' try/finally
# blocks differ in shape, and rewriting control flow to save two lines each is how a mechanical
# edit lands a restore inside a docstring. This names the tuple without touching structure.
def _save_seam():
    """The stubbable seam surface, as it is right now.

    TWO PATCHING IDIOMS LIVE IN THIS FILE AND THEY ARE NOT REDUNDANT — recorded here because a
    review filed them as duplication and the reading is easy to repeat. This one replaces the
    SEAM's own functions (`_svc`, `_get_context`, `_post_ironclaw`) for tests about what a turn
    does. The hand-rolled `urllib.request.urlopen` swaps replace the TRANSPORT UNDERNEATH
    `_post_ironclaw`, for the tests that are about `_post_ironclaw` itself — retries, idempotency
    keys, how a refused connection is classified. A test asserting on both layers uses both, and
    that is the shape rather than a mistake.

    All six transport swaps restore in a `finally`; checked, because a `urlopen` left patched
    would corrupt every later test in the process rather than failing its own."""
    return (asvc._svc, ing._get_context, ing._post_ironclaw)


def _restore_seam(saved):
    """Put back exactly what `_save_seam` took."""
    asvc._svc, ing._get_context, ing._post_ironclaw = saved


def _stub_turn(accts, contexts=None, post=None, svc_raises=None, svc=None):
    """Install seam stubs and return a restore() — shared by the product-behavior tests below.

    `svc` overrides the whole /list_accounts document, for the cases that need a shape the
    healthy path never produces (a catalog this seam cannot read is a DEFECT, not an outage, and
    the two are asserted apart)."""
    saved = _save_seam()

    def fake_svc(p, client=None):
        if svc_raises and "list_accounts" in p:
            raise svc_raises
        if "list_accounts" not in p:
            return {}
        return svc if svc is not None else {"accounts": accts, "org": "o"}

    asvc._svc = fake_svc
    ing._get_context = lambda aid, client=None: (contexts or {}).get(aid)
    ing._post_ironclaw = post or (lambda body, client=None: {
        "id": "resp_x", "output": [{"type": "message",
                                    "content": [{"type": "output_text", "text": "ok"}]}]})

    def restore():
        _restore_seam(saved)
    return restore


def test_a_malformed_catalog_row_does_not_kill_the_tenant():
    """REGRESSION, reproduced before it was fixed: the seam read Account Service payloads with
    `[]`, so ONE row missing `name` raised KeyError out of `turn()`. `bridge_core._run_turn`
    classifies that as an ordinary failure (nothing was sent), so the tenant got FAILED_TERMINAL
    on EVERY message until an operator repaired the store — a whole group down because one row
    was short a column.

    Degraded mode already covers an UNAVAILABLE store; this is the MALFORMED case, which is the
    same fact to a client and a different one to an operator. The usable rows must still be
    served, the model must be told the book it sees is short (silently shrinking it is the worse
    failure — the analyst would answer confidently about accounts it cannot see), and nothing
    about our plumbing may reach the client's envelope beyond that."""
    posts = []
    restore = _stub_turn(
        [{"account_id": "a1", "name": "Good Co", "updated_at": "1"},
         {"account_id": "a2", "updated_at": "1"},        # no name
         {"name": "No Id Co", "updated_at": "1"},        # no account_id
         "not-even-a-dict"],
        contexts={"a1": {"record_id": "a1", "account": {"name": "Good Co"}, "facts": []}},
        post=lambda body, client=None: (posts.append(body), {
            "id": "r1", "output": [{"type": "message",
                                    "content": [{"type": "output_text", "text": "ok"}]}]})[1])
    try:
        _text, supplied = ing.turn(ing.Thread(CL), "what should we look at?", speaker="Sam")
    finally:
        restore()
    assert supplied == ["a1"], f"the healthy row must still be served: {supplied}"
    envelope = posts[0]["input"]
    assert "partial" in envelope and "could not be read" in envelope, \
        "the model was not told its book is incomplete"
    assert "3 record(s)" in envelope, f"the count of unreadable rows is wrong: {envelope[:200]}"
    assert "Good Co" in envelope
    # Operator plumbing must not leak into the client-visible envelope.
    for leak in ("account_id", "name'", "malformed", "KeyError"):
        assert leak not in envelope.split("ACCOUNT RECORDS")[0], f"{leak!r} leaked into the envelope"
    print("  PASS a malformed catalog row is dropped, the book is declared short, the turn serves")


def test_a_malformed_context_is_an_orphan_not_a_crash():
    """The second payload shape. A context that comes back but cannot be RENDERED is the same
    fact as one that never came back — `_record_orphan`'s own words, 'your store lists a row my
    read of it cannot resolve' — so it takes that path: bounded retry, self-healing when the row
    is repaired, visible to `ironworks tenant inspect`, and silent in the envelope.

    The two causes need DIFFERENT operator repairs, though (prune a catalog vs fix a null
    column), so the log must say which one happened rather than claiming 404 for both."""
    thread = ing.Thread(CL)
    restore = _stub_turn(
        [{"account_id": "a1", "name": "Good Co", "updated_at": "1"}],
        contexts={"a1": {"record_id": "a1", "account": {}, "facts": []}})   # unrenderable
    try:
        _text, supplied = ing.turn(thread, "what should we look at?", speaker="Sam")
    finally:
        restore()
    assert supplied == [], f"an unrenderable record must not be supplied: {supplied}"
    assert "a1" in thread.orphans, "the malformed record was not recorded as an orphan"
    print("  PASS an unrenderable account context is recorded as an orphan, not raised")


def test_empty_book_is_declared_to_the_model():
    """C2: with ZERO accounts loaded the model must be TOLD the book is empty. A bare user
    message + a persona that says 'work from the records supplied to you' leaves
    confabulate-or-stall to chance — the live pilot's day-1 state."""
    posts = []
    restore = _stub_turn([], post=lambda body, client=None: (posts.append(body), {
        "id": "r1", "output": [{"type": "message",
                                "content": [{"type": "output_text", "text": "ok"}]}]})[1])
    try:
        ing.turn(ing.Thread(CL), "which accounts should we focus on?", speaker="Sam")
    finally:
        restore()
    sent = posts[0]["input"]
    assert "ACCOUNT RECORDS STATUS:" in sent and "empty" in sent, sent
    assert "no account records have been loaded" in sent, sent
    print("  PASS empty book declared: the model is told the book is empty, never left to guess")


def test_store_outage_degrades_instead_of_killing_the_turn():
    """An Account Service outage must not error EVERY message (including 'thanks!'):
    the turn proceeds context-free and the model is told records are briefly unavailable."""
    posts = []
    restore = _stub_turn([], svc_raises=OSError("connection refused"),
                         post=lambda body, client=None: (posts.append(body), {
                             "id": "r1", "output": [{"type": "message",
                                                     "content": [{"type": "output_text", "text": "ok"}]}]})[1])
    try:
        text, supplied = ing.turn(ing.Thread(CL), "thanks, that helps")
    finally:
        restore()
    assert text == "ok" and supplied == [], (text, supplied)
    assert "temporarily unavailable" in posts[0]["input"], posts[0]["input"]
    print("  PASS store outage: turn still answers, model told records are briefly unavailable")


def test_a_seam_defect_degrades_like_an_outage_but_is_not_reported_as_one():
    """THE CLIENT CANNOT TELL; THE OPERATOR MUST.

    `except Exception` covered both branches, so a KeyError on `catalog["accounts"]` or any bug
    in `_usable_catalog_rows` printed "account store unreachable" — sending whoever read that
    line to look at the network for a bug in this file. The module's own header says it exists to
    stop exactly that misdiagnosis.

    Both halves are asserted. The turn must still answer with the same caveat, because a bug here
    must not kill a conversation any more than an outage does; and the operator line must say
    `[defect]`, not `[degraded]`, and must not claim the store was unreachable."""
    posts, log = [], io.StringIO()
    # A catalog the real service would never send: no `accounts` key at all, so the
    # `catalog["accounts"]` lookup inside the try raises KeyError — a defect in this seam's
    # reading of a response it DID receive, which is the case that used to be misreported.
    restore = _stub_turn([], svc={"org": "testco"},
                         post=lambda body, client=None: (posts.append(body), {
                             "id": "r1", "output": [{"type": "message",
                                                     "content": [{"type": "output_text",
                                                                  "text": "ok"}]}]})[1])
    try:
        with contextlib.redirect_stdout(log), contextlib.redirect_stderr(io.StringIO()):
            text, supplied = ing.turn(ing.Thread(CL), "thanks, that helps")
    finally:
        restore()
    assert text == "ok" and supplied == [], (text, supplied)
    assert "temporarily unavailable" in posts[0]["input"], posts[0]["input"]
    out = log.getvalue()
    assert "[defect]" in out, f"a seam bug was not reported as one:\n{out}"
    assert "account store unreachable" not in out, (
        f"a bug in this file was reported to the operator as a network outage:\n{out}")
    print("  PASS seam defect: client sees the same degraded turn, operator is told it is a bug")


def test_lost_previous_response_id_self_heals():
    """If the server no longer knows our previous_response_id, every later turn would
    404 forever (a permanently bricked group). One retry on a fresh thread instead."""
    import urllib.error
    posts = []

    def fake_post(body, client=None):
        posts.append(dict(body))
        if "previous_response_id" in body:
            raise urllib.error.HTTPError("u", 404, "Not Found", None, None)
        return {"id": "resp_new", "output": [{"type": "message",
                                              "content": [{"type": "output_text", "text": "ok"}]}]}

    restore = _stub_turn([], post=fake_post)
    try:
        th = ing.Thread(CL)
        th.prev = "resp_expired"
        text, _ = ing.turn(th, "still there?")
    finally:
        restore()
    assert text == "ok", text
    assert len(posts) == 2 and "previous_response_id" not in posts[1], posts
    assert th.prev == "resp_new", th.prev
    print("  PASS 404 self-heal: an expired previous_response_id retries on a fresh thread")


def test_first_seed_flags_the_dropped_conversation():
    """The starvation reset is correct but LOSSY — facts the team supplied conversationally
    during the empty-book weeks don't come along. Say so instead of discarding silently."""
    posts = []
    state = {"accts": []}

    def fake_post(body, client=None):
        posts.append(dict(body))
        return {"id": f"resp_{len(posts)}", "output": [{"type": "message",
                                                        "content": [{"type": "output_text", "text": "ok"}]}]}

    saved = _save_seam()
    asvc._svc = lambda p, client=None: ({"accounts": state["accts"], "org": "o"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Northwind Labs"},
                                                 "contacts": [], "activities": [], "missing": []}
    ing._post_ironclaw = fake_post
    try:
        th = ing.Thread(CL)
        ing.turn(th, "anything on northwind?")                      # empty book
        state["accts"] = [{"account_id": "NW-001", "name": "Northwind Labs"}]
        ing.turn(th, "anything on northwind?")                      # first records land
    finally:
        _restore_seam(saved)
    assert "previous_response_id" not in posts[1], "starvation reset must still drop the stale thread"
    assert "restate" in posts[1]["input"], posts[1]["input"]
    print("  PASS first-seed disclosure: the dropped pre-records conversation is flagged, not silent")


def test_untriggered_whole_book_question_still_gets_records():
    """A book-wide question that names no account and matches no PRIORITIZE phrase must still
    receive records.

    Regression for the defect multi/eval found at 40 accounts under --isolate: "Is anything in
    the book time-sensitive right now?" resolved to NOTHING, so the analyst answered a
    book-wide question with zero records and truthfully reported that none were loaded. The
    keyword list cannot be the only door — natural phrasings walk past it. The fallback must
    also stay inject-once (no re-blobbing) and must not disturb deliberate naming.
    """
    fetches = []
    saved = _save_seam()
    book = [{"account_id": "NW-001", "name": "Northwind Labs", "domain": "northwind-labs.example"},
            {"account_id": "TF-005", "name": "Tallow Finch", "domain": "tallow-finch.example"},
            {"account_id": "BW-010", "name": "Blackwater Instruments", "domain": "bw.example"}]
    asvc._svc = lambda p, client=None: ({"accounts": book, "org": "eval"} if "list_accounts" in p else {})
    ing._get_context = lambda aid, client=None: (fetches.append(aid) or
                                  {"record_id": aid, "account": {"name": aid},
                                   "contacts": [], "activities": [], "missing": []})
    ing._post_ironclaw = lambda body, client=None: {"id": "r", "output": [
        {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}
    try:
        # 1. fresh thread, untriggered phrasing -> the whole book, not silence
        th = ing.Thread(CL)
        fetches.clear(); ing.turn(th, "Is anything time-sensitive right now?")
        assert sorted(fetches) == ["BW-010", "NW-001", "TF-005"], \
            f"untriggered book-wide question got {fetches} — the analyst would answer blind"

        # 2. inject-once still holds: asking again re-fetches nothing
        fetches.clear(); ing.turn(th, "Anything urgent?")
        assert fetches == [], f"fallback must not re-blob an already-supplied book: {fetches}"

        # 3. deliberate naming still wins on a fresh thread (fallback must not override it)
        th2 = ing.Thread(CL)
        fetches.clear(); ing.turn(th2, "Tell me about Blackwater Instruments")
        assert fetches == ["BW-010"], f"naming one account must fetch only it: {fetches}"

        # 4. ...and the rest of the book arrives on the next unresolved question
        fetches.clear(); ing.turn(th2, "What should I worry about?")
        assert sorted(fetches) == ["NW-001", "TF-005"], \
            f"remainder of the book should arrive once, got {fetches}"
    finally:
        _restore_seam(saved)
    print("  PASS untriggered book-wide questions receive records (inject-once, naming still wins)")
if __name__ == "__main__":
    # Discovered, not listed. The hand-maintained call list drifted: two tests defined
    # in this file were never in it, so CI (pytest) ran them and the documented
    # `python3 test_turn.py` silently skipped them. globals() preserves
    # definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL TURN TESTS PASS")
