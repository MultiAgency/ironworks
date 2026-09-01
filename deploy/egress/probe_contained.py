"""The CONTAINED leg of the egress probe: what a boundaried runtime can and cannot reach.

Runs inside the runtime's own network namespace (`docker run --network container:<runtime>`) and
reports three sets, which are NOT the same thing:

  results        assertions that hold AND could have failed here — the evidence
  corroborating  assertions that hold but fail identically with no boundary at all
  unmeasured     assertions that could not be attempted meaningfully from anywhere

`EGRESS_CHECKS_PASSED` on the last line counts ONLY the first set, and that count is what
`egress_status.write_stamp` records as the boundary's verification. A corroborating leg counted
there would stamp VERIFIED on evidence an empty network also produces.

`probe_attempts.py` next door is the CONTROL leg — the same destinations from an uncontained
container — and it computes which of them are measurable at all. This file consumes that answer
through MEASURABLE/DISCRIMINATING.

WHY THIS IS A FILE. It used to be 157 lines inside `python3 -c '...'` in probe-egress.sh:
unlintable, uncollectable by any test runner, invisible to `ruff` and to the whitespace gate, and
one apostrophe away from a shell parse error in the block that computes the stamped count. The
control leg in the same script was already a file, read into an env var and `exec`d — so the
script demonstrated the right pattern beside the wrong one. This is that pattern.

Env in: PROVIDER_HOST, PROVIDER_PORT, EGRESS_PROXY, FORBIDDEN_JSON, MEASURABLE, DISCRIMINATING,
MODE. Exit 0 when every measurable assertion held.
"""
import json, os, socket, sys

TIMEOUT = 6
results = []          # assertions that hold AND could have failed here — the evidence
corroborating = []    # assertions that hold but fail the same way with no boundary at all
unmeasured = []       # assertions that could not be attempted meaningfully from anywhere


def record(label, got, ok, proves=True):
    """`proves=False` keeps the assertion and withholds it from the EVIDENCE count.

    The count is written into the verification stamp and read back as "assertions proved", so a
    leg that would have passed against a container with unrestricted egress must not be in it.
    """
    if ok and not proves:
        corroborating.append(label)
        print("  [-] %-48s %s (corroborating; cannot discriminate)" % (label, got))
        return
    results.append((ok, label, got))
    print("  [%s] %-48s %s" % ("x" if ok else "!", label, got))


def direct(label, host, port, must, proves=True):
    """A raw TCP attempt from inside the target namespace. No proxy, no DNS tricks.

    A NAME THAT DOES NOT RESOLVE COUNTS AS BLOCKED, and that is not a loophole — having no
    resolver path out is PART of the boundary, which is why the stack proof asserts it
    separately ("DNS cannot be abused to bypass the destination policy"). Measured against the
    live runtime: inside `multi_inner` even `example.com` fails to resolve, so treating
    resolution failure as unmeasured marked five correct assertions BLOCKED and refused to stamp
    a boundary that was working.

    What makes a leg EVIDENCE is not how it failed here; it is whether the same attempt SUCCEEDS
    with the boundary removed. Only the control run can answer that, and `proves` carries its
    answer in.
    """
    try:
        s = socket.create_connection((host, int(port)), timeout=TIMEOUT)
        s.close()
        got = "REACHED"
    except Exception as e:
        got = "blocked (%s)" % type(e).__name__
    record(label, got, (got == "REACHED") == (must == "reach"), proves)


def via_proxy(label, proxy, host, port, must):
    """A CONNECT through the gateway. `must` is "allow" (expect 200) or "deny" (expect a
    refusal status). A gateway that cannot be reached at all is a FAILURE either way: it means
    the boundary is in place but the product cannot work through it."""
    phost, _, pport = proxy.rpartition(":")
    try:
        s = socket.create_connection((phost, int(pport)), timeout=TIMEOUT)
    except Exception as e:
        record(label, "gateway unreachable (%s)" % type(e).__name__, False)
        return
    try:
        s.settimeout(TIMEOUT)
        s.sendall(("CONNECT %s:%s HTTP/1.1\r\nHost: %s:%s\r\n\r\n"
                   % (host, port, host, port)).encode())
        head = s.recv(256).decode("latin-1", "replace").split("\r\n", 1)[0]
    except Exception as e:
        record(label, "no gateway response (%s)" % type(e).__name__, False)
        return
    finally:
        try:
            s.close()
        except OSError:
            pass
    allowed = " 200 " in head + " "
    got = "gateway said: %s" % head.strip()
    record(label, got, allowed == (must == "allow"))


P_HOST, P_PORT = os.environ["PROVIDER_HOST"], os.environ["PROVIDER_PORT"]
PROXY = os.environ.get("EGRESS_PROXY", "").strip()

