#!/usr/bin/env python3
"""Service definitions + the internal-tenant parity rules. Pure unit tests, no live services.
Run: python3 test_services.py   (from multi/seam — the suites import siblings by bare name)

WHAT THIS GUARDS. Before service definitions there was one hard-coded composition on the
registry path, so MultiAgency could not be one of its own tenants: its book was reachable only
through `context_ingress.py`'s dev oracle, which has no registry, no routing, no mandatory
guidance, no confinement gate and no deprovisioning. The claim "we run the same path we sell"
was untestable because there was only one path and MultiAgency was not on it.

These tests pin the properties that make the claim true AND keep it safe:

  1. An internal tenant and an external tenant travel the SAME code path — same loader, same
     composition function, same Thread, same turn — differing only in configuration.
  2. Being internal grants NOTHING: guidance is mandatory, slug-bound, and service-bound.
  3. A service can only be bound to a tenant by TWO agreeing edits (registry + guidance
     marker), so no single typo moves a tenant onto another composition.
  4. The un-guided internal composition is not reachable from the registry at all.
"""
import inspect
import json
import os
import pathlib
import tempfile
try:
    from . import context_ingress as ing
    from . import persona as per
    from . import pins
    from . import services as svc
except ImportError:
    import context_ingress as ing
    import persona as per
    import pins
    import services as svc

ROOT = pathlib.Path(__file__).resolve().parents[2]

# A guidance body long enough to clear the 400-char floor, with no marker line — the tests
# below prepend whichever marker they are exercising.
BODY = """
# Organization guidance — Synthetic Org
## Company & offer
Synthetic Org runs a book of accounts used only to exercise the seam in tests.
## Target customer
Nobody: every record here is invented and carries a synthetic banner.
## Qualification criteria
- A stated problem in the record
- A named person who is still there
## Disqualification criteria
- A do-not-contact flag on the account
## Account stages
new -> reviewed -> decided. Recommend only these, or continue discovery.
## Supported evidence sources
The loaded account book and what the team states in chat.
## Desired decisions
Which accounts need attention this week and what to ask next.
## Prohibited claims & actions
Never contact anyone. Read-only always.
"""


def _code_only(src):
    """`src` with comments and string literals removed, so a source assertion about BRANCHING
    is not fooled by prose. Dedent first: `inspect.getsource` of a method keeps its
    indentation, which `tokenize` rejects as an IndentationError."""
    import io
    import textwrap
    import tokenize
    out = []
    try:
        for tok in tokenize.generate_tokens(io.StringIO(textwrap.dedent(src)).readline):
            if tok.type not in (tokenize.COMMENT, tokenize.STRING):
                out.append(tok.string)
    except (tokenize.TokenError, IndentationError) as e:  # never let the guard fail open
        raise AssertionError(
            f"could not tokenize the serving-path source to check it: {e}") from e
    return " ".join(out)


def _guidance(dirp, slug, service=None, body=BODY):
    marker = f"<!-- client-guidance v1 slug: {slug}"
    if service:
        marker += f" service: {service}"
    marker += " -->\n"
    p = pathlib.Path(dirp) / f"{slug}.guidance.md"
    p.write_text(marker + body)
    return p


def _tenant(dirp, slug, service=None, group=None, extra=""):
    """Write one registry .env, with the matching guidance file beside it."""
    _guidance(dirp, slug, service)
    lines = [f"CLIENT_SLUG={slug}", f"CLIENT_NAME={slug.title()}",
             f"ACCOUNT_TOKEN=acct-{slug}", f"IRONCLAW_TOKEN=member-{slug}"]
    if service:
        lines.append(f"SERVICE={service}")
    if group:
        lines.append(f"TELEGRAM_GROUP_ID={group}")
    if extra:
        lines.append(extra)
    p = pathlib.Path(dirp) / f"{slug}.env"
    p.write_text("\n".join(lines) + "\n")
    return p


# ── the definitions themselves ────────────────────────────────────────────────────────

