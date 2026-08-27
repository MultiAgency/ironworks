#!/usr/bin/env python3
"""Central redaction — the forms a credential actually takes on a printable path.

Run: python3 test_redact.py   (from multi/seam)

Each case below is a shape that the previous exact-substring redactor let through. They are
not hypothetical variations on a theme: a percent-encoded token and a plain one are different
strings, and `urllib` produces the encoded one on the way into the error a human then reads.
"""
try:
    from . import redact as r
except ImportError:
    import redact as r

TOKEN = "s3cr3t-token-value"
# A value that percent-encodes to something different — the case an exact-substring redactor
# silently misses, and the reason `variants()` exists.
ENCODED_TOKEN = "abc+def/ghi=jkl mno"
BOT_TOKEN = "123456789:AAHf-ExampleBotTokenValue_0123456789"


def test_exact_value_is_masked():
    assert TOKEN not in r.redact(f"failed with {TOKEN}", [TOKEN])
    print("  PASS exact value")


def test_percent_encoded_value_is_masked():
    import urllib.parse
    enc = urllib.parse.quote(ENCODED_TOKEN, safe="")
    assert enc != ENCODED_TOKEN, "the fixture must actually differ once encoded"
    out = r.redact(f"GET /x?t={enc} failed", [ENCODED_TOKEN])
    assert enc not in out and ENCODED_TOKEN not in out, out
    # ...and the plus-encoded form too (urlencode uses quote_plus, urlopen uses quote).
    plus = urllib.parse.quote_plus(ENCODED_TOKEN)
    assert plus not in r.redact(f"body t={plus}", [ENCODED_TOKEN])
    print("  PASS percent- and plus-encoded forms")


def test_query_string_credentials_are_masked_even_when_unknown():
    """A credential that arrived from somewhere else — in an upstream error, say — is exactly
    the one no secret set contains. Shape has to carry it."""
    out = r.redact("https://svc.example/list?access_token=NOT-IN-OUR-SET&page=2", [])
    assert "NOT-IN-OUR-SET" not in out, out
    assert "page=2" in out, "a non-credential parameter was collateral damage"
    print("  PASS unknown query-string credentials")


def test_header_lines_are_masked():
    for line in ("Authorization: Bearer abc.def.ghi",
                 "authorization: abc.def.ghi",
                 "X-Service-Token: mia_sales_token_value"):
        out = r.redact(f"request failed\n{line}\nstatus 401", [])
        assert "abc.def.ghi" not in out and "mia_sales_token_value" not in out, out
        assert "status 401" in out
    print("  PASS Authorization / X-Service-Token header lines")


def test_a_non_bearer_scheme_does_not_leave_the_credential_in_the_clear():
    """THE WORST OUTPUT IS THE ONE THAT LOOKS REDACTED. Only `Bearer` was consumed as a scheme,
    so for anything else the credential group matched the SCHEME WORD and the secret survived
    beside a `<redacted>` that a reader would trust and paste:

        'Proxy-Authorization: Basic dXNlcjpzZWNyZXQ=' -> 'Proxy-Authorization: <redacted> dXNlcjpzZWNyZXQ='

    `proxy-authorization` is in the pattern for the CONNECT proxy under deploy/egress/, where
    `Basic` is the normal scheme — so the header most likely to carry a non-Bearer credential
    was the one guaranteed to leak it. This suite tested Bearer and bare values only."""
    for line, secret in (
            ("Proxy-Authorization: Basic dXNlcjpzZWNyZXRwYXNzd29yZA==", "dXNlcjpzZWNyZXRwYXNzd29yZA=="),
            ("Authorization: Token tok_live_abc123", "tok_live_abc123"),  # gitleaks:allow — synthetic redaction fixture
            ("authorization: ApiKey k-secret-9", "k-secret-9")):
        out = r.redact(f"upstream said\n{line}\nstatus 407", [])
        assert secret not in out, out
        assert r.MASK in out, out
        assert "status 407" in out, "the diagnosis was redacted along with the credential"
    print("  PASS Basic / Token / ApiKey schemes are masked with their credential, not instead of it")


def test_telegram_bot_token_url_shape_is_masked():
    url = f"https://api.telegram.org/bot{BOT_TOKEN}/getUpdates"
    out = r.redact(f"poll error at {url}", [])
    assert BOT_TOKEN not in out, out
    assert "/getUpdates" in out, "the method name is diagnostic and should survive"
    print("  PASS the /bot<token>/ URL shape, without knowing the token")


def test_a_secret_that_prefixes_another_leaves_no_tail():
    short, long = "tok-abc", "tok-abcdef"
    out = r.redact(f"used {long} then {short}", [short, long])
    assert "def" not in out, f"the longer secret left a tail: {out}"
    print("  PASS longest-first ordering leaves no tail")


def test_empty_and_none_secrets_do_not_redact_everything():
    """An unset env var reaching the secret set as '' would mask every character boundary."""
    out = r.redact("nothing secret here", ["", None])
    assert out == "nothing secret here", out
    assert r.secrets_of({}, "", None) == set()
    print("  PASS empty/None secrets are dropped, not applied")


def test_non_string_input_is_accepted():
    """The normal call is redact(exception, secrets) — an exception, not a str."""
    e = RuntimeError(f"HTTP 401 for https://x/?token={TOKEN}")
    assert TOKEN not in r.redact(e, [TOKEN])
    print("  PASS exceptions and other non-strings")


def test_secrets_of_collects_registry_credentials():
    class C:
        def __init__(self, a, b):
            self.ironclaw_token, self.account_token = a, b
    got = r.secrets_of({"a": C("ic-a", "ac-a"), "b": C("ic-b", "ac-b")}, "bot-token")
    assert got == {"ic-a", "ac-a", "ic-b", "ac-b", "bot-token"}
    print("  PASS secrets_of gathers every tenant credential plus extras")


def test_the_bridge_uses_the_central_redactor():
    """The bridge is the path that prints in front of clients. If it ever re-grows a local
    redactor, this catches it: its helpers must delegate."""
    import inspect
    import pathlib
    src = pathlib.Path(inspect.getfile(r)).with_name("telegram_bridge.py").read_text()
    assert "import redact as redact_mod" in src
    assert "redact_mod.redact(" in src and "redact_mod.secrets_of(" in src
    assert "s = s.replace(sec" not in src, "the bridge re-grew its own substring redactor"
    print("  PASS the bridge delegates to the central redactor")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL REDACTION TESTS PASS")
