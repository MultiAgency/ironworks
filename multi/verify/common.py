"""Shared /v1/responses helpers for the verify scripts — one copy instead of four.

post() targets the local MT instance (:3020) by default; pass api= for another target
(e.g. test_instr_live.py's live-instance check).
"""
import atexit
import json
import os
import pathlib
import urllib.error
import urllib.request

DEFAULT_API = os.environ.get("IRONCLAW_API", "http://127.0.0.1:3020").rstrip("/")


def model_pin():
    """The model of record, read once from the repo-root MODEL_PIN. `MODEL` env still wins.

    NO FALLBACK LITERAL, deliberately. MODEL_PIN is tracked, so it is absent only in a broken
    checkout — and a proof that quietly runs on a different model than production is worse than
    one that refuses to start.
    """
    env = os.environ.get("MODEL")
    if env:
        return env
    p = pathlib.Path(__file__).resolve().parents[2] / "MODEL_PIN"
    try:
        pin = p.read_text().split("#", 1)[0].strip()
    except OSError as e:
        raise SystemExit(f"!! cannot read {p} ({e}) — MODEL_PIN is tracked; set MODEL to override")
    if not pin:
        raise SystemExit(f"!! {p} has no model on its first line")
    return pin


def post(path, body, token, api=None, timeout=90):
    req = urllib.request.Request((api or DEFAULT_API) + path, data=json.dumps(body).encode(),
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json",
                 "User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
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


def text_of(resp):
    out = []
    for item in resp.get("output", []) or []:
        for c in item.get("content", []) or []:
            if isinstance(c, dict) and c.get("text"):
                out.append(c["text"])
    if not out and resp.get("output_text"):
        out.append(resp["output_text"])
    return "\n".join(out).strip()


def get(path, token, api=None, timeout=30):
    """GET a JSON endpoint with a bearer token. The read-side twin of post().

    Five proofs hand-rolled this. The divergence that mattered: only three of the five sent
    the browser User-Agent that post() treats as load-bearing, so a proof could be shaped by
    which helper its author happened to copy.
    """
    req = urllib.request.Request((api or DEFAULT_API) + path,
        headers={"Authorization": "Bearer " + token, "User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
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