def test_every_committed_definition_validates():
    """Every file in multi/services/ loads. A definition that names a persona part which has
    been renamed or deleted would otherwise fail at BRIDGE STARTUP, in front of clients."""
    names = svc.available(ROOT)
    assert names, "multi/services/ holds no definitions — the registry path has nothing to load"
    for name in names:
        d = svc.load_service(name, ROOT)
        assert svc.service_id(d) == f"{name}@{d['version']}"
    print(f"  PASS {len(names)} committed service definition(s) validate: {', '.join(names)}")


def test_default_service_exists_and_is_external():
    d = svc.load_service(svc.DEFAULT_SERVICE, ROOT)
    assert d["audience"] == "external", \
        "the DEFAULT service is what an unmarked registry entry gets — it must be the " \
        "client-generic one, so a forgotten SERVICE= key can never land on an internal book"
    print("  PASS the default service is the external, client-generic one")


def test_unknown_service_fails_closed():
    for bad in ("no-such-service", "Account-Analysis", "../etc/passwd", ""):
        try:
            svc.load_service(bad, ROOT)
        except svc.ServiceError:
            continue
        raise AssertionError(f"service {bad!r} loaded — an unknown/illegal name must fail closed")
    print("  PASS unknown, mis-cased and path-shaped service names all fail closed")


def test_default_composition_is_unchanged_by_the_service_layer():
    """The refactor must be behaviour-preserving for every tenant already on disk: the default
    service composes byte-for-byte what compose_client_persona composed before."""
    with tempfile.TemporaryDirectory() as d:
        g = _guidance(d, "alpha")
        a = per.compose_client_persona(str(g), "alpha", ROOT)
        b = per.compose_service_persona(svc.DEFAULT_SERVICE, str(g), "alpha", ROOT)
        assert a == b, "compose_client_persona diverged from the default service definition"
        # ...and it is genuinely the client-generic composition, not an empty string that two
        # broken code paths agree on.
        assert "ORGANIZATION GUIDANCE" in a and "Synthetic Org" in a
        assert (ROOT / "agent/identity/ANALYST.md").read_text()[:200] in a
    print("  PASS the default service composes byte-for-byte what clients already got")


# ── parity: one path for internal and external ────────────────────────────────────────

def test_internal_and_external_tenants_use_the_same_execution_path():
    """REQUIRED SCENARIO 1. Both tenants are built by the same loader, carry the same type,
    and reach IronClaw through the same functions. The assertion is structural, not a
    re-description: if anyone adds an `if internal:` branch to the serving path, the source
    check below fails."""
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "multiagency", service="relationship-intelligence", group="-100900001")
        _tenant(d, "acme", service="account-analysis", group="-100900002")
        clients = ing.load_clients(d)

        assert set(clients) == {"multiagency", "acme"}
        internal, external = clients["multiagency"], clients["acme"]
        assert type(internal) is type(external) is ing.ClientConfig, \
            "the two tenants are not even the same type — that is a second path"
        assert internal.service_id == "relationship-intelligence@1"
        assert external.service_id == "account-analysis@1"
        # Same composition function, same turn function, for both.
        for c in (internal, external):
            t = ing.Thread(c)                     # refuses an un-composed persona
            assert t.client is c and t.prev is None and t.supplied == {}

    # No serving-path branch may key off the tenant's identity. `turn`, `_targets_for`,
    # `build_envelope` and `load_clients` are the whole path; none may name a tenant.
    # CODE ONLY — comments and docstrings are stripped first, because this rule is about what
    # executes, and the prose in these functions legitimately explains why the rule exists.
    for fn in (ing.turn, ing._targets_for, ing.build_envelope, ing.load_clients,
               per.compose_service_persona):
        code = _code_only(inspect.getsource(fn)).lower()
        for forbidden in ("multiagency", "internal-dev", "founder", "is_internal"):
            assert forbidden not in code, \
                f"{fn.__name__} branches on {forbidden!r} — the serving path must not know " \
                "which tenant is ours"
    print("  PASS internal and external tenants share the loader, the type and the turn path")


