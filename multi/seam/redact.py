"""One redactor for every path that can print — logs, errors, operator output, artifacts.

WHY CENTRAL. Redaction was a four-line helper inside `telegram_bridge.py` that only the bridge
could reach, and it only ever replaced EXACT secret substrings. Everything else that can print
— the operator CLI, a provisioning journal, a release artifact, a traceback from the Account
Service client — either had its own copy or had none. A redactor that half the printing paths
cannot import is not a control, it is a habit.

WHAT IT COVERS, and why each form is here rather than "obviously the same thing":

  1. The exact value.
  2. Its percent-encoding. `getUpdates` and the Account Service both take credentials in URLs;
     `urllib` percent-encodes on the way out, so the string in an error is not the string in
     the secret set. A token with no reserved characters encodes to itself, which is exactly
     why this was easy to miss — it only bites for the ones that do.
  3. Query-string values for known credential parameter names, and `Authorization:`/
     `X-Service-Token:` header lines, even when the value is one this process never held. A
     redactor that only knows its OWN secrets cannot redact a credential that arrived in an
     error from somewhere else, and those are the ones nobody is watching for.
  4. The Telegram bot-token path segment (`/bot<token>/method`), which is a URL SHAPE rather
     than a value — it is how the bot token leaks in practice.

ORDER MATTERS. Longest values first, so a secret that contains another secret as a prefix
cannot leave the shorter one's tail behind. And every caller redacts BEFORE the string reaches
a sink; there is no "log it then scrub the log" path here, because there is no way to unsend.

NOT A GUARANTEE. This is defence in depth over custody: the primary control is that
credentials live in the seam and are never placed where they could be printed. A redactor is
what makes the mistake survivable, not what makes it safe.
"""
import re
import urllib.parse

MASK = "<redacted>"

# Query parameters whose value is a credential wherever it appears, whoever minted it.
_CRED_PARAMS = ("token", "access_token", "api_key", "apikey", "key", "secret",
                "service_token", "auth", "password", "signature")
_QS = re.compile(r"(?i)\b(" + "|".join(map(re.escape, _CRED_PARAMS)) + r")=([^&\s\"']+)")
# Header lines, as they appear in a formatted exception or a curl trace. ANY scheme is consumed
# with the credential, not just Bearer.
#
# `(?:bearer\s+)?` was the only one, so for every other scheme the credential group matched the
# SCHEME WORD and the secret survived in a line that looks redacted:
#
#   'Proxy-Authorization: Basic dXNlcjpzZWNyZXQ='  ->  'Proxy-Authorization: <redacted> dXNlcjpzZWNyZXQ='
#
# `proxy-authorization` is in this alternation specifically for the CONNECT proxy under
# deploy/egress/, where `Basic` is the normal scheme — so the header most likely to carry a
# non-Bearer credential was the one guaranteed to leak it. A masked-LOOKING line is worse than
# an unmasked one, because a reader will paste it.
#
# The scheme group is optional and the value still stops at whitespace, so a bare value
# (`X-Service-Token: acct-abc`) is masked by backtracking, and prose after the credential
# ("… failed with 401") survives — masking to end-of-line would take the diagnosis with it.
# The scheme separator is HORIZONTAL space only: with `\s+` a bare-value header would consume
# its own newline and mask the first word of the next line as the credential.
_HEADER = re.compile(r"(?i)\b(authorization|x-service-token|x-api-key|proxy-authorization)"
                     r"([ \t]*:[ \t]*)(?:[A-Za-z][A-Za-z0-9._-]*[ \t]+)?([^\s\"',}\]]+)")
# The Telegram bot-token URL shape: https://api.telegram.org/bot<TOKEN>/getUpdates
_TG_PATH = re.compile(r"(/bot)([0-9]{4,}:[A-Za-z0-9_-]{20,})")


def variants(secret):
    """Every textual form one secret can take on a printable path."""
    if not secret:
        return ()
    forms = {secret, urllib.parse.quote(secret, safe=""), urllib.parse.quote_plus(secret)}
    return tuple(f for f in forms if f)


def redact(text, secrets=()):
    """Return `text` with known secrets and credential-shaped material masked.

    `secrets` is any iterable of values this process holds. Non-string input is coerced, so
    `redact(exception, secrets)` is the normal call — an exception's `str()` is where a
    token-bearing URL usually surfaces."""
    s = str(text)
    forms = set()
    for sec in secrets or ():
        forms.update(variants(sec))
    # Longest first: a secret that is a prefix of another must not leave the tail behind.
    for form in sorted(forms, key=len, reverse=True):
        s = s.replace(form, MASK)
    # Shape-based passes run AFTER the value pass, so a known secret is masked by identity
    # (and stays masked) rather than depending on the regexes recognising its context.
    s = _TG_PATH.sub(r"\1" + MASK, s)
    s = _QS.sub(lambda m: f"{m.group(1)}={MASK}", s)
    s = _HEADER.sub(lambda m: f"{m.group(1)}{m.group(2)}{MASK}", s)
    return s


def secrets_of(clients, *extra):
    """The credential set for a loaded registry: every tenant's tokens, plus anything else the
    caller holds (a bot token, an operator token). Falsy values are dropped — an unset env var
    must never become the empty string that redacts everything."""
    out = set()
    for c in (clients or {}).values():
        out.update((getattr(c, "ironclaw_token", ""), getattr(c, "account_token", "")))
    out.update(extra)
    return {s for s in out if s}
