#!/usr/bin/env python3
"""Can the bridge RECOVER a model turn it already paid for, instead of running a second one?

This is the measurement the durable-delivery design rests on. The bridge crashes — or is
restarted to pick up a new tenant — somewhere between "IronClaw ran the turn" and "the client
saw the answer". What it does next depends entirely on properties of the pinned runtime that
must be measured rather than assumed:

  Q1  does `GET /v1/responses/{id}` exist and answer?
  Q2  does a completed response survive the bridge restarting, and IronClaw restarting?
  Q3  does retrieval reconstruct the SAME output the create call returned, byte for byte,
      through the seam's own `_output_text` extraction?
  Q4  is retrieval scoped to the sealed member — can tenant B fetch tenant A's response by id?
  Q5  how long are completed responses retained?
  Q6  does response CREATION honour an idempotency key?
  Q7  does replaying the same key return the same response rather than running a second turn?
  Q8  if the bridge loses the HTTP response before persisting the id, can it still recover?

Q8 is the one that decides the design. If replaying a stored idempotency key returns the
original response, then the durable recovery handle is the KEY — which the bridge can persist
BEFORE the request — and not the response id, which only exists after a reply it may never
receive. A journal keyed on `{update_id: response_id}` cannot cover that window at all.

COST: this makes a handful of real model calls with a two-token reply. It mints nothing and
deletes nothing; it uses two already-provisioned proof tenants and their own credentials.

Run:  python3 multi/verify/test_responses_recovery.py [--restart-ironclaw]

`--restart-ironclaw` additionally restarts the MT container to settle Q2's second half. Only
pass it on a box where no bridge is serving clients — it interrupts every tenant for the length
of a container restart. The script refuses to do it if a bridge process is running.
"""
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "seam"))
import context_ingress as ing
from common import Checks, request

API = os.environ["IRONCLAW_API"].rstrip("/")
# A two-token reply keeps the measurement cheap. The nonce is what makes "the same response"
# checkable: a second RUN of the model would be free to word it differently, so identical
# output text carrying the same nonce is evidence of replay rather than re-execution.
PROMPT = "Reply with exactly this and nothing else: OK-{nonce}"


def _req(method, path, token, body=None, key=None, timeout=180):
    """The shared client, bound to this proof's API and default timeout. The local copy this
    replaces called `json.loads` bare, so a non-JSON body raised out of the helper."""
    return request(method, path, token, body=body, key=key, timeout=timeout, api=API)


def create(client, nonce, key=None):
    return _req("POST", "/v1/responses", client.ironclaw_token,
                {"model": client.model, "input": PROMPT.format(nonce=nonce)}, key=key)


def fetch(client, rid):
    return _req("GET", "/v1/responses/" + rid, client.ironclaw_token, timeout=30)


def bridge_running():
    """Is a bridge process alive? FAILS CLOSED — an unanswerable question is "yes".

    This matched `telegram_bridge.py` alone. `multi/serve/bridge.service` now launches the
    bridge as `python3 -u -m multi.seam.telegram_bridge`, so that substring is absent from the
    live cmdline and `pgrep` returned nothing: the `--restart-ironclaw` guard below saw no
    bridge and went on to `docker restart multiclaw`, interrupting every tenant's in-flight
    turn — the exact outcome the guard's block message says it refuses.

    Both spellings are matched, because a host may still be running the older unit, and
    `systemctl is-active` is asked as well: on the serve host that is the authority, and it
    does not depend on how the command line happens to be spelled.
    """
    for pattern in ("telegram_bridge.py", "multi.seam.telegram_bridge"):
        if subprocess.run(["pgrep", "-f", pattern],
                          capture_output=True, text=True).stdout.strip():
            return True
    try:
        active = subprocess.run(["systemctl", "is-active", "--quiet", "bridge"],
                                capture_output=True, text=True)
    except FileNotFoundError:
        return False                     # no systemd: pgrep was the whole answer
    return active.returncode == 0