def test_internal_tenant_gets_no_privilege_guidance_is_still_mandatory():
    """REQUIRED SCENARIO 2. Internal-ness is configuration, not authority: drop the guidance
    file and the internal tenant refuses to load exactly like an external one."""
    for service in ("relationship-intelligence", "account-analysis"):
        with tempfile.TemporaryDirectory() as d:
            _tenant(d, "who", service=service)
            (pathlib.Path(d) / "who.guidance.md").unlink()
            try:
                ing.load_clients(d)
            except per.GuidanceError:
                continue
            raise AssertionError(f"{service}: a tenant with no guidance loaded — fail-open")
    print("  PASS a missing guidance file fails closed for the internal service too")


def test_internal_composition_cannot_be_bound_to_a_tenant_by_one_edit():
    """REQUIRED SCENARIO 2 (the mis-wiring half). Flipping only the registry's SERVICE= key —
    the single-typo failure — must not move a tenant onto another composition."""
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "acme", service="account-analysis", group="-100900003")
        env = pathlib.Path(d) / "acme.env"
        env.write_text(env.read_text().replace("SERVICE=account-analysis",
                                               "SERVICE=relationship-intelligence"))
        try:
            ing.load_clients(d)
        except per.GuidanceError as e:
            assert "account-analysis" in str(e) and "relationship-intelligence" in str(e), \
                "the refusal must name both sides so the operator knows which one is wrong"
            print("  PASS a registry-only service change is refused (guidance binding disagrees)")
            return
    raise AssertionError("an external tenant was moved onto the internal composition by one edit")


def test_unmarked_guidance_pins_the_default_service():
    """Every guidance file already on disk predates the `service:` field. Those files must keep
    working AND must keep pinning the default — silence means 'the default', never 'anything'."""
    with tempfile.TemporaryDirectory() as d:
        _guidance(d, "legacy")                      # no service: field
        p = pathlib.Path(d) / "legacy.env"
        p.write_text("CLIENT_SLUG=legacy\nACCOUNT_TOKEN=a\nIRONCLAW_TOKEN=b\n")
        c = ing.load_clients(d)["legacy"]
        assert c.service == svc.DEFAULT_SERVICE
        # ...and naming a non-default service against it is refused.
        p.write_text(p.read_text() + "SERVICE=relationship-intelligence\n")
        try:
            ing.load_clients(d)
        except per.GuidanceError:
            print("  PASS unmarked guidance pins the default and refuses any other service")
            return
    raise AssertionError("unmarked guidance accepted a non-default service")


def test_internal_service_is_not_reachable_without_guidance():
    """`compose_persona()` (the un-guided internal composition) is the dev oracle's harness.
    Nothing on the registry path may produce it — if it could, an internal tenant would be a
    tenant with no guidance, which is the fail-open case this whole design removes."""
    oracle = per.compose_persona(ROOT)
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "multiagency", service="relationship-intelligence")
        composed = ing.load_clients(d)["multiagency"].persona
    assert composed != oracle, "the registry produced the un-guided internal composition"
    assert "ORGANIZATION GUIDANCE" in composed and "ORGANIZATION GUIDANCE" not in oracle
    print("  PASS the un-guided internal composition is unreachable from the registry")


def test_service_and_persona_versions_are_operator_visible_without_secrets():
    """REQUIRED SCENARIO 17. The operator must be able to read what a tenant is running
    without the reading itself exposing anything."""
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "acme", service="account-analysis", group="-100900004")
        c = ing.load_clients(d)["acme"]
        visible = f"{c.slug} {c.name} {c.service_id} {c.persona_sha} {c.model}"
        assert c.service_id == "account-analysis@1"
        assert len(c.persona_sha) == 16 and c.persona_sha.isalnum()
        for secret in (c.ironclaw_token, c.account_token):
            assert secret not in visible, "a version line carried a credential"
        # ...and the digest actually discriminates. The difference must be in the guidance
        # BODY: the marker line is an HTML comment and is stripped before composition, so two
        # tenants whose guidance differs only by slug legitimately compose identically.
        _tenant(d, "beta", service="account-analysis", group="-100900005")
        _guidance(d, "beta", "account-analysis",
                  BODY.replace("Synthetic Org", "A Different Synthetic Org"))
        beta = ing.load_clients(d)["beta"]
        assert beta.persona_sha != c.persona_sha, \
            "the persona digest is the same for two tenants with different guidance — it is " \
            "not fingerprinting what it claims to"
    print("  PASS service/persona versions are visible and carry no credential")


