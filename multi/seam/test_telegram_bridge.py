#!/usr/bin/env python3
"""Telegram bridge: routing, the summon gate, persisted thread state, and reply formatting.
Run: python3 test_telegram_bridge.py   (from multi/seam)

SCOPE, and what is deliberately elsewhere. This suite asks what the bridge does when everything
works: does a message reach ITS group's tenant and no other, does an unregistered group get
silence, does the gate hold, does thread state survive a save/load round trip, and is a reply
chunked and stripped before it is sent. The `test_bridge_recovery.py`,
`test_bridge_delivery.py`, and `test_bridge_operations.py` suites own crash boundaries.

WHY THESE, and not a wider net. Each test names the defect it pins: an empty registry serving a
group MultiAgency's internal composition through a since-removed env-pair fallback; a bot with
no username running "healthy" while deaf in every client group; and `ever_supplied` lost across
a restart, which trips data-starvation recovery and silently wipes a live conversation.
"""
import dataclasses, json, os
# This suite drives the seam against a FAKE instance, so it configures one outright.
# Not an import prop: `context_ingress` resolves IRONCLAW_API on use, so this is the
# value under test. Assigned, not `setdefault`, so a configured box cannot leak a real
# instance into a hermetic unit suite.
os.environ["IRONCLAW_API"] = "http://test.invalid"
try:
    from . import context_ingress as ing
except ImportError:
    import context_ingress as ing


def _seam(name):
    """Import a sibling seam module under BOTH invocations, from inside a test body.

    The module top above carries this as a try/except, and every import inside a function
    was missed when `multi/seam/__init__.py` landed — so `pytest multi/seam`, the
    per-subsystem command `CONTRIBUTING.md` documents, failed with
    `ModuleNotFoundError: No module named 'telegram_bridge'`. The full `pytest -q` passed,
    because `deploy/lib/test_ironworks_cli.py` loads the console and `deploy/ironworks`
    puts `multi/seam` on `sys.path` — so the seam tests were passing on an unrelated
    suite's side effect rather than on their own imports.

    A helper rather than twelve copies of the try/except: twelve copies is the shape that
    let the module tops and the test bodies drift apart in the first place."""
    import importlib
    try:
        return importlib.import_module("." + name, __package__)
    except (ImportError, TypeError):      # script invocation: no package context
        return importlib.import_module(name)


# The one explicit test client: there is no ambient default client or persona any more
# (the env-pair fallback was removed) — every thread names its client.
CL = ing.ClientConfig(slug="testco", ironclaw_token="test-token",
                      account_token="test-account-token", persona="TEST PERSONA (fixture)")


def _synthetic_guidance(slug):
    """Minimal valid slug-bound guidance for registry fixtures (client guidance is
    mandatory and fail-closed since the pre-sale readiness round)."""
    return (f"<!-- client-guidance v1 slug: {slug} -->\n"
            "> **SYNTHETIC GUIDANCE — test fixture, not a real business.**\n"
            f"# Client guidance — {slug.title()} Test Co (synthetic)\n"
            "## Company & offer\nTest fixture organization; sells fixture widgets.\n"
            "## Target customer\nFixture buyers.\n"
            "## Qualification criteria\n- fixture pain\n- fixture budget\n"
            "## Disqualification criteria\n- not a fixture\n"
            "## Account stages\nnew -> qualified. Recommend only these, continue discovery, or deprioritize.\n"
            "## Supported evidence sources\nThe loaded fixture book only.\n"
            "## Desired decisions\nWhich fixture accounts to focus on.\n"
            "## Terminology\nNone.\n"
            "## Prohibited claims & actions\nRead-only always.\n")


