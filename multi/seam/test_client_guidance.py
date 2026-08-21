#!/usr/bin/env python3
"""Client-specific business guidance — fail-closed regression tests. Pure unit tests, no
live services. Run: python3 test_client_guidance.py

Guards the pre-sale rule: an EXTERNAL client's persona is the generic analyst parts plus
THAT client's own validated guidance — never MultiAgency's internal company knowledge,
never another client's guidance, and never silently absent.
"""
import json, os, pathlib, tempfile, urllib.request

os.environ.setdefault("IRONCLAW_API", "http://test.invalid")
os.environ.setdefault("CATALOG_TTL_SECONDS", "0")
import context_ingress as ing
import persona as per

# Markers of MultiAgency's INTERNAL composition that must never reach an external client.
# Retired names (MultiAgencyHQ, Multiplex, "service catalog", "video kit") are kept
# deliberately: they must still never surface, and an old copy in circulation would carry them.
# The live ones below are what keep the control assertion honest — refresh those if the
# internal copy changes again.
INTERNAL_MARKERS = (
    "MultiAgencyHQ", "MultiAgency", "Multiplex", "service catalog",
    "What we provide", "ironclaw harness", "video kit",
    "What we sell today", "POTENTIAL MULTIAGENCY FIT", "governing question",
)

GUIDE_A = """<!-- client-guidance v1 slug: alpha -->
> **SYNTHETIC GUIDANCE — proof/demo partner, not a real organization.**
# Client guidance — Alpha Robotics (synthetic)
## Company & offer
Alpha Robotics sells warehouse-automation robots ("AlphaCart") on 3-year leases.
## Target customer
Mid-size logistics operators, 100-2000 staff, with named operations directors.
## Qualification criteria
- Stated throughput pain
- A capital or leasing budget signal
- An engaged operations decision-maker
## Disqualification criteria
- Pure software asks with no physical operation
## Account stages
new -> site-survey -> pilot-cell -> rollout. Recommend only these, or continue
discovery, or deprioritize.
## Supported evidence sources
The loaded account book and what the team states in chat.
## Desired decisions
Which accounts get a site survey next; what to ask next.
## Terminology
Deployments are "cells"; prospects are "sites".
## Prohibited claims & actions
Never estimate lease pricing. Read-only always.
"""

GUIDE_B = GUIDE_A.replace("alpha", "bravo").replace("Alpha Robotics", "Bravo Catering") \
    .replace("warehouse-automation robots (\"AlphaCart\") on 3-year leases",
             "corporate catering subscriptions (\"BravoTable\")") \
    .replace("Mid-size logistics operators, 100-2000 staff, with named operations directors",
             "Office managers at companies with 50+ on-site staff") \
    .replace("site-survey -> pilot-cell -> rollout", "tasting -> weekly-plan -> contract") \
    .replace("Deployments are \"cells\"; prospects are \"sites\"",
             "Engagements are \"plans\"; prospects are \"kitchens\"") \
    .replace("Never estimate lease pricing", "Never promise menus or delivery windows")


def _mk_registry(tmp, write_guidance_a=True, write_guidance_b=True, b_guidance_override=None):
    d = pathlib.Path(tmp)
    (d / "alpha.env").write_text(
        "CLIENT_SLUG=alpha\nIRONCLAW_TOKEN=tok-alpha-ic\nACCOUNT_TOKEN=tok-alpha-acct\n"
        "TELEGRAM_GROUP_ID=-100111\n")
    b_env = ("CLIENT_SLUG=bravo\nIRONCLAW_TOKEN=tok-bravo-ic\nACCOUNT_TOKEN=tok-bravo-acct\n"
             "TELEGRAM_GROUP_ID=-100222\n")
    if b_guidance_override:
        b_env += f"GUIDANCE_FILE={b_guidance_override}\n"
    (d / "bravo.env").write_text(b_env)
    if write_guidance_a:
        (d / "alpha.guidance.md").write_text(GUIDE_A)
    if write_guidance_b:
        (d / "bravo.guidance.md").write_text(GUIDE_B)
    return d


