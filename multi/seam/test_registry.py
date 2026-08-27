#!/usr/bin/env python3
"""The tenant registry: what `load_clients` preserves, and what it refuses.
Run: python3 test_registry.py   (from multi/seam)

NO INSTANCE AND NO NETWORK, and that is a contract rather than a convenience. `load_clients`
must stay usable on a clean clone with nothing running — `multi/verify/test_fixtures_offline.py`
pins it from the other side — so every check in `registry.py` is a local one, and this suite
imports `registry` alone. Runtime identity probes belong at bridge startup and are tested by
`test_operator_token_guard.py`.

The registry's other fail-closed refusals are pinned HERE, beside the code that raises them,
because each is a silent-wrong-answer if it misses and none of them needs an instance: duplicate
slug, duplicate group id, a shared member token, cross-wired credentials, and the canonical form
of the Account Service base. The two whose consequence is only visible elsewhere stay there —
`test_operator_token_guard.py` (the operator token in a member slot, which needs the runtime
probe) and `test_client_guidance.py` (a shared ACCOUNT_TOKEN, which is a shared data scope).
Guidance and service refusals belong to `test_client_guidance.py` and `test_services.py`.

AN EARLIER VERSION OF THIS PARAGRAPH CLAIMED all of them were already covered by those three
files, and four were not. The wrong claim is what let the gap persist, so it is recorded rather
than quietly replaced: `duplicate CLIENT_SLUG` and `IRONCLAW_TOKEN == ACCOUNT_TOKEN` had no test
anywhere; the shared member token was reached only incidentally by
`deploy/lib/test_ironworks_cli.py`, whose subject is credential redaction and which asserts a
non-zero exit without ever checking WHICH refusal fired; and the duplicate group id — the
sharpest, since a miss serves a whole Telegram group the WRONG tenant's tokens and data — was
pinned only by a live adversarial proof. A docstring that says "covered" is not coverage.
"""
import os, pathlib, tempfile

try:
    from . import registry as reg
except ImportError:
    import registry as reg


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


def _reg(base, slug, **over):
    """One tenant on disk: its env file plus the canonical slug-bound guidance beside it."""
    kv = {"CLIENT_SLUG": slug, "ACCOUNT_TOKEN": f"at-{slug}", "IRONCLAW_TOKEN": f"it-{slug}"}
    kv.update({k: v for k, v in over.items() if v is not None})
    (base / f"{slug}.env").write_text("".join(f"{k}={v}\n" for k, v in kv.items()))
    (base / f"{slug}.guidance.md").write_text(_synthetic_guidance(slug))


def _refuses(base, *must_name):
    """Assert load_clients refuses, and that the refusal NAMES what an operator has to fix.

    A fail-closed check whose message does not say which two files collide sends the operator
    to read the whole registry directory, which for a real deployment is where the credentials
    are. Returns the message so a caller can make further assertions on it."""
    try:
        reg.load_clients(base)
    except ValueError as e:
        for s in must_name:
            assert s in str(e), f"the refusal did not name {s!r}: {e}"
        return str(e)
    raise AssertionError(f"load_clients ACCEPTED a registry it must refuse ({must_name})")


def test_agency_dir_relocates_the_default_registry():
    saved = {key: os.environ.get(key) for key in ("AGENCY_DIR", "CLIENTS_DIR")}
    with tempfile.TemporaryDirectory() as d:
        try:
            os.environ["AGENCY_DIR"] = d
            os.environ.pop("CLIENTS_DIR", None)
            clients = pathlib.Path(d) / "clients"
            clients.mkdir()
            _reg(clients, "relocated")
            assert set(reg.load_clients()) == {"relocated"}
        finally:
            for key, value in saved.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
    print("  PASS AGENCY_DIR relocates the default tenant registry")


