"""Shared /v1/responses helpers for the verify scripts — one copy instead of four.

post() targets the local MT instance (:3020) by default; pass api= for another target
(e.g. test_instr_live.py's live-instance check).

IMPORT PATHS, and the two rules that decide what a proof needs to write.

  A SIBLING IN THIS DIRECTORY NEEDS NOTHING. `python3 multi/verify/x.py` puts this directory
  on `sys.path` before the file runs, and so does pytest's basedir insertion, so
  `sys.path.insert(0, <this directory>)` has never done anything. Eight proofs carried that
  line; every one was a no-op and they are gone. Do not add it back.

  A SEAM MODULE NEEDS ONE LINE — `sys.path.insert` of `multi/seam`, written whichever way the
  file already spells its own root. Importing `common` happens to do that too, as a side effect
  of the `import pins` below. Do not lean on that: it makes the order of two unrelated imports
  load-bearing, which is the same class of silent coupling the `os.environ.setdefault` cleanup
  removed from the seam. State what the file needs.
"""
import atexit
import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "seam"))
import pins
# The seam's reader under this suite's established name, and the seam's User-Agent, for the
# same reason `model_pin` below wraps `pins`: a proof must not read the response — or address
# the instance — a different way than production does. The reader copy this replaces walked
# EVERY content entry carrying a `text` key, so it also picked up the model's REASONING, and the
# UA copy it replaces was a bare "Mozilla/5.0" the product never sent, guarded by a comment
# asserting that the difference mattered. See responses.py for both measured divergences. The
# two injection proofs decide "did it refuse?" from this string.
from responses import BROWSER_UA, output_text as text_of  # noqa: F401

DEFAULT_API = os.environ.get("IRONCLAW_API", "http://127.0.0.1:3020").rstrip("/")


def model_pin():
    """The model of record — the seam's reader, so a proof cannot run on a different model
    than production reads. Raises rather than exiting, and this wrapper turns that into the
    SystemExit the proof scripts expect."""
    try:
        return pins.model_pin()
    except pins.PinError as e:
        raise SystemExit(f"!! {e}") from e


def _build(method, path, token, body=None, key=None, api=None):
    """One request object, so every proof sends the same headers."""
    headers = {"Authorization": "Bearer " + token, "User-Agent": BROWSER_UA}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    if key:
        headers["Idempotency-Key"] = key
    return urllib.request.Request((api or DEFAULT_API) + path, data=data,
                                  headers=headers, method=method)


def _parse(raw):
    """JSON if it is JSON, otherwise the decoded text.

    A streaming turn answers `text/event-stream`, and the SSE framing is itself the evidence
    that streaming survived. One of the copies this replaces called `json.loads` bare, so a
    stream body raised out of the helper instead of being returned."""
    try:
        return json.loads(raw)
    except ValueError:
        return raw.decode("utf-8", "replace")


def request(method, path, token, body=None, key=None, timeout=60, api=None):
    """`(status, parsed)` for any endpoint. NEVER raises — status 0 means it never left.

    THE NON-RAISING SHAPE, and the reason there are two. `post`/`get` below raise, because most
    proofs want an unexpected failure to stop the run. The negative proofs want the opposite:
    asserting that B gets 404 on A's project means the 404 IS the result, and a helper that
    raised would need every such assertion wrapped. Same split, same reasoning, as
    `pins.pin_value` vs `pins.require_pin`.

    Four copies of this existed — `proof_checks.api`, `test_responses_recovery._req`,
    `test_member_admin_negative.req`, and the pair below. They disagreed on three things: whether
    a non-JSON body was tolerated, whether the result was parsed or raw bytes, and what the
    transport-failure key was called (`transport` vs `transport_error`). Nothing asserted on the
    key, which is how they stayed different.
    """
    req = _build(method, path, token, body=body, key=key, api=api)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, _parse(r.read())
    except urllib.error.HTTPError as e:
        try:
            return e.code, _parse(e.read())
        except OSError:
            return e.code, None
    except OSError as e:
        return 0, {"transport": type(e).__name__}


def post(path, body, token, api=None, timeout=90):
    """POST and return the parsed body, RAISING on any non-2xx. See `request` for the other
    shape and why both exist."""
    with urllib.request.urlopen(_build("POST", path, token, body=body, api=api),
                                timeout=timeout) as r:
        return json.loads(r.read())


_MINTED = {}   # uid -> (op_token, api), drained by delete_user, swept at exit


def mint_member(display_name, op_token, api=None):
    """Mint a throwaway sealed member for a proof. Returns (token, user_id).

    THE OTHER HALF OF delete_user, AND THE REASON IT EXISTS. Cleanup was shared; minting was
    not — three proofs each hand-rolled the same POST plus the same comment explaining why the
    matching cleanup matters. That asymmetry is the bug shape itself: a proof that mints without
    going through a helper is exactly the proof that can forget to clean up, and an abandoned
    member is an un-confined member on the instance that serves clients (see delete_user).

    So minting here also REGISTERS the account for at-exit deletion. Callers should still delete
    explicitly in a `finally` — that is immediate rather than at-exit, and it reports per-proof —
    but the registration means forgetting is no longer possible. `delete_user` treats 404 as the
    desired end state, so the second delete of an already-deleted account is a no-op.
    """
    u = post("/api/webchat/v2/admin/users",
             {"display_name": display_name, "role": "member"}, op_token, api=api)
    token, uid = u["api_token"], u["user"]["user_id"]
    _MINTED[uid] = (op_token, api)
    return token, uid