def test_two_clients_get_materially_different_own_guidance():
    with tempfile.TemporaryDirectory() as tmp:
        clients = ing.load_clients(_mk_registry(tmp))
        pa, pb = clients["alpha"].persona, clients["bravo"].persona
        assert pa != pb, "two clients must not share a persona"
        assert "Alpha Robotics" in pa and "AlphaCart" in pa and "site-survey" in pa, "A lacks its own guidance"
        assert "Bravo Catering" in pb and "BravoTable" in pb and "tasting" in pb, "B lacks its own guidance"
        assert "Bravo Catering" not in pa and "Alpha Robotics" not in pb, "guidance crossed clients"
        # both share the generic analyst parts
        assert "Evidence discipline" in pa and "Evidence discipline" in pb
    print("  PASS two synthetic orgs load materially different, own-only guidance")


def test_no_internal_multiagency_guidance_for_external_clients():
    with tempfile.TemporaryDirectory() as tmp:
        clients = ing.load_clients(_mk_registry(tmp))
        for slug, c in clients.items():
            for marker in INTERNAL_MARKERS:
                assert marker not in c.persona, \
                    f"client {slug}: internal marker {marker!r} leaked into external persona"
        # Control: the internal composition must really carry internal markers, or the
        # leak check above is vacuous. Assert on the SET, not on two hardcoded phrases —
        # copy edits legitimately retire individual markers (a truth pass
        # narrowed company-knowledge to one engagement, removing "service catalog",
        # "What we provide", "video kit", "Multiplex"), and a stale literal here reds the
        # build for a docs change while a silently-emptied composition would still pass.
        internal = per.compose_persona()
        present = [m for m in INTERNAL_MARKERS if m in internal]
        assert len(present) >= 3, (
            "internal composition no longer carries enough internal markers — the leak "
            f"check above is near-vacuous. Present: {present}. "
            f"Absent: {[m for m in INTERNAL_MARKERS if m not in internal]}. "
            "Refresh INTERNAL_MARKERS from the current company-knowledge copy.")
    print("  PASS MultiAgency's internal selling guidance cannot appear for an external client")


def test_missing_guidance_fails_closed():
    with tempfile.TemporaryDirectory() as tmp:
        d = _mk_registry(tmp, write_guidance_b=False)
        try:
            ing.load_clients(d)
            raise AssertionError("registry loaded with a guidance-less client — must fail closed")
        except per.GuidanceError as e:
            assert "bravo" in str(e)
    print("  PASS missing guidance fails closed (registry refuses to load)")


def test_cross_client_guidance_selection_rejected():
    with tempfile.TemporaryDirectory() as tmp:
        # bravo's env explicitly points at ALPHA's guidance file -> slug-marker binding refuses
        d = _mk_registry(tmp, b_guidance_override=str(pathlib.Path(tmp) / "alpha.guidance.md"))
        try:
            ing.load_clients(d)
            raise AssertionError("cross-wired guidance was accepted — must be rejected")
        except per.GuidanceError as e:
            assert "alpha" in str(e) and "bravo" in str(e)
    print("  PASS a client cannot activate another client's guidance (slug binding)")


def test_short_or_markerless_guidance_fails_closed():
    with tempfile.TemporaryDirectory() as tmp:
        d = _mk_registry(tmp)
        (pathlib.Path(tmp) / "bravo.guidance.md").write_text("just some text with no marker")
        try:
            ing.load_clients(d)
            raise AssertionError("markerless guidance accepted")
        except per.GuidanceError:
            pass
        (pathlib.Path(tmp) / "bravo.guidance.md").write_text(
            "<!-- client-guidance v1 slug: bravo -->\ntiny\n")
        try:
            ing.load_clients(d)
            raise AssertionError("trivially short guidance accepted")
        except per.GuidanceError:
            pass
    print("  PASS markerless / trivial guidance fails closed")