if PROXY:
    # 1. the ONE destination the product needs, through the ONE way out.
    via_proxy("required provider via gateway (%s:%s)" % (P_HOST, P_PORT), PROXY,
              P_HOST, P_PORT, "allow")
    # 1b. the allowlist is a real decision, not an artifact of what happens to resolve.
    via_proxy("gateway REFUSES a non-allowlisted host", PROXY, "example.com", 443, "deny")
    # 1c. and it refuses a lookalike, because the match is exact host:port, never a suffix.
    via_proxy("gateway REFUSES a lookalike hostname", PROXY,
              P_HOST + ".attacker.example", 443, "deny")
    # 2. ...and the provider is NOT reachable directly, or the gateway is decoration.
    direct("provider is NOT reachable bypassing the gateway", P_HOST, P_PORT, "block")
else:
    direct("required model provider (%s:%s)" % (P_HOST, P_PORT), P_HOST, P_PORT, "reach")

# These hold in both modes: nothing but the gateway may be reachable from the namespace. The
# list comes from forbidden-destinations.json, which proof_checks.py reads too — so the probe
# that STAMPS the verification can no longer assert less than the one that does not.
#
# WHAT THE CONTROL RUN DECIDES, IN TWO STEPS — because a forbidden leg has three states and not
# two. `MEASURABLE` is every destination an uncontained container could attempt at all; anything
# outside it never reached the boundary from anywhere and is UNMEASURED. `DISCRIMINATING` is the
# subset where the uncontained container COMPLETED A HANDSHAKE, which is the only case where this
# leg failing here proves the boundary did it.
#
# The difference is not academic: run this same body against a container on the default bridge
# with no gateway and the timeout/refused legs — cloud metadata, link-local, 10/8, 172.16/12, the
# account database — all score PASS with no boundary whatsoever. They are still asserted, because
# a REACHED there would be a genuine failure; they are not counted, because their success is not
# evidence.
MEASURABLE = {p for p in os.environ.get("MEASURABLE", "").split(",") if p}
DISCRIMINATING = {p for p in os.environ.get("DISCRIMINATING", "").split(",") if p}
for d in json.loads(os.environ["FORBIDDEN_JSON"])["destinations"]:
    label = "%s (%s:%s)" % (d["label"], d["host"], d["port"])
    pair = "%s:%s" % (d["host"], d["port"])
    if pair not in MEASURABLE:
        unmeasured.append(label)
        print("  [~] %-48s UNMEASURED (no vantage reaches it at all)" % label)
        continue
    direct(label, d["host"], d["port"], "block", proves=pair in DISCRIMINATING)

bad = [r for r in results if not r[0]]
print()
if bad:
    print("FAIL %d/%d assertion(s) did not hold:" % (len(bad), len(results)))
    for _, label, got in bad:
        print("  - %s: %s" % (label, got))
    sys.exit(1)
if unmeasured:
    # NOT A PASS AND NOT A FAILURE. Nothing observed is wrong; something claimed was never
    # observed. Exit 3 is the BLOCKED verdict this repository uses everywhere else — a
    # guarantee is unevaluated — and the caller refuses to stamp on it.
    print("BLOCKED %d assertion(s) could not be MEASURED on this host:" % len(unmeasured))
    for label in unmeasured:
        print("  - %s" % label)
    print("  Each is unreachable from an uncontained container too, so its failure to connect")
    print("  from inside the boundary says nothing about the boundary. Fix the destination in")
    print("  deploy/egress/forbidden-destinations.json, or stand up what it names.")
    sys.exit(3)
if not DISCRIMINATING:
    # A run in which NO forbidden destination answers an uncontained container has measured the
    # gateway and nothing else. Every forbidden leg would have passed with the boundary removed,
    # so there is no evidence here to stamp — the same "could not evaluate" this file treats as
    # non-zero everywhere else. Typical cause: a host whose own outbound is firewalled, where
    # every destination times out from every vantage.
    print("BLOCKED no forbidden destination answered an UNCONTAINED container, so not one of")
    print("  them can tell this boundary from its absence. The gateway legs above still hold;")
    print("  the containment claim is unproved on this host and will not be stamped.")
    sys.exit(3)
if corroborating:
    print("%d assertion(s) held but cannot discriminate (they fail the same way with no" %
          len(corroborating))
    print("  boundary at all) and are NOT counted as evidence:")
    for label in corroborating:
        print("  - %s" % label)
    print()
print("PASS %d/%d — the runtime reaches the model provider and nothing else." % (
    len(results), len(results)))
# EVIDENCE ONLY. This becomes `checks_passed` in the verification stamp, which an operator reads
# back as "assertions proved", so a leg that would also have passed with unrestricted egress
# must not be in it.
print("EGRESS_CHECKS_PASSED=%d" % len(results))
