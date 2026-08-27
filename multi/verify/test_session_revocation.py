#!/usr/bin/env python3
"""Does anything on the pinned runtime actually REVOKE a sealed member's session?

Deprovisioning is the one lifecycle step whose success claim is a security claim: "this client
no longer has access." Every other proof in this directory asks whether a live client is
contained. This one asks whether a *former* client is.

WHY A PROBE AND NOT A DOC. The answer is knowable from the pinned source, and it is written
down in SECURITY.md — but a source reading goes stale silently on the next pin
bump, and this is precisely the claim we must not get wrong. So the runbook's exit codes are
driven by a measurement, and this script is that measurement, run as a proof and again by
`multi/provision/deprovision.sh` on every real deprovision.

WHAT IT MEASURES, in order, on a THROWAWAY member (never a real client):

  A. The probe is meaningful — a freshly minted member token authenticates a member route.
  B. Suspension (`POST admin/users/<id>/status {"status":"suspended"}`) — does it cut the
     member off the PRODUCT surface, or only the admin surface?
  C. Deletion (`DELETE admin/users/<id>`) — does the token stop authenticating?
  D. Is there a session-revocation route mounted at all (`POST /auth/logout`)?

The probe route is `GET /v1/responses/<uuid-that-does-not-exist>`:
  401 -> the bearer was rejected (authority is gone)
  404 -> the bearer was ACCEPTED and the response id merely does not exist (authority remains)
It costs no model call, which is what makes it usable inside deprovision.sh.

EXPECTED AT THE PINNED REV (`IRONCLAW_PIN` 70795c16e, source-traced then measured):
  - `SignedTokenSessionStore::lookup` consults only the HMAC signature, `exp`, the
    process-local revoked set, and the tenant — it never reads the user directory. So neither
    suspension nor deletion can affect it.
  - `POST /auth/logout` is mounted only by the SSO auth router; in env-bearer mode
    (`empty_webui_v2_auth_providers_mount`) it is deliberately absent, so there is no route
    that reaches `revoke()` at all.
  - `ADMIN_API_TOKEN_LIFETIME_DAYS = 365`, a Rust constant with no config path.
  => a deprovisioned client's member token keeps authenticating for up to a year.

That is the finding this script exists to keep honest, and it is why deprovision.sh exits 3
(RESIDUAL AUTHORITY) rather than 0. It is NOT a reason to relax: the containment is custody —
the token never leaves the seam — plus the global rotation in deploy/README.md.

EXIT CODES (also the contract deprovision.sh reads):
  0  the probed token is REJECTED after deletion — verified revoked
  3  the probed token is still ACCEPTED after deletion — residual authority (expected here)
  1  a hard check failed (the admin surface did not contain a deleted member's token)
  2  BLOCKED — the instance or operator token was unavailable; nothing was measured

Run:  WEBUI_TOKEN=<operator token> python3 multi/verify/test_session_revocation.py
      [--json]   machine-readable verdict on stdout, no secrets
"""
import json
import os
import sys
import urllib.error
import urllib.request

from common import DEFAULT_API, Checks, delete_user, mint_member, request

BOGUS_RESPONSE_ID = "resp_00000000000000000000000000000000"
PROBE_PATH = "/v1/responses/" + BOGUS_RESPONSE_ID

# The member route used for the "is this bearer accepted?" question. Chosen because it is
# cheap, read-only, and member-reachable; see the module docstring for how its codes read.
ACCEPTED, REJECTED, UNKNOWN = "ACCEPTED", "REJECTED", "UNKNOWN"


def _status(path, token, api=None, method="GET", body=None, timeout=20):
    """HTTP status for a bearer against a path, without raising. Discards the body on purpose:
    a body could carry material we do not want in a log or a JSON artifact.

    `common.request` is the non-raising helper written for exactly this — negative proofs where
    the status IS the result. Its `[0]` is the whole answer here. The copies this replaces also
    sent a bare `Mozilla/5.0` rather than `common.BROWSER_UA`. This docstring used to justify
    that with common.py's claim that the header was load-bearing because "the instance shapes
    some responses by it" — measured false (`responses.BROWSER_UA`): the instance answers
    identically to every agent. The header decides whether a request survives a HOSTED
    instance's edge, so the bare copies were a portability difference, not a measurement one."""
    return request(method, path, token, body=body, timeout=timeout, api=api)[0]


def bearer_verdict(token, api=None):
    """ACCEPTED / REJECTED / UNKNOWN for one bearer against the product surface.

    401 and 403 are BOTH read as REJECTED: 403 means the identity resolved but the route
    refused it, which for a member on a member route is still a closed door. Anything else
    that is not a transport failure means the gateway let the bearer through."""
    code = _status(PROBE_PATH, token, api)
    if code == 0:
        return UNKNOWN, code
    if code in (401, 403):
        return REJECTED, code
    return ACCEPTED, code


def logout_route_mounted(api=None):
    """Is POST /auth/logout mounted? Probed with NO bearer, so nothing is revoked either way:
    the handler returns 204 before touching the store when there is no Authorization header."""
    req = urllib.request.Request((api or DEFAULT_API) + "/auth/logout", method="POST", data=b"")
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status != 404, r.status
    except urllib.error.HTTPError as e:
        return e.code != 404, e.code
    except OSError:
        return None, 0


def _admin(path, op_token, api=None, method="POST", body=None):
    """The operator-token twin of `_status`. Same helper, same headers, status only."""
    return _status(path, op_token, api=api, method=method, body=body, timeout=30)


