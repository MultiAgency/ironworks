#!/usr/bin/env python3
"""Session 2B regression tests — structured handoff, IN-MEMORY transfer (no persistence).
Pure unit tests: no live services. Run: python3 test_handoff_2b.py

They pin the security-relevant properties:
  - the schema is enforced (accept valid; reject missing/wrong-type/extra);
  - identity + provenance are adapter-stamped and CANNOT be spoofed by the model;
  - generation is tool-free/inline (never a 'written'/'document' request -> no file tool / in_progress);
  - generation retries once on invalid model output, then validates;
  - the receiving side is a FRESH context and labels the object DERIVED/verify, never 'trusted'.
"""
import json
import os

# This suite drives the seam against no instance at all, so it configures one outright.
# Assigned, not `setdefault`: a configured box must not leak a real instance into a hermetic
# unit suite, and `setdefault` also leaves whatever it did not overwrite in place for every
# test collected after this one.
os.environ["IRONCLAW_API"] = "http://test.invalid"
# THE SHIM, which this file alone did not have. It reached its siblings through a bare
# `sys.path.insert(<this dir>)` + `import context_ingress`, so under `pytest multi/seam` from the
# repository root — where this module is `multi.seam.test_handoff_2b` — it loaded a SECOND copy
# of context_ingress, handoff, registry, persona, services, envelope, account_service, responses
# and pins as top-level modules beside the `multi.seam.*` ones already imported: two
# `ClientConfig` classes, two module-global namespaces. Worse, the path entry was never removed,
# so for the rest of the session every other suite's `except ImportError:` arm could succeed —
# masking the `ModuleNotFoundError` that CONTRIBUTING.md names as the loud signal that the
# package markers and these shims have come apart.
try:
    from . import context_ingress as ing
    from . import handoff as H
except ImportError:  # run as a bare script from inside multi/seam/
    import context_ingress as ing
    import handoff as H

# Explicit test client — no ambient default client/persona exists any more (A4).
CL = ing.ClientConfig(slug="testco", ironclaw_token="test-token",
                      account_token="test-account-token", persona="TEST PERSONA (fixture)")


def _valid_model_obj():
    return {
        "relationship_path": "Warm intro via Sam Okafor (VP Product) at a healthtech meetup.",
        "opportunity_hypothesis": "Custom AI patient-intake assistant.",
        "commercial_timing": "Must be live before the next enrollment cycle.",
        "value_band": "mid",
        "recommended_next_action": "Book a discovery call with Sam this week.",
        "owner": "James",
        "follow_up_timing": "This week",
        "key_people": ["Sam Okafor — VP Product (engaged, internal champion)"],
        "confirmed_facts": ["Wants a patient intake assistant (Account Store, 2026-07-26)"],
        "assumptions": ["Budget freeze is competitive, not deprioritization"],
        "unknowns": ["Exact enrollment-cycle date"],
        "evidence_refs": ["Account Store activity 2026-07-26", "Skyto 2026-08-17"],
        "risks": ["Defaults to the EHR vendor if we move slowly"],
    }


def _full_valid():
    o = _valid_model_obj()
    o.update({"account_id": "MD-005", "account_name": "Meridian Health",
              "source_thread_id": "resp_si_thread", "generated_at": "2026-08-18T00:00:00+00:00"})
    return o


def test_schema_accepts_valid_and_rejects_malformed():
    assert H.validate(_full_valid()) == [], H.validate(_full_valid())

    missing = _full_valid(); del missing["owner"]
    assert any("owner" in e for e in H.validate(missing)), "missing owner not caught"

    wrong = _full_valid(); wrong["confirmed_facts"] = "not a list"
    assert any("confirmed_facts" in e for e in H.validate(wrong)), "wrong-type list not caught"

    empty = _full_valid(); empty["recommended_next_action"] = "   "
    assert any("recommended_next_action" in e for e in H.validate(empty)), "empty string not caught"

    extra = _full_valid(); extra["injected_extra"] = "x"
    assert any("unexpected keys" in e for e in H.validate(extra)), "extra key not caught"
    print("  PASS schema: accepts valid; rejects missing / wrong-type / empty / extra key")