@atexit.register
def _sweep_minted():
    """Backstop for anything mint_member created that a proof did not delete itself.

    delete_user discards its entry on a terminal outcome, so a proof that cleaned up correctly
    leaves nothing here and this prints nothing. Only a genuine leak is noisy, which is right.
    """
    for uid, (op_token, api) in list(_MINTED.items()):
        print(f"  sweep: {uid} was minted but never deleted by the proof —")
        delete_user(uid, op_token, api=api)


def delete_user(user_id, op_token, api=None):
    """Delete a throwaway sealed account: DELETE /api/webchat/v2/admin/users/<id>.

    WHY THE PROOFS MUST CALL THIS. Each injection/product proof MINTS a member every run.
    Left behind, each is a permanent account carrying the STOCK tool catalog — `builtin.http`
    at `always_allow` — on the instance that serves clients. Nothing else cleans them up:
    `confine-existing.sh` and `test_egress_closed.py` both iterate the CLIENT REGISTRY, and a
    throwaway member is not in it, so it is invisible to exactly the two tools meant to find
    un-confined members. Cleanup is the proof's own job, in a `finally`.

    HONEST LIMIT: deleting the account does NOT revoke the token already issued (signed
    session, in-memory revoked-set, no upstream session-revoke API — see deprovision.sh). What
    makes that acceptable here is custody — a proof's token never leaves the process. This is
    not evidence that deletion cuts access.

    Best-effort by design: returns the HTTP status, never raises. Cleanup must not be able to
    mask a real verdict by throwing out of a `finally`.
    """
    _MINTED.pop(user_id, None)     # handled — _sweep_minted must not delete it a second time
    req = urllib.request.Request(
        f"{(api or DEFAULT_API)}/api/webchat/v2/admin/users/{user_id}",
        method="DELETE", headers={"Authorization": "Bearer " + op_token})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            code = r.status
    except urllib.error.HTTPError as e:
        code = e.code
    except OSError as e:
        print(f"  cleanup: DELETE {user_id} failed to send ({e}) — REMOVE IT BY HAND")
        return 0
    ok = code in (200, 202, 204, 404)      # 404 = already gone, which is the desired end state
    print(f"  cleanup: DELETE admin/users/{user_id} -> HTTP {code}"
          + ("" if ok else "  ** LEFT AN UN-CONFINED MEMBER BEHIND — remove it by hand **"))
    return code




def get(path, token, api=None, timeout=30):
    """GET a JSON endpoint with a bearer token. The read-side twin of post().

    Five proofs hand-rolled this, and only three of the five sent the browser User-Agent. That
    divergence is real and worth recording; the reason first given for it was not. It said a
    proof "could be shaped by which helper its author happened to copy" — measurement (see
    `responses.BROWSER_UA`) showed the instance answers identically to every agent, including
    the python-urllib default. What the header actually decides is whether a request survives a
    hosted instance's edge, and `api=` can point any of these helpers at one. So the five copies
    were a portability difference, not a measurement difference — and one helper is still the
    fix, because the next divergence between five hand-rolled requests need not be this one.
    """
    with urllib.request.urlopen(_build("GET", path, token, api=api), timeout=timeout) as r:
        return json.loads(r.read())


class Checks:
    """The proof tick-list: one `check()`, one `block()`, one place they are counted.

    This collector existed ELEVEN times, and the copies had already drifted in ways that
    change what a run reports: test_two_clients appended the raw `ok` rather than `bool(ok)`,
    test_freshness_lifecycle printed a different separator and called the skip concept `skip`
    where four other files call it `block`. A proof suite whose scoreboard is copy-pasted can
    quietly disagree with itself about what counts as a pass.

    Deliberately NOT shared: each proof's final verdict line. Those are not duplication —
    test_adversarial_routing's names a specific finding and recommends a code change,
    test_adversarial_cross_org's names the leak class. Collapsing them into one format string
    would need a parameter carrying the whole sentence, which saves nothing and loses the
    diagnostic. Files that DO share the blocked-aware shape call finish() below.
    """

    def __init__(self):
        self.results = []
        self.blocked = []

    def check(self, label, ok, detail=""):
        self.results.append(bool(ok))
        print(f"  [{'x' if ok else ' '}] {label}" + (f" — {detail}" if detail and not ok else ""))
        return bool(ok)

    def block(self, label, why):
        """A leg that could not run. NOT a pass and NOT a failure — counted separately so a
        suite where everything was blocked cannot read as green."""
        self.blocked.append(label)
        print(f"  [~] BLOCKED: {label} — {why}")

    skip = block          # the same concept under the name test_freshness_lifecycle used

    @property
    def ran(self):
        return len(self.results)

    @property
    def passed(self):
        return sum(self.results)

    @property
    def ok(self):
        """`ran == 0` is NOT a pass: a suite that asserted nothing has proven nothing."""
        return all(self.results) if self.results else False

    def finish(self, tagline=""):
        """Print the score and exit — the shape shared by the blocked-aware proofs.

        Exit 2 when every leg was blocked, because "no assertions ran" must never be
        reported as success; 0 on a clean run, 1 on any failure.
        """
        print(f"\nscore: {self.passed}/{self.ran}"
              + (f", {len(self.blocked)} BLOCKED" if self.blocked else "")
              + (f" — {tagline}" if tagline and self.ok and self.ran else ""))
        if self.blocked and not self.results:
            print("ALL LEGS BLOCKED — no assertions ran; not a pass. Operator: run on the VM.")
            raise SystemExit(2)
        raise SystemExit(0 if self.ok else 1)