def test_definitions_declare_read_only_capabilities():
    """The repo's frozen boundary (no writes, no outreach, no egress, read-only records) is
    stated in prose in four places. A service definition is where a machine can check it."""
    for name in svc.available(ROOT):
        cap = svc.load_service(name, ROOT)["capabilities"]
        assert cap.get("account_records") == "read-only", f"{name}: records are not read-only"
        assert cap.get("writes") == "none", f"{name}: declares a write capability"
        assert cap.get("egress") == "none", f"{name}: declares egress"
        assert cap.get("outreach") == "none", f"{name}: declares outreach"
    print("  PASS every service definition declares the read-only, no-egress, no-outreach shape")


def test_model_policy_is_pinned_for_every_service():
    for name in svc.available(ROOT):
        assert svc.load_service(name, ROOT)["model_policy"] == "pin", \
            f"{name}: model_policy must be 'pin' — MODEL_PIN is a product promise, not a default"
    print("  PASS every service pins the model of record")


def test_off_pin_tenant_model_is_rejected_before_serving():
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "acme", service="account-analysis", extra="MODEL=OffPin/Model")
        try:
            ing.load_clients(d)
        except ValueError as e:
            assert "MODEL_PIN" in str(e) and "tenant MODEL override" in str(e), str(e)
        else:
            raise AssertionError("an off-pin tenant MODEL loaded onto the canonical serving path")
    print("  PASS an off-pin tenant MODEL is rejected before serving")


def test_off_pin_process_model_is_rejected_before_serving():
    prior = os.environ.get("MODEL")
    os.environ["MODEL"] = "OffPin/ProcessModel"
    try:
        with tempfile.TemporaryDirectory() as d:
            _tenant(d, "acme", service="account-analysis")
            try:
                ing.load_clients(d)
            except ValueError as e:
                assert "process MODEL" in str(e) and "Remove MODEL" in str(e), str(e)
            else:
                raise AssertionError("an off-pin process MODEL loaded the canonical serving path")
    finally:
        os.environ.pop("MODEL", None)
        if prior is not None:
            os.environ["MODEL"] = prior
    print("  PASS an off-pin process MODEL is rejected before serving")


def test_normal_registry_tenant_uses_the_literal_pin():
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "acme", service="account-analysis")
        c = ing.load_clients(d)["acme"]
        assert c.model == ing.MODEL == pins.require_pin("MODEL_PIN", ROOT)
    print("  PASS a normal registry tenant uses the literal repository MODEL_PIN")


def test_answer_quality_evaluation_claims_are_honest():
    account = svc.load_service("account-analysis", ROOT)
    relationship = svc.load_service("relationship-intelligence", ROOT)
    assert account["evaluation"] == "multi/eval", \
        "the account-analysis service lost its actual answer-quality suite"
    assert relationship["evaluation"] is None, \
        "relationship-intelligence falsely claims an account-analysis answer-quality suite"
    print("  PASS answer-quality evaluation is claimed only by the service it measures")