def test_a_duplicate_client_slug_is_refused():
    """Two files claiming one slug. `clients` is a dict keyed by slug, so without this the LATER
    file silently REPLACES the earlier tenant — its tokens, its guidance, its service — and the
    registry reports one fewer client than exists on disk with no indication which one won."""
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        _reg(base, "alpha")
        # A second FILE whose CLIENT_SLUG claims alpha. Its own guidance is bound to "bravo",
        # so the slug check must fire BEFORE guidance is read, or the operator gets a confusing
        # guidance error for what is really a duplicate-identity mistake.
        (base / "bravo.env").write_text(
            "CLIENT_SLUG=alpha\nACCOUNT_TOKEN=at-bravo\nIRONCLAW_TOKEN=it-bravo\n")
        (base / "bravo.guidance.md").write_text(_synthetic_guidance("bravo"))
        _refuses(base, "duplicate client slug", "alpha")
    print("  PASS a duplicate CLIENT_SLUG is refused, not silently collapsed to one tenant")


def test_a_duplicate_telegram_group_id_is_refused():
    """THE SHARPEST ONE. `telegram_bridge.load_groups()` builds {gid: client}, so a duplicate gid
    keeps whichever file sorted last and serves that ENTIRE Telegram group with another tenant's
    member token and another tenant's account records. Pinned here, offline, next to the code
    that raises it — the only thing pinning it before was a live adversarial proof."""
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        _reg(base, "alpha", TELEGRAM_GROUP_ID="-100777")
        _reg(base, "bravo", TELEGRAM_GROUP_ID="-100777")
        msg = _refuses(base, "TELEGRAM_GROUP_ID", "-100777", "alpha")
        assert "at-alpha" not in msg and "it-alpha" not in msg, \
            f"the refusal echoed a credential: {msg}"
    print("  PASS one Telegram group cannot map to two tenants (misroute is fail-closed)")


def test_two_tenants_cannot_share_one_member_token():
    """Two tenants on one IRONCLAW_TOKEN are the SAME IronClaw member: one thread history, one
    memory, one egress confinement. Every per-tenant isolation claim rests on member identity
    being one-to-one and nothing else in the seam checks it — `assert_no_member_is_the_operator`
    asks whether each token is an operator, never whether two tenants hold the same one."""
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        _reg(base, "alpha")
        _reg(base, "bravo", IRONCLAW_TOKEN="it-alpha")
        msg = _refuses(base, "IRONCLAW_TOKEN", "alpha")
        assert "it-alpha" not in msg, f"the refusal echoed the token itself: {msg}"
    print("  PASS two tenants cannot share one member token (same identity, shared threads)")


def test_cross_wired_credentials_are_refused():
    """IRONCLAW_TOKEN == ACCOUNT_TOKEN means one string is presented BOTH as an IronClaw member
    bearer and as an Account Service org credential. Whichever it really is, the other surface
    is being handed a credential minted for a different trust domain."""
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        _reg(base, "alpha", IRONCLAW_TOKEN="same-token", ACCOUNT_TOKEN="same-token")
        msg = _refuses(base, "IRONCLAW_TOKEN", "ACCOUNT_TOKEN")
        assert "same-token" not in msg, f"the refusal echoed the credential: {msg}"
    print("  PASS cross-wired IRONCLAW_TOKEN/ACCOUNT_TOKEN is refused")