def test_bridge_dispatch_and_state():
    """Bridge routing: a message routes to ITS group's client, unregistered groups are ignored,
    the summon gate holds per group, and thread state survives a save/load round-trip."""
    import tempfile
    tmp = tempfile.mkdtemp()
    for slug, gid in (("acme", "-100111"), ("bravo", "-100222")):
        with open(os.path.join(tmp, slug + ".env"), "w") as f:
            f.write(f"CLIENT_NAME={slug.title()}\nACCOUNT_TOKEN=at-{slug}\n"
                    f"IRONCLAW_TOKEN=it-{slug}\nTELEGRAM_GROUP_ID={gid}\n")
        with open(os.path.join(tmp, slug + ".guidance.md"), "w") as f:
            f.write(_synthetic_guidance(slug))
    os.environ["CLIENTS_DIR"] = tmp
    os.environ["TELEGRAM_BOT_TOKEN"] = "fake-bot-token"
    os.environ["BRIDGE_STATE"] = os.path.join(tmp, "threads.json")
    tb = _seam("telegram_bridge")

    groups = tb.load_groups()
    assert sorted(c.slug for c in groups.values()) == ["acme", "bravo"], groups
    # Production resolves this through authenticated /list_accounts before thread loading.
    # This routing test is deliberately network-free, so supply the same trusted result.
    groups = {gid: dataclasses.replace(c, organization_id=c.slug,
                                        organization_verified=True)
              for gid, c in groups.items()}
    msg = lambda gid, text: {"chat": {"id": int(gid)}, "text": text}
    # summon via reply-to-the-bot (the /si prefix was retired in favour of @mention + reply)
    reply = {"chat": {"id": -100111}, "text": "hello", "reply_to_message": {"from": {"username": "example_bot"}}}
    assert tb.summoned(reply, groups, "example_bot") == ("-100111", "hello")
    gid, text = tb.summoned(msg("-100222", "hey @example_bot status"), groups, "example_bot")
    assert gid == "-100222" and "status" in text
    assert tb.summoned(msg("-100999", "@example_bot hello"), groups, "example_bot") is None, "unregistered group answered"
    assert tb.summoned(msg("-100111", "just chatting"), groups) is None, "unsummoned reply"

    threads = tb._load_threads(groups)
    assert threads["-100111"].client.slug == "acme" and threads["-100222"].client.slug == "bravo"
    threads["-100111"].prev = "resp_A"; threads["-100111"].supplied = {"NW-001": None}
    tb._save_threads(threads)
    again = tb._load_threads(groups)
    assert again["-100111"].prev == "resp_A" and set(again["-100111"].supplied) == {"NW-001"}
    assert again["-100222"].prev is None
    secrets = tb._secrets(groups)
    # `tb.BOT`, not the literal. This used to be load-bearing for a bad reason: the bridge read
    # TELEGRAM_BOT_TOKEN at IMPORT, so the literal set above only won when this file happened to
    # import the module first. Another suite importing first made the failure read "the bot token
    # is not redacted" — which was not true. The token now resolves on USE, so the assignment
    # above genuinely takes effect and test order no longer decides. Keep asserting `tb.BOT`
    # anyway: the invariant worth pinning is that whatever token the bridge is running with is in
    # the redaction set, not that it equals a particular string.
    assert {"at-acme", "it-acme", "at-bravo", "it-bravo", tb.BOT} <= secrets
    assert tb._redact("boom it-acme boom", secrets) == "boom <redacted> boom"
    print("  PASS bridge dispatch/state: per-group routing, gate, persisted threads, redaction")


def test_bridge_empty_registry_fails_closed():
    """The SALES_GROUP_ID env-pair fallback is GONE: an empty registry must refuse
    to serve even with that env pair present — it used to hand the group MultiAgency's
    internal composition instead of a guidance-validated client persona."""
    import tempfile
    os.environ["CLIENTS_DIR"] = tempfile.mkdtemp()      # empty registry
    os.environ["TELEGRAM_BOT_TOKEN"] = "fake-bot-token"
    os.environ["SALES_GROUP_ID"] = "-100999"            # the removed fallback's trigger…
    os.environ["IRONCLAW_TOKEN"] = "ignored-ic"          # …and its env pair: all ignored now
    os.environ["ACCOUNT_TOKEN"] = "ignored-acct"
    tb = _seam("telegram_bridge")
    try:
        tb.load_groups()
        assert False, "empty registry served groups — the removed fallback is back?"
    except RuntimeError as e:
        assert "no client groups" in str(e)
    finally:
        for k in ("SALES_GROUP_ID", "IRONCLAW_TOKEN", "ACCOUNT_TOKEN"):
            os.environ.pop(k, None)
    print("  PASS empty-registry fails closed: SALES_GROUP_ID fallback removed, env ignored")