def test_the_internal_service_reasons_about_relationship_state_not_qualification():
    """The replacement's whole point, pinned as text (D-090).

    `relationship-intelligence@1` replaced `account-intelligence@1` to change the internal
    service's reasoning OBJECTIVE, not its name. A rename that left a prospect qualifier behind
    would satisfy every other test in this file — service loads, guidance binds, capabilities
    are frozen, the digest is stable — and still be the old product.

    THE DECLARATION IS THE THING UNDER TEST NOW. This used to assert a hardcoded list of phrases
    against the composed text, under a docstring saying the composition was "the only place the
    objective actually lives". That was true and was the defect: the one artifact meant to carry
    the service's meaning carried a file list, so a test that wanted the meaning had to grep a
    prompt for it. `responsibility` now carries it, and the anchors below are asserted against
    BOTH — present in the declaration, and addressed by the composition. Change the declaration
    to drop an anchor and this fails; change the persona to stop addressing one and this fails.
    Neither can drift from the other quietly.

    Deliberately NOT an answer-quality suite: that is `multi/eval/`, it needs a live instance,
    and it measures the account-analysis claim. This is the offline floor — it cannot tell you
    the persona reasons well, only that it is not aimed at the wrong question."""
    composed = per.compose_persona(ROOT)
    declared = svc.load_service(per._INTERNAL_SERVICE, ROOT)["responsibility"]

    # The load-bearing terms of THIS service's responsibility. Curated rather than tokenised on
    # purpose: extracting keywords from a sentence is a guess about which words matter, and a
    # guess is what makes a gate noisy enough to be switched off. These are checked against the
    # declaration first, so the declaration cannot quietly stop meaning them.
    for anchor in ("committed", "outstanding", "stale", "needs a person"):
        assert anchor in declared.lower(), \
            f"the declared responsibility no longer states {anchor!r} — if the objective really " \
            "changed, that is a product decision to make in the definition, not a test to edit"
        assert anchor in composed.lower(), \
            f"the composition never addresses {anchor!r}, which its own service definition " \
            "declares this service is answerable for"

    # Carried over: terms the composition must state that the one-line declaration does not have
    # room for. These remain persona assertions and are honest about it.
    for phrase in ("what is committed", "what is outstanding", "supersedes", "contradict"):
        assert phrase.lower() in composed.lower(), \
            f"the internal composition never mentions {phrase!r} — it is not stating the " \
            "relationship-state objective D-090 records"

    # The objective it must REFUSE. Each of these appears in the composition only inside a
    # prohibition, so assert the prohibition rather than the absence of the word.
    assert "You are not qualifying a sales pipeline" in composed, \
        "the internal composition no longer refuses the qualification objective by name"
    assert "Never infer a" in composed and "pipeline stage" in composed, \
        "nothing stops the model reading a pipeline stage out of the legacy sales columns"

    # Evidence discipline survived the replacement — this is shared behaviour, not sales copy.
    for tier in ("FACT", "STATED", "INFERENCE", "UNKNOWN"):
        assert f"**{tier}**" in composed, f"the {tier} tier was lost in the replacement"

    # It reasons over the persisted model and introduces no entity beside it.
    for entity in ("account", "contacts", "activities"):
        assert entity in composed.lower(), f"the composition never names {entity}"
    assert "relationship_id" not in composed.lower(), \
        "the composition names a relationship_id — there is no such entity (PRODUCT_DIRECTION §4)"
    print("  PASS the internal service states the relationship-state objective and refuses qualification")


def test_every_service_declares_its_own_distinct_responsibility():
    """A service is answerable for something, and it says so in its own words.

    Two failures this closes, both of which every other test in this file would pass. A new
    definition copy-pasted from an existing one inherits its responsibility and declares itself
    answerable for the wrong thing — which is exactly the mistake the two-agreeing-edits rule
    exists to make hard elsewhere. And a responsibility that merely restates `title` carries no
    information: the field would be ceremony, and the next reader would rightly ignore it.

    Nothing branches on this value at serve time and nothing should. It is checked, not consumed.
    """
    seen = {}
    for name in svc.available():
        d = svc.load_service(name, ROOT)
        r = d["responsibility"].strip()
        assert len(r) > 40, \
            f"{name}: responsibility {r!r} is too short to state what the service is answerable for"
        assert r.lower() != d["title"].strip().lower(), \
            f"{name}: responsibility merely restates the title — it carries no information"
        if r.lower() in seen:
            raise AssertionError(
                f"{name} and {seen[r.lower()]} declare the SAME responsibility — one of them is "
                "answerable for something it has not stated")
        seen[r.lower()] = name
    assert seen, "no service definitions were checked"
    print(f"  PASS {len(seen)} service(s) each declare a distinct responsibility")


