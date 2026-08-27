#!/usr/bin/env python3
"""T3 — a tenant's member token must be a sealed MEMBER, verified against the runtime.
Run: python3 test_operator_token_guard.py   (from multi/seam)

THE DEFECT THIS REPLACES. `load_clients` refuses a registry entry whose IRONCLAW_TOKEN is the
operator token, but recognises one by comparing against the LOADING PROCESS's environment. The
bridge carries none of IRONCLAW_OPERATOR_TOKEN / IRONCLAW_REBORN_WEBUI_TOKEN / WEBUI_TOKEN, so
that set is empty and the check has never fired there. Measured three ways: a loop over
load_clients with the operator env var as the only variable (REFUSED with it, LOADED without),
the serve host's bridge.env carrying none of the three, and the asymmetry that `deploy/ironworks`
DOES carry one — so the check is alive on the operator's own box, where it is harmless, and
inert in the process that serves tenants.

The first test below is that defect, pinned. It asserts the OLD guard's blindness rather than
its correctness, so it stays honest if someone re-adds an env comparison and believes it works.

The rest exercise the replacement, which asks the runtime instead: an operator identity is
accepted on the admin surface, a sealed member is refused there. No live instance needed —
the probe is driven against a fake.
"""
import os
import pathlib
import tempfile
import urllib.error

os.environ.setdefault("IRONCLAW_API", "http://test.invalid")
try:
    from . import context_ingress as ing
except ImportError:
    import context_ingress as ing

GUIDANCE = ("<!-- client-guidance v1 slug: {slug} -->\n"
            "# Organization guidance\n## Company & offer\nSynthetic, for tests only.\n"
            "## Target customer\nNobody: every record here is invented.\n"
            "## Qualification criteria\n- A stated problem\n- A named person\n"
            "## Disqualification criteria\n- A do-not-contact flag\n"
            "## Account stages\nnew -> reviewed -> decided.\n"
            "## Supported evidence sources\nThe loaded book and what the team states.\n"
            "## Desired decisions\nWhich accounts need attention this week.\n"
            "## Prohibited claims & actions\nNever contact anyone. Read-only always.\n")


def _registry(d, token):
    pathlib.Path(d, "acme.guidance.md").write_text(GUIDANCE.format(slug="acme"))
    pathlib.Path(d, "acme.env").write_text(
        f"CLIENT_SLUG=acme\nCLIENT_NAME=Acme\nIRONCLAW_TOKEN={token}\n"
        "ACCOUNT_TOKEN=acct-acme\nTELEGRAM_GROUP_ID=-100999\n")
    return d


class _Resp:
    def __init__(self, status): self.status = status
    def __enter__(self): return self
    def __exit__(self, *a): return False


def _fake_urlopen(status=None, raise_http=None, raise_os=None):
    def open_(req, timeout=None):
        if raise_os:
            raise OSError(raise_os)
        if raise_http:
            raise urllib.error.HTTPError(req.full_url, raise_http, "no", {}, None)
        return _Resp(status)
    return open_


def _with_fake(fake, fn):
    real = ing.urllib.request.urlopen
    ing.urllib.request.urlopen = fake
    try:
        return fn()
    finally:
        ing.urllib.request.urlopen = real


# ── the defect, pinned ────────────────────────────────────────────────────────────────

def test_the_env_comparison_is_blind_without_the_operator_token():
    """THE BUG, asserted as behaviour. Identical registry, one variable: whether the process
    holds the operator token. This is what the bridge's environment looks like."""
    optok = "op-token-FIXTURE-not-a-real-credential"
    saved = {k: os.environ.pop(k, None)
             for k in ("IRONCLAW_OPERATOR_TOKEN", "IRONCLAW_REBORN_WEBUI_TOKEN", "WEBUI_TOKEN")}
    try:
        with tempfile.TemporaryDirectory() as d:
            _registry(d, optok)
            clients = ing.load_clients(d)             # no operator token in the environment
            assert "acme" in clients, "registry did not load at all — wrong failure"
            os.environ["WEBUI_TOKEN"] = optok         # the operator's own box
            try:
                ing.load_clients(d)
            except ValueError as e:
                assert "operator" in str(e).lower()
            else:
                raise AssertionError("the env comparison did not fire even WITH the token set")
    finally:
        os.environ.pop("WEBUI_TOKEN", None)
        for k, v in saved.items():
            if v is not None:
                os.environ[k] = v
    print("  PASS the env comparison is blind unless the process itself holds the operator token")