def main(argv):
    as_json = "--json" in argv
    api = DEFAULT_API
    op = os.environ.get("WEBUI_TOKEN") or os.environ.get("IRONCLAW_OPERATOR_TOKEN") or ""
    report = {"api_reachable": None, "pin_probe": "session-revocation", "legs": {}}

    if not op:
        msg = "WEBUI_TOKEN (or IRONCLAW_OPERATOR_TOKEN) is not set — nothing measured"
        if as_json:
            print(json.dumps({**report, "verdict": "BLOCKED", "why": msg}))
        else:
            print(f"  [~] BLOCKED: {msg}")
        return 2

    c = Checks()
    print(f"== session revocation probe against {api} ==")

    uid = None
    try:
        try:
            tok, uid = mint_member("ironworks revocation probe", op, api=api)
        except OSError as e:
            msg = f"instance unreachable or operator token refused ({type(e).__name__})"
            if as_json:
                print(json.dumps({**report, "verdict": "BLOCKED", "why": msg}))
            else:
                print(f"  [~] BLOCKED: {msg}")
            return 2
        report["api_reachable"] = True

        # A. the probe is meaningful — BOTH directions. The positive control alone would let
        # a route that answers 404 for everything read as "authority remains" forever; the
        # negative control is what makes ACCEPTED mean accepted.
        nv, ncode = bearer_verdict("not-a-real-token-" + "0" * 16, api)
        report["legs"]["negative_control"] = {"verdict": nv, "http": ncode}
        c.check("a GARBAGE bearer is REJECTED on the probe route (negative control)",
                nv == REJECTED, f"got {nv} (HTTP {ncode}) — the route does not gate on the bearer, "
                                "so ACCEPTED would prove nothing")
        v, code = bearer_verdict(tok, api)
        report["legs"]["fresh_member"] = {"verdict": v, "http": code}
        c.check("a fresh member token is ACCEPTED on the product surface (positive control)",
                v == ACCEPTED, f"got {v} (HTTP {code}) — the probe route cannot distinguish auth")
        if v != ACCEPTED or nv != REJECTED:
            # Without a meaningful baseline every later verdict is noise.
            c.finish()

        # B. suspension
        st = _admin(f"/api/webchat/v2/admin/users/{uid}/status", op, api,
                    body={"status": "suspended"})
        sv, scode = bearer_verdict(tok, api)
        report["legs"]["after_suspend"] = {"verdict": sv, "http": scode, "set_status_http": st}
        print(f"  ..  POST admin/users/<id>/status suspended -> HTTP {st}; "
              f"member bearer now {sv} (HTTP {scode})")
        c.check("suspension is an available admin operation (HTTP 2xx)", 200 <= st < 300,
                f"HTTP {st} — suspension is not usable as a containment step on this rev")

        # C. deletion
        dcode = delete_user(uid, op, api=api)
        uid = None                      # handled; do not double-delete in `finally`
        dv, dstatus = bearer_verdict(tok, api)
        report["legs"]["after_delete"] = {"verdict": dv, "http": dstatus, "delete_http": dcode}
        print(f"  ..  DELETE admin/users/<id> -> HTTP {dcode}; "
              f"member bearer now {dv} (HTTP {dstatus})")

        # The one thing that MUST hold whatever upstream does about sessions: a deleted
        # member's token must not reach the admin surface. That is our operator-privilege
        # containment, and upstream asserts it too (admin_api_e2e.rs).
        adm = _status("/api/webchat/v2/admin/users", tok, api)
        report["legs"]["deleted_on_admin_surface"] = {"http": adm}
        c.check("a DELETED member's token is refused on the admin surface",
                adm in (401, 403), f"HTTP {adm} — a deleted member reached the admin API")

        # D. is there any revocation route at all?
        mounted, lcode = logout_route_mounted(api)
        report["legs"]["logout_route"] = {"mounted": mounted, "http": lcode}
        print(f"  ..  POST /auth/logout (no bearer) -> HTTP {lcode}; "
              f"session-revocation route {'MOUNTED' if mounted else 'NOT mounted'}")

        residual = dv == ACCEPTED
        report["verdict"] = ("RESIDUAL_AUTHORITY" if residual else
                             "VERIFIED_REVOKED" if dv == REJECTED else "BLOCKED")
        report["residual_authority"] = residual
        report["revocation_route_available"] = bool(mounted)

        print()
        if residual:
            print("VERDICT: RESIDUAL AUTHORITY — a deleted member's token still authenticates the")
            print("  product surface. This matches the pinned runtime's documented behaviour")
            print("  (SECURITY.md). Containment is token CUSTODY plus the global")
            print("  rotation in deploy/README.md — not this DELETE.")
        elif dv == REJECTED:
            print("VERDICT: VERIFIED REVOKED — the token is refused after deletion.")
            print("  This is BETTER than the recorded behaviour. Re-read the pinned source and")
            print("  update SECURITY.md and deprovision.sh's exit")
            print("  contract before relying on it.")
        else:
            print("VERDICT: BLOCKED — the probe could not reach the instance after deletion.")

        if as_json:
            print(json.dumps(report))

        if not c.ok:
            return 1
        if report["verdict"] == "VERIFIED_REVOKED":
            return 0
        if report["verdict"] == "BLOCKED":
            return 2
        return 3
    finally:
        if uid:                        # only on an exception path; normal flow cleared it
            delete_user(uid, op, api=api)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