def test_the_internal_service_output_contract_is_closed_and_channel_safe():
    """Two user-facing defects from the first real turn, pinned as text.

    (1) THE EVIDENCE VOCABULARY IS CLOSED AT FOUR. The turn printed `SCHEDULED` in an evidence
    column beside FACT and STATED. A fifth tier invented at answer time is not a richer
    epistemics, it is a label whose meaning nothing defines and which no reader can check
    against the record. Whether something is scheduled, pending, due or deferred is BUSINESS
    CONTENT; the tier says how well the record supports it. `HYPOTHESIS` went with the same
    change: on a relationship record a plausible guess is either INFERENCE with its basis named
    or UNKNOWN, and a tier that licenses speculation is the last thing this service needs.

    (2) TABLES DO NOT SURVIVE THE CHANNEL. The turn answered in markdown tables. The bridge
    strips markdown deterministically in `send()`, so a table arrives as unreadable runs of
    pipes -- the S1 defect. That is a SERVICE concern, not a transport one: the fix belongs in
    the composition that decides what an answer looks like, never in the bridge.

    Text assertions, deliberately. Whether the model obeys is a live-turn question and cannot be
    a CI gate; what CI can guarantee is that the instruction is present and unambiguous."""
    composed = per.compose_persona(ROOT)

    for tier in ("FACT", "STATED", "INFERENCE", "UNKNOWN"):
        assert f"**{tier}**" in composed, f"the {tier} tier is missing from the internal service"
    assert "HYPOTHESIS" not in composed, \
        "HYPOTHESIS is back in the internal composition -- the vocabulary is four, closed"
    assert "Never invent a fifth" in composed, \
        "nothing closes the evidence vocabulary; a turn can mint SCHEDULED again"
    # the closure must name the trap, or it reads as a style note
    assert "business\ncontent, not an evidence tier" in composed or \
           "business content, not an evidence tier" in composed.replace("\n", " "), \
        "the closure does not say WHY a fifth label is wrong"

    assert "Never use a markdown table" in composed, \
        "the internal service does not forbid tables, and the channel cannot render them"
    assert "chat window on a phone" in composed, \
        "the composition states no channel, so answer length is unconstrained"
    assert "Needs attention" in composed, "no attention section in the response shape"
    # and it must stay a default, not a skeleton every answer is poured into
    assert "not a template" in composed, \
        "the response shape reads as a fixed template -- a one-fact question deserves one sentence"

    # THE LIVE-ROOM DEFECTS. The first real Telegram turn came back at ~4.1k delivered
    # characters: it opened "I'll analyze the NEAR Foundation relationship record and report
    # on...", then spread one finding across six headed sections, restating the same caveat in
    # three of them. Accurate, and the wrong artefact for someone reading on a phone. Each rule
    # below answers one of those observed behaviours.
    assert "Start with the answer" in composed and "I'll analyze" in composed, \
        "nothing forbids the meta-preamble the first live turn opened with"
    assert "Say each thing once" in composed, \
        "nothing stops one finding being restated under three headings"
    assert "No headings by default" in composed, \
        "nothing stops an ordinary question becoming a multi-section report"
    assert "Select; do not restate" in composed, \
        "nothing tells the service it may leave unchanging record material out"
    print("  PASS the internal service's evidence vocabulary is closed and its shape is channel-safe")