def test_account_service_base_identity_is_canonical_and_fails_closed():
    """This value IS a continuity decision. It is persisted as `account_service_base` and a
    mismatch refuses to continue a persisted conversation (docs/BRIDGE_DELIVERY.md), so it must
    normalise everything that is genuinely one endpoint and refuse everything it cannot
    represent honestly. Six raise branches, IPv6 bracketing and default-port normalisation, and
    no direct test until now — while `thread_identity` fed it straight into the bridge store.

    Credentials are refused because the value is PERSISTED: Account Service authentication
    belongs in X-Service-Token and must never reach a database row."""
    same = reg.account_service_base_identity

    # NORMALISATION: each pair is one endpoint written two ways and must yield ONE identity,
    # or a cosmetic ACCOUNT_BASE edit would strand every tenant's conversation.
    for written, canonical in (("http://Host:80/", "http://host"),
                               ("https://Host:443/api/", "https://host/api"),
                               ("http://HOST/", "http://host"),
                               ("https://[2001:db8::1]:443/api/", "https://[2001:db8::1]/api")):
        assert same(written) == same(canonical) == canonical, \
            f"{written!r} -> {same(written)!r}, expected {canonical!r}"

    # An IPv6 literal survives its brackets. `urlsplit().hostname` strips them, so the function
    # puts them back; without that the identity would be an unparseable `http://::1:8443`.
    assert same("http://[::1]:8443") == "http://[::1]:8443"
    assert same("http://[::1]") == "http://[::1]"

    # A NON-default port is never dropped: two services on one host are two trust endpoints.
    assert same("http://host:8443") != same("http://host:80")
    assert same("http://host:8443") != same("https://host:8443"), "scheme is part of identity"

    # ...and every refusal, each with the message an operator has to act on.
    for bad, expect in (("", "absolute http(s) URL"),
                        (None, "absolute http(s) URL"),
                        ("127.0.0.1:8443", "absolute http(s) URL"),
                        ("ftp://host", "absolute http(s) URL"),
                        ("http:///path", "absolute http(s) URL"),
                        ("http://::1:8443", "absolute http(s) URL"),
                        ("http://u:p@host", "must not contain credentials"),
                        ("http://host?x=1", "query or fragment"),
                        ("http://host#f", "query or fragment"),
                        ("http://host:notaport", "invalid port")):
        try:
            same(bad)
        except ValueError as e:
            assert expect in str(e), f"{bad!r} refused with the wrong reason: {e}"
            continue
        raise AssertionError(f"account_service_base_identity ACCEPTED {bad!r}")

    # The credential refusal must not print the credential it is refusing. It is the one input
    # here that carries a secret, and the message travels into operator logs.
    try:
        same("http://svc:s3cret@host:8443")
    except ValueError as e:
        assert "s3cret" not in str(e), f"the refusal echoed the password: {e}"
    print("  PASS account service base identity: canonical, port-aware, and fails closed")


def test_fact_fields_presence_survives_registry_parsing():
    """The distinction is only real if the registry parser preserves it. `kv` holds exactly the
    keys a file wrote, so presence is already there — it was being discarded one line later."""
    assert reg._parse_fact_fields({}) is None, "an absent key must parse as UNDECLARED"
    assert reg._parse_fact_fields({"FACT_FIELDS": ""}) == (), \
        "an explicitly empty key must parse as DECLARED EMPTY, not as absent"
    assert reg._parse_fact_fields({"FACT_FIELDS": "   "}) == (), "whitespace-only is still declared"
    assert reg._parse_fact_fields({"FACT_FIELDS": "allocation"}) == ("allocation",)
    assert reg._parse_fact_fields({"FACT_FIELDS": " a , ,b "}) == ("a", "b"), "trim and drop blanks"

    # ...and end to end through load_clients, which is what actually builds the tenant.
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        for slug, line in (("absent", ""), ("empty", "FACT_FIELDS=\n"), ("listed", "FACT_FIELDS=allocation\n")):
            (base / f"{slug}.env").write_text(
                f"CLIENT_SLUG={slug}\nACCOUNT_TOKEN=at-{slug}\nIRONCLAW_TOKEN=it-{slug}\n" + line)
            (base / f"{slug}.guidance.md").write_text(_synthetic_guidance(slug))
        clients = reg.load_clients(base)
    assert clients["absent"].fact_fields is None
    assert clients["empty"].fact_fields == ()
    assert clients["listed"].fact_fields == ("allocation",)
    print("  PASS FACT_FIELDS presence survives registry parsing (None / () / declared)")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL REGISTRY TESTS PASS")