def test_bridge_persists_ever_supplied_across_restart():
    """The highest-impact product defect this suite pins: `ever_supplied` must survive a restart. If it
    doesn't, the first post-restart turn that injects a NEW account trips data-starvation
    recovery and silently WIPES the group's conversation (and previously-supplied accounts are
    never re-injected, so the analyst goes permanently blind on them)."""
    import tempfile, pathlib
    bstate = _seam("bridge_state")
    tb = _seam("telegram_bridge")
    with tempfile.TemporaryDirectory() as d:
        # State moved from a JSON file to a single transactional store (bridge_state.py), so
        # this drives the store directly instead of a module global. The PROPERTY is unchanged
        # and is the one that matters: a restart must not lose ever_supplied.
        st = bstate.BridgeState(pathlib.Path(d) / "state.db")
        groups = {"-100999": CL}
        th = ing.Thread(CL)
        th.prev, th.supplied, th.ever_supplied = "resp_7", {"NW-001": None}, True
        tb._save_threads({"-100999": th}, state=st)

        reloaded = tb._load_threads(groups, state=st)["-100999"]      # the restart
        assert reloaded.prev == "resp_7" and set(reloaded.supplied) == {"NW-001"}
        assert reloaded.ever_supplied is True, "ever_supplied lost on restart -> next new account wipes the thread"
        st.close()

        # A pre-versioning state file (supplied as a LIST) must still be REFUSED, not coerced.
        # Coercing to {} would derive ever_supplied=False for a thread that has had context,
        # which trips starvation recovery and silently wipes a live conversation. The refusal
        # now lives at MIGRATION — the one place that can still see the old shape — and it must
        # still tell the operator how to fix it rather than just failing.
        legacy = pathlib.Path(d) / "bridge-threads.json"
        legacy.write_text(json.dumps({"-100999": {"prev": "resp_7", "supplied": ["NW-001"]}}))
        try:
            bstate.migrate_from_json(legacy, pathlib.Path(d) / "migrated.db")
            raise AssertionError("pre-versioning state file was accepted — it must be refused")
        except bstate.LegacyStateError as e:
            assert "UPGRADE.md" in str(e), f"refusal must tell the operator how to fix it: {e}"
            assert "Do NOT delete" in str(e), f"refusal must warn against the wrong fix: {e}"
    print("  PASS restart persistence: ever_supplied survives (no silent conversation wipe)")


def test_bridge_requires_bot_username():
    """A bot with no username matches NO mention: it would run 'healthy' while deaf in every
    client group. Fail loudly at startup instead."""
    tb = _seam("telegram_bridge")
    saved = tb.BOT_USERNAME
    tb.BOT_USERNAME = ""
    try:
        try:
            tb.main()
            raise AssertionError("main() must refuse to start without TELEGRAM_BOT_USERNAME")
        except RuntimeError as e:
            assert "TELEGRAM_BOT_USERNAME" in str(e), e
    finally:
        tb.BOT_USERNAME = saved
    print("  PASS deaf-bot guard: missing TELEGRAM_BOT_USERNAME fails loudly at startup")


def test_replies_chunk_on_line_boundaries():
    """Hard-slicing at 3800 cuts briefing lines in half in front of the client."""
    tb = _seam("telegram_bridge")
    body = "\n".join(f"line {i}: " + "x" * 100 for i in range(60))   # > 3800 chars, many lines
    chunks = tb._chunks(body)
    assert len(chunks) > 1, "fixture must actually split"
    assert all(len(c) <= 3800 for c in chunks), [len(c) for c in chunks]
    assert "\n".join(chunks) == body, "chunking must be lossless"
    for c in chunks:
        assert c.startswith("line ") and c.rstrip().endswith("x"), f"chunk broke mid-line: {c[:40]}"
    assert tb._chunks("") == ["(no response)"]
    print("  PASS reply chunking: splits on line boundaries, lossless, no mid-line cuts")


def test_markdown_stripped_before_send():
    """We send with NO parse_mode, so every marker renders literally in the client's chat.

    Prompt guidance is not sufficient on its own — measured: instructing the analyst
    "no markdown at all" cut bold spans from ~6 to 3 per reply but never to 0. So the guarantee
    is deterministic and lives in send(). The false-positive cases matter as much as the
    stripping: mangling a client's arithmetic or a snake_case filename would be a worse bug
    than the asterisks this fixes."""
    tb = _seam("telegram_bridge")
    strips = [("**FIT:** strong", "FIT: strong"), ("## Summary\nbody", "Summary\nbody"),
              ("__WHY:__ evidence", "WHY: evidence"), ("use `refresh acme` now", "use refresh acme now"),
              ("```\ncode kept\n```", "code kept"), ("**multi\nline** bold", "multi\nline bold")]
    for src, want in strips:
        assert tb.to_plain(src) == want, (src, tb.to_plain(src), want)
    # must NOT touch: arithmetic, snake_case, a lone marker, already-plain text
    for untouched in ["2 * 3 * 4 = 24", "a_b_c filename", "bare ** marker", "no markdown here"]:
        assert tb.to_plain(untouched) == untouched, (untouched, tb.to_plain(untouched))
    # send() must strip BEFORE chunking, or a marker could straddle two messages
    assert "**" not in "".join(tb._chunks(tb.to_plain("**x**\n" + "y" * 4000)))
    print("  PASS markdown stripped before send: no literal ** / # / ` reaches the client")


if __name__ == "__main__":
    # Discovered, not listed — a hand-maintained call list drifted here once, and the tests it
    # forgot ran under pytest but not under the documented command. globals() preserves
    # definition order, so the run order is still the file's own.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL TELEGRAM BRIDGE TESTS PASS")