def main(argv):
    do_restart = "--restart-ironclaw" in argv
    c = Checks()
    findings = {}

    try:
        clients = ing.load_clients()
    except Exception as e:
        print(f"  [~] BLOCKED: registry did not load ({type(e).__name__}) — nothing measured")
        return 2
    if len(clients) < 2:
        print("  [~] BLOCKED: need two provisioned tenants to test cross-tenant scoping")
        return 2
    a, b = [clients[s] for s in sorted(clients)][:2]
    print(f"== response recovery + idempotency, {API} ==")
    print(f"   tenant A = {a.slug}   tenant B = {b.slug}   model = {a.model}")

    # ── Q1 / Q3: create, then retrieve, and compare through the seam's own extractor ──
    nonce = "alpha1"
    st, created = create(a, nonce)
    if st != 200 or not created:
        print(f"  [~] BLOCKED: create returned HTTP {st} — the instance or model is unavailable")
        return 2
    rid = created.get("id")
    created_text = ing.output_text(created)
    findings["create"] = {"http": st, "has_id": bool(rid), "text_len": len(created_text)}

    st_g, got = fetch(a, rid)
    fetched_text = ing.output_text(got or {})
    c.check("Q1 GET /v1/responses/{id} answers for the owning tenant", st_g == 200,
            f"HTTP {st_g}")
    c.check("Q3 retrieval reconstructs the SAME output text the create call returned",
            bool(created_text) and fetched_text == created_text,
            f"create={created_text[:40]!r} fetch={fetched_text[:40]!r}")
    c.check("Q3 the retrieved response is terminal (status completed/absent)",
            (got or {}).get("status") in (None, "completed"),
            f"status={(got or {}).get('status')!r}")
    findings["retrieve_same_process"] = {"http": st_g, "text_matches": fetched_text == created_text}

    # ── Q4: cross-tenant. Knowing the id must not be enough. ──
    st_x, cross = fetch(b, rid)
    cross_text = ing.output_text(cross or {})
    c.check("Q4 tenant B CANNOT retrieve tenant A's response by id",
            st_x != 200 or (cross_text != created_text and not cross_text),
            f"HTTP {st_x}, text={cross_text[:60]!r} — a response id is not a capability")
    findings["cross_tenant"] = {"http": st_x, "leaked_text": bool(cross_text)}

    # ── Q6 / Q7 / Q8: idempotency — the recovery handle ──
    # The KEY is chosen by the caller BEFORE the request, which is the only thing the bridge can
    # durably record before it risks losing the reply. If replaying it returns the same response,
    # the bridge never has to guess whether a turn ran.
    key = "ironworks-recovery-probe-" + nonce + "-" + str(int(time.time()))
    n2 = "bravo2"
    st1, first = create(a, n2, key=key)
    st2, second = create(a, n2, key=key)
    id1, id2 = (first or {}).get("id"), (second or {}).get("id")
    t1, t2 = ing.output_text(first or {}), ing.output_text(second or {})
    same_id = bool(id1) and id1 == id2
    c.check("Q6/Q7 replaying an Idempotency-Key returns the SAME response id",
            same_id, f"first={id1!r} second={id2!r} (HTTP {st1}/{st2}) — "
                     "without this the bridge cannot recover a turn whose reply it lost")
    c.check("Q7 the replay returns identical output (it replayed, it did not re-run)",
            bool(t1) and t1 == t2, f"{t1[:40]!r} vs {t2[:40]!r}")
    findings["idempotency"] = {"same_id": same_id, "same_text": t1 == t2,
                              "http": [st1, st2], "distinct_ids": [id1, id2]}

    # Same key, DIFFERENT body: a fingerprint mismatch must not silently serve the old answer
    # to a different question. Whatever it does, the bridge must know — this is why the journal
    # stores the key alongside the update it belongs to and never reuses one across updates.
    st3, third = create(a, "charlie3", key=key)
    t3 = ing.output_text(third or {})
    findings["idempotency_body_mismatch"] = {
        "http": st3, "served_old_answer": bool(t1) and t3 == t1,
        "id": (third or {}).get("id")}
    print(f"  ..  Q7b same key + DIFFERENT body -> HTTP {st3}, "
          f"{'served the ORIGINAL answer' if t3 == t1 and t1 else 'did not serve the original'}")

    # ── Q2: survives a bridge restart (trivially — different process, same store) and an
    #        IronClaw restart (the half that actually needs the container to bounce). ──
    st_r, again = fetch(a, rid)
    c.check("Q2a a completed response is retrievable by a NEW process (bridge restart)",
            st_r == 200 and ing.output_text(again or {}) == created_text, f"HTTP {st_r}")

    if do_restart:
        if bridge_running():
            c.block("Q2b retrievable after an IronClaw restart",
                    "a bridge process is running on this host — refusing to restart the "
                    "container underneath live tenants")
        else:
            cont = subprocess.run(
                ["docker", "restart", os.environ.get("MT_CONTAINER", "multiclaw")],
                capture_output=True, text=True)
            if cont.returncode != 0:
                c.block("Q2b retrievable after an IronClaw restart",
                        f"docker restart failed: {cont.stderr.strip()[:120]}")
            else:
                for _ in range(40):                       # wait for the API to answer again
                    time.sleep(3)
                    try:
                        with urllib.request.urlopen(API + "/api/health", timeout=5) as h:
                            if h.status == 200:
                                break
                    except OSError:
                        continue
                st_p, post = fetch(a, rid)
                post_text = ing.output_text(post or {})
                c.check("Q2b the response survives an IronClaw restart",
                        st_p == 200 and post_text == created_text,
                        f"HTTP {st_p}, text={post_text[:40]!r}")
                findings["survives_ironclaw_restart"] = {"http": st_p,
                                                         "text_matches": post_text == created_text}
    else:
        c.block("Q2b retrievable after an IronClaw restart",
                "not attempted — pass --restart-ironclaw on a host with no serving bridge")

    # ── Q5: retention. Not measurable in one run; recorded as an open question rather than
    #        guessed, because a wrong retention assumption silently breaks recovery later. ──
    c.block("Q5 completed-response retention window",
            "not measurable in a single run. The bridge must therefore treat retrieval as "
            "FALLIBLE and degrade to RECOVERY_BLOCKED rather than assume availability")

    print()
    print("FINDINGS (json):", json.dumps(findings, sort_keys=True))
    c.finish("the recovery handle is available")


if __name__ == "__main__":
    main(sys.argv[1:])