def test_no_persona_part_injects_its_frontmatter():
    """Frontmatter is build metadata, not prompt. Every part reaches the model VERBATIM.

    The skill parts carry a YAML header; the persona parts do not. Injected, that header is a
    context pointer with nothing to point at — the body it would gate is already loaded, every
    turn, unconditionally — plus an `activation.keywords` list NOTHING in this repo reads,
    landing in the instructions as bare nouns attached to no sentence. `persona._read_part`
    strips it at the one funnel both composition paths share.

    Asserted over EVERY part of EVERY committed definition, not the two that carry a header
    today: the guard has to survive the next part someone adds with one."""
    for name in svc.available(ROOT):
        defn = svc.load_service(name, ROOT)
        for rel in list(defn["persona_parts"]) + [defn["safety_tail"]]:
            raw = (ROOT / rel).read_text()
            if not raw.startswith("---\n"):
                continue
            part = per._read_part(ROOT, rel)
            head = raw[4:raw.find("\n---\n", 3)]
            assert not part.startswith("---"), f"{rel}: frontmatter reaches the model intact"
            for line in head.splitlines():
                key = line.split(":")[0].strip(" -")
                if key and key.isidentifier():
                    assert not part.startswith(key), f"{rel}: frontmatter key {key!r} survives"
            assert "activation:" not in part, f"{rel}: an unread keyword list is being injected"

    # The composed article, since that is what is actually sent.
    composed = per.compose_persona(ROOT)
    for leak in ("activation:", "version: 0.1.0", "name: relationship-record"):
        assert leak not in composed, f"the internal composition injects {leak!r} as prompt"
    print("  PASS no persona part injects its frontmatter")


def test_an_internal_relationship_tenant_declares_an_empty_gap_shape():
    """A tenant on the internal service must DECLARE `FACT_FIELDS=`, even though empty.

    The one regression here that actually bit, and it was configuration rather than behaviour.
    Silence in the registry is not "no gaps" — it is UNDECLARED, and the seam then falls back to
    the Account Service's sales-shaped list and injects `missing fields (genuinely unknown):
    budget, timeline, decision_process, economic_buyer, stated_problem` into every envelope. A
    relationship book has no use for any of them, and the model believes the record over the
    persona, so it reports them as real gaps. That reached a live room.

    WHY THIS IS A CONFIG ASSERTION AND NOT AN ANSWER-QUALITY CASE. The symptom appeared roughly
    one turn in two, so a grader over model prose is a poor detector for it — and two controls
    showed that the persona paragraph written alongside the fix does not prevent the echo. The
    declaration does. Check the cause, deterministically, offline."""
    with tempfile.TemporaryDirectory() as d:
        _tenant(d, "declared", service="relationship-intelligence", group="-100900041")
        env = pathlib.Path(d) / "declared.env"
        env.write_text(env.read_text() + "FACT_FIELDS=\n")
        assert ing.load_clients(d)["declared"].fact_fields == (), \
            "an explicit empty declaration must parse as DECLARED EMPTY"

        # ...and the failure this guards: absent means undeclared, which means the sales list.
        _tenant(d, "silent", service="relationship-intelligence", group="-100900042")
        assert ing.load_clients(d)["silent"].fact_fields is None, \
            "an absent key must stay UNDECLARED — collapsing it to () would hide the defect"
    print("  PASS an internal tenant's empty gap shape is declared, not inferred from silence")


def test_definitions_are_json_and_hold_no_secret_shaped_values():
    """Definitions are COMMITTED. A token pasted into one would ship to the public repo."""
    for p in sorted((ROOT / "multi" / "services").glob("*.json")):
        text = p.read_text()
        d = json.loads(text)
        assert "token" not in text.lower() or "tool_policy" in d, \
            f"{p.name}: the word 'token' appears in a committed service definition"
        for v in d.values():
            if isinstance(v, str):
                assert len(v) < 400 or "\n" in v, f"{p.name}: a long opaque string looks minted"
    print("  PASS committed definitions are plain JSON with no secret-shaped values")


if __name__ == "__main__":
    # Discovered, not listed — a hand-maintained call list drifts.
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL SERVICE-DEFINITION TESTS PASS")