def test_generate_stamps_identity_and_provenance_the_model_cannot_spoof():
    """Even if the model tries to assert a different account/org/provenance, the adapter overwrites
    identity+provenance with its own trusted values."""
    saved = (ing._get_context, ing._post_ironclaw)
    spoof = _valid_model_obj()
    spoof.update({"account_id": "RIVAL-999", "account_name": "Someone Else",
                  "source_thread_id": "attacker", "generated_at": "1999-01-01T00:00:00+00:00"})
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Meridian Health"}}
    ing._post_ironclaw = lambda body, client=None: {"id": "resp_gen",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": json.dumps(spoof)}]}]}
    try:
        th = ing.Thread(CL); th.prev = "resp_si_thread"
        obj = H.generate_handoff(th, "MD-005")
    finally:
        ing._get_context, ing._post_ironclaw = saved
    assert obj["account_id"] == "MD-005", obj["account_id"]
    assert obj["account_name"] == "Meridian Health", obj["account_name"]
    assert obj["source_thread_id"] == "resp_si_thread", obj["source_thread_id"]
    assert obj["generated_at"] != "1999-01-01T00:00:00+00:00", "model spoofed generated_at"
    assert H.validate(obj) == []
    print("  PASS provenance: adapter overwrites model-asserted identity/provenance (no spoofing)")


def test_generation_is_tool_free_inline():
    """The generation request must NOT use file/document framing (which makes the agent reach for a
    file-authoring tool and return in_progress) and must ask for JSON in the reply."""
    captured = {}
    saved = (ing._get_context, ing._post_ironclaw)
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Meridian Health"}}
    def cap(body, client=None, attempts=4):
        captured["input"] = body["input"]
        return {"id": "r", "output": [{"type": "message",
                "content": [{"type": "output_text", "text": json.dumps(_valid_model_obj())}]}]}
    ing._post_ironclaw = cap
    try:
        th = ing.Thread(CL); th.prev = "resp_si_thread"
        H.generate_handoff(th, "MD-005")
    finally:
        ing._get_context, ing._post_ironclaw = saved
    low = captured["input"].lower()
    # 'no markdown' in the prompt is protective; the risk verbs are the ones that make the agent
    # author a FILE (which returns in_progress). Guard against those specifically.
    for verb in ("written", "document", "file", ".md"):
        assert verb not in low, f"generation prompt uses file-authoring verb {verb!r}"
    assert "json" in low and "reply" in low, "generation prompt does not ask for inline JSON reply"
    print("  PASS tool-free: generation asks for inline JSON, no file/document framing")


def test_generate_retries_once_then_validates():
    saved = (ing._get_context, ing._post_ironclaw)
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Meridian Health"}}
    calls = {"n": 0}
    def flaky(body, client=None, attempts=4):
        calls["n"] += 1
        txt = "not json at all" if calls["n"] == 1 else json.dumps(_valid_model_obj())
        return {"id": f"r{calls['n']}", "output": [{"type": "message",
                "content": [{"type": "output_text", "text": txt}]}]}
    ing._post_ironclaw = flaky
    try:
        th = ing.Thread(CL); th.prev = "resp_si_thread"
        obj = H.generate_handoff(th, "MD-005")
    finally:
        ing._get_context, ing._post_ironclaw = saved
    assert calls["n"] == 2, f"expected one retry, got {calls['n']}"
    assert H.validate(obj) == []
    print("  PASS retry: invalid model output retried once, then validates")


def test_receiving_context_is_fresh_and_labeled_derived_not_trusted():
    captured = {}
    saved = ing._post_ironclaw
    def cap(body, client=None, attempts=4):
        captured["body"] = body
        return {"id": "r", "output": [{"type": "message",
                "content": [{"type": "output_text", "text": "cold answer"}]}]}
    ing._post_ironclaw = cap
    try:
        out = H.receiving_turn(_full_valid(), "What is going on and what should we do next?", client=CL)
    finally:
        ing._post_ironclaw = saved
    body = captured["body"]
    assert "previous_response_id" not in body, "receiving turn must be a FRESH context"
    env = body["input"]
    assert "AGENT-GENERATED" in env and "VERIF" in env.upper(), "object not labeled derived/verify"
    for reserved in ("ACCOUNT RECORDS", "TRUSTED BUSINESS CONTEXT"):   # current + historical labels
        assert reserved not in env, f"must not launder the object as store facts ({reserved})"
    # the confirmed/assumption/unknown separation survives the transfer
    for label in ("confirmed_facts:", "assumptions:", "unknowns:"):
        assert label in env, f"missing {label} in receiving envelope"
    assert out == "cold answer"
    print("  PASS receiving: fresh context; object labeled DERIVED/verify (not trusted); fact/assumption split preserved")


def test_receiving_refuses_invalid_handoff():
    bad = _full_valid(); del bad["risks"]
    try:
        H.receiving_turn(bad, "?", client=CL)
        assert False, "should have refused an invalid handoff"
    except ValueError as e:
        assert "invalid handoff" in str(e)
    print("  PASS receiving-guard: refuses to initialize from an invalid object")


def test_generate_failed_response_does_not_advance_thread_prev():
    """A failed-status generation response must raise WITHOUT touching the caller's SI thread:
    a thread.prev pointing into a failed generation lineage would poison every later turn."""
    saved = (ing._get_context, ing._post_ironclaw)
    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Meridian Health"}}
    ing._post_ironclaw = lambda body, client=None, attempts=4: {"id": "resp_failed",
                                                                "status": "failed", "output": []}
    try:
        th = ing.Thread(CL); th.prev = "resp_si_GOOD"
        try:
            H.generate_handoff(th, "MD-005")
            assert False, "failed-status generation must raise"
        except RuntimeError as e:
            assert "did not complete" in str(e), e
        assert th.prev == "resp_si_GOOD", f"SI thread poisoned by failed generation: {th.prev}"
    finally:
        ing._get_context, ing._post_ironclaw = saved
    print("  PASS failed generation: raises; SI thread.prev untouched (no failed-lineage poisoning)")


def test_generate_polls_in_progress_to_terminal():
    """Generation shares turn()'s completion semantics: an in_progress response (tool reach)
    is polled to terminal.

    Thread contract (CHANGED 2026-08-20, deliberately): generation must NOT advance the caller's
    thread.prev. It previously did, which anchored the group's conversation to a JSON-only turn —
    the client's next message would follow a machine-readable brief instead of the discussion.
    Generation now chains on its own local lineage, so prev stays on the last real turn."""
    import urllib.request
    saved = (ing._get_context, ing._post_ironclaw)
    orig = urllib.request.urlopen
    polls = []

    class _Resp:
        def __init__(self, d): self._d = json.dumps(d).encode()
        def read(self): return self._d
        def __enter__(self): return self
        def __exit__(self, *a): pass

    def fake_poll(req, timeout=None):
        polls.append(req.full_url)
        assert req.full_url.endswith("/v1/responses/resp_gen"), req.full_url
        return _Resp({"id": "resp_gen", "status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text",
                                             "text": json.dumps(_valid_model_obj())}]}]})

    ing._get_context = lambda aid, client=None: {"record_id": aid, "account": {"name": "Meridian Health"}}
    ing._post_ironclaw = lambda body, client=None, attempts=4: {"id": "resp_gen",
                                                                "status": "in_progress", "output": []}
    urllib.request.urlopen = fake_poll
    try:
        th = ing.Thread(CL); th.prev = "resp_si_thread"
        obj = H.generate_handoff(th, "MD-005")
    finally:
        ing._get_context, ing._post_ironclaw = saved
        urllib.request.urlopen = orig
    assert H.validate(obj) == [] and len(polls) == 1
    assert th.prev == "resp_si_thread", f"generation contaminated the SI thread: {th.prev}"
    assert obj["source_thread_id"] == "resp_si_thread"   # provenance still records where it came from
    print("  PASS in-progress generation: polled to terminal; SI thread.prev NOT advanced "
          "(no JSON-turn contamination)")


def test_receiving_refuses_personaless_client():
    """A4 companion: the receiving entry point also refuses a client with no composed persona."""
    bare = ing.ClientConfig(slug="bare", ironclaw_token="t", account_token="a")
    try:
        H.receiving_turn(_full_valid(), "?", client=bare)
        assert False, "personaless client served a receiving turn"
    except RuntimeError as e:
        assert "persona" in str(e)
    print("  PASS receiving-guard: personaless client refused (no default persona to fall back to)")

if __name__ == "__main__":
    # Discovered, not listed — a hand-maintained call list drifts (it did in
    # test_ingress_fixes.py). globals() preserves definition order.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL SESSION-2B HANDOFF TESTS PASS")