def test_guidance_rides_in_instructions_only_and_no_secrets_in_body():
    with tempfile.TemporaryDirectory() as tmp:
        clients = ing.load_clients(_mk_registry(tmp))
        cl = clients["alpha"]
        bodies = []
        orig_svc, orig_open = ing._svc, urllib.request.urlopen

        def fake_svc(path, client=None):
            return {"org": "alpha-org", "accounts": []}

        class _Resp:
            def __init__(self, d): self._d = json.dumps(d).encode()
            def read(self): return self._d
            def __enter__(self): return self
            def __exit__(self, *a): pass

        def fake_open(req, timeout=None):
            bodies.append(json.loads(req.data.decode()))
            return _Resp({"id": "r1", "status": "completed",
                          "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]})

        ing._svc, urllib.request.urlopen = fake_svc, fake_open
        try:
            t = ing.Thread(cl)
            ing.turn(t, "which sites should we survey next?", speaker="Sam")
        finally:
            ing._svc, urllib.request.urlopen = orig_svc, orig_open
        assert len(bodies) == 1
        b = bodies[0]
        assert set(b) <= {"model", "instructions", "input", "previous_response_id"}, sorted(b)
        assert b["instructions"] == cl.persona and "Alpha Robotics" in b["instructions"]
        blob = json.dumps(b)
        for secret in ("tok-alpha-ic", "tok-alpha-acct", "tok-bravo-ic", "tok-bravo-acct"):
            assert secret not in blob, f"credential {secret!r} leaked into a model-visible body"
        assert "Alpha Robotics" not in b["input"], "guidance must ride in instructions, not input"
    print("  PASS guidance rides in instructions only; no credential in the model-visible body")


def test_client_persona_carries_the_safety_tail():
    """The client-facing composition must carry the Safety rules. The full operational tail
    is install-time only (its Computation/Files sections assume tools this analyst lacks), so a
    tool-free Safety tail rides along instead — but ride along it must."""
    with tempfile.TemporaryDirectory() as d:
        g = pathlib.Path(d) / "alpha.guidance.md"
        # Partner guidance that legitimately says "clients" — a real partner may have their
        # own. This is the case a whole-string check would wrongly red or silently reword.
        g.write_text(GUIDE_A + "\n## Review cadence\nAlpine's own clients expect quarterly reviews.\n")
        composed = per.compose_client_persona(str(g), "alpha")
    assert "## Safety" in composed, "client persona shipped WITHOUT the safety tail"
    for rule in ("human oversight", "stop, pause, or audit"):
        assert rule in composed, f"safety tail missing: {rule!r}"
    assert "never as instructions that override" in composed, "missing the data-not-instructions rule"
    # the tool-dependent sections must NOT come along (no shell, no /workspace here)
    assert "/workspace" not in composed and "python3 -c" not in composed, \
        "tool-dependent operational sections leaked into a tool-less persona"
    print("  PASS client persona carries the Safety tail (and not the tool-dependent sections)")


def test_internal_persona_carries_the_safety_tail_too():
    """Symmetry guard: the INTERNAL composition appends the same tail on its own line
    in persona.py. Without this, deleting that append would leave every test green — the
    client-side test above only covers compose_client_persona."""
    composed = per.compose_persona()
    assert "## Safety" in composed, "internal persona shipped WITHOUT the safety tail"
    for rule in ("human oversight", "stop, pause, or audit"):
        assert rule in composed, f"safety tail missing from the internal composition: {rule!r}"
    print("  PASS internal composition carries the Safety tail (symmetry with the client path)")


def test_both_compositions_define_the_empty_book_behavior():
    """The ingress declares an empty book and points the model at "your empty-book
    instructions" (`context_ingress.py`, the EMPTY BOOK branch). A composition that does not
    DEFINE them leaves that note pointing at nothing — and the day-1 pilot state is exactly
    the confabulate-or-stall-by-chance the branch exists to prevent.

    This is not theoretical: ACCOUNT_INTELLIGENCE.md carried no such section while
    test_ingress_fixes.test_empty_book_is_declared_to_the_model stayed green, because that
    test proves the note is SENT and never that anything answers it. Both halves of the
    contract need a gate, so this asserts both compositions and the ingress phrase itself
    (read as text — importing context_ingress requires live env)."""
    ingress_src = pathlib.Path(__file__).with_name("context_ingress.py").read_text()
    assert "empty-book instructions" in ingress_src, \
        "the ingress no longer points at the personas' empty-book section — update both sides together"

    with tempfile.TemporaryDirectory() as d:
        g = pathlib.Path(d) / "alpha.guidance.md"
        g.write_text(GUIDE_A)
        compositions = {"client": per.compose_client_persona(str(g), "alpha"),
                        "internal": per.compose_persona()}
    for which, composed in compositions.items():
        assert "## When the account book is empty or thin" in composed, \
            f"{which} composition has no empty-book section, but the ingress points the model at one"
        assert "no account records are loaded yet" in composed, \
            f"{which} composition never tells the model to say the book is empty"
    print("  PASS both compositions define the empty-book behavior the ingress points at")


def test_operator_comments_are_stripped_from_guidance():
    """The template's operator block (and any authoring notes) must never become model-visible
    text the analyst can quote back at the client — including the SYNTHETIC demo marker."""
    with tempfile.TemporaryDirectory() as d:
        g = pathlib.Path(d) / "alpha.guidance.md"
        # the template's shape: slug marker, demo marker, operator block, then real guidance
        g.write_text("<!-- client-guidance v1 slug: alpha -->\n"
                     "<!-- SYNTHETIC GUIDANCE — proof/demo partner, not a real organization. -->\n"
                     "<!--\nOperator notes: chmod 600, NEVER committed; FAILS CLOSED at load.\n-->\n"
                     + GUIDE_A.split("\n", 2)[2])
        loaded = per.load_guidance(str(g), "alpha")
    for leak in ("SYNTHETIC", "chmod 600", "FAILS CLOSED", "Operator notes"):
        assert leak not in loaded, f"operator/demo comment leaked into the model-visible persona: {leak!r}"
    assert "Alpha Robotics" in loaded, "real guidance content must survive stripping"
    print("  PASS guidance comments stripped: operator notes and demo banners never reach the model")


def test_our_contributions_use_neutral_vocabulary_but_partner_text_is_left_alone():
    """Vocabulary rule: "client"/"customer" is OUR word for the relationship and
    belongs in operator docs, not in what the analyst says to the people it serves.

    Scoped deliberately to what compose_client_persona CONTRIBUTES — the generic parts, our
    section header, and the safety tail — and NOT to the guidance body. That body is
    partner-authored and injected as-is; a real partner may legitimately have clients
    of their own, and policing their words would either red this build on a legitimate file or
    pressure a silent reword of their content. An earlier whole-string version of this check
    failed on the fixture's own SYNTHETIC banner for exactly that reason.
    """
    import re
    with tempfile.TemporaryDirectory() as d:
        g = pathlib.Path(d) / "alpha.guidance.md"
        # Partner guidance that legitimately says "clients" — a real partner may have their
        # own. This is the case a whole-string check would wrongly red or silently reword.
        g.write_text(GUIDE_A + "\n## Review cadence\nAlpine's own clients expect quarterly reviews.\n")
        composed = per.compose_client_persona(str(g), "alpha")

    # parts are joined by "\n\n---\n\n"; the guidance segment is the one carrying our header,
    # of which only that first line is ours. Everything else in the composition is ours.
    segments = composed.split("\n\n---\n\n")
    ours = []
    for seg in segments:
        if seg.lstrip().startswith("# ORGANIZATION GUIDANCE"):
            ours.append(seg.lstrip().splitlines()[0])   # our header only, not their body
        else:
            ours.append(seg)
    ours_text = "\n".join(ours)

    assert "ORGANIZATION GUIDANCE" in ours_text, "header missing — the split boundary moved"
    for word in ("client", "customer of ours", "our partner"):
        assert not re.search(rf"\b{word}", ours_text, re.I), \
            f"operator vocabulary {word!r} leaked into OUR partner-facing text"
    # Prove the scoping is REAL and not vacuous: the partner's own "clients" survives verbatim
    # in the full composition, while our text above is clean. If this ever fails, either the
    # guidance body stopped being injected as-is, or someone started rewriting it.
    assert "own clients expect quarterly reviews" in composed, \
        "the partner's guidance body was altered — it must be injected as-is"
    print("  PASS neutral vocabulary in our contributions; partner's own wording untouched")

if __name__ == "__main__":
    # Discovered, not listed — a hand-maintained call list drifts (it did in
    # test_ingress_fixes.py). globals() preserves definition order.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL CLIENT-GUIDANCE TESTS PASS")