# ── the replacement ───────────────────────────────────────────────────────────────────

def test_a_sealed_member_is_accepted():
    """401 and 403 both mean 'refused on the admin surface', which is what a member looks like."""
    with tempfile.TemporaryDirectory() as d:
        clients = ing.load_clients(_registry(d, "member-token"))
        for refused in (401, 403):
            _with_fake(_fake_urlopen(raise_http=refused),
                       lambda: ing.assert_no_member_is_the_operator(clients))
    print("  PASS a token refused on the admin surface (401/403) is a sealed member")


def test_an_operator_identity_is_refused():
    """Accepted on the admin surface = operator. This is the case the old guard could not see."""
    with tempfile.TemporaryDirectory() as d:
        clients = ing.load_clients(_registry(d, "actually-the-operator"))
        try:
            _with_fake(_fake_urlopen(status=200),
                       lambda: ing.assert_no_member_is_the_operator(clients))
        except ing.OperatorTokenInRegistry as e:
            assert "acme" in str(e) and "OPERATOR" in str(e)
            assert "provision-client.sh" in str(e), "the refusal must say how to fix it"
        else:
            raise AssertionError("an admin-accepted token was allowed to serve")
    print("  PASS a token ACCEPTED on the admin surface is refused, and the error says how to fix it")


def test_an_unreachable_instance_fails_closed():
    """THE POLICY, pinned. Unknown is not a pass. Follows load_groups (raises), not _catalog
    (degrades): a control that cannot verify identity does not serve. Changing this to degrade
    restores exactly the silence this check replaces, so it must break a test to change."""
    with tempfile.TemporaryDirectory() as d:
        clients = ing.load_clients(_registry(d, "member-token"))
        try:
            _with_fake(_fake_urlopen(raise_os="connection refused"),
                       lambda: ing.assert_no_member_is_the_operator(clients))
        except ing.OperatorTokenInRegistry as e:
            assert "cannot reach" in str(e) and "Refusing to serve" in str(e)
        else:
            raise AssertionError("an unreachable instance was treated as a pass — fail-open")
    print("  PASS an unreachable instance fails CLOSED, not open")


def test_the_check_is_not_at_registry_load():
    """D-077's placement, pinned. `load_clients` must stay usable with no instance reachable —
    multi/verify/test_fixtures_offline.py validates committed fixtures on a clean clone, and a
    network probe at load time would break that contract."""
    with tempfile.TemporaryDirectory() as d:
        def boom(*a, **k):
            raise AssertionError("load_clients touched the network")
        _with_fake(boom, lambda: ing.load_clients(_registry(d, "member-token")))
    print("  PASS load_clients performs no network probe — the check is at startup, not load")


def test_the_probe_carries_the_browser_user_agent():
    """WHY A HEADER IS WORTH A TEST. This probe reads 403 as "refused on the admin surface", i.e.
    as a sealed member. Cloudflare's bot-protection 1010 block is ALSO a 403, and it is served by
    the edge before the request ever reaches the admin route. Sent as `Python-urllib/3.x` — which
    is what this request was, while every other outbound call in `context_ingress` carries
    `BROWSER_UA` — a bot-blocked edge would certify every tenant as sealed, including one holding
    the operator token. The check would then be silently doing the opposite of its job."""
    seen = {}

    def open_(req, timeout=None):
        seen["ua"] = req.get_header("User-agent")
        raise urllib.error.HTTPError(req.full_url, 401, "no", {}, None)

    with tempfile.TemporaryDirectory() as d:
        clients = ing.load_clients(_registry(d, "member-token"))
        _with_fake(open_, lambda: ing.assert_no_member_is_the_operator(clients))
    assert seen.get("ua") == ing.BROWSER_UA, (
        f"the operator-token probe went out as {seen.get('ua')!r}. A Cloudflare-fronted instance "
        "1010-blocks that agent with a 403, which this check reads as a sealed member.")
    print("  PASS the operator-token probe carries BROWSER_UA, so an edge block cannot pass as sealed")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL OPERATOR-TOKEN GUARD TESTS PASS")
