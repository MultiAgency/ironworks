"""The control leg of the egress probe: which forbidden destinations can be measured at all.

Run by `probe-egress.sh` in a container with DEFAULT networking — no gateway, no internal-only
network, full outbound access — immediately before the real probe runs the same list from inside
the boundary.

WHY THIS EXISTS. Every forbidden leg asserts a negative: "the runtime cannot reach this." A
negative is satisfied just as well by a destination that could never have been reached from
anywhere, and three entries in `forbidden-destinations.json` were exactly that — two `127.0.0.1`
legs that addressed the runtime's own loopback rather than the docker host, and a
`host.docker.internal` leg that failed at DNS because that name is not injected into a
`--network container:` run. All three returned "blocked" against a container with completely
unrestricted egress, and the probe counted them into the assertion total it writes into the
VERIFIED stamp.

MEASURABLE IS NOT THE SAME AS REACHABLE, and using the wrong one here breaks the probe in the
opposite direction. Almost nothing in the forbidden list has a listener: `10.0.0.1:8443` and
`172.16.0.1:80` name ranges, not services, and an uncontained container reaches them with a
timeout rather than a handshake. Requiring a successful connection would mark the whole list
unmeasurable on every ordinary host.

The distinction that matters is whether THE ADDRESS EXISTED TO BE TRIED:

  measurable    connected, or refused, or timed out — the packet had somewhere to go, so a
                containment failure at this address WOULD have shown up as a connection.
  unmeasurable  the name did not resolve, or the network/host was unreachable. The attempt died
                before the boundary was consulted, so its failure from inside proves nothing.

That is exactly the line the three broken legs fell on the wrong side of, and it is the line a
contained runtime falls on the wrong side of on purpose — inside the boundary those same
addresses raise ENETUNREACH, which is the containment being observed rather than a defect.

Stdout contract — two PREFIX-ANCHORED lines, in this order:

    REACHABLE_FROM_UNCONTAINED=<host:port>[,<host:port>...]
    DISCRIMINATING=<host:port>[,<host:port>...]

Both are stated because the caller reads by prefix (`probe-egress.sh` uses
`sed -n 's/^…//p' | tail -1`), not by position. This block used to call the first one "the last
line the caller parses" while `main()` printed the second one after it — harmless only by
accident, and the accident was that no caller ever did what the contract described.

Empty is a legitimate answer and produces an empty value rather than a missing line, so the
caller can tell "nothing is measurable" from "the control did not run".
"""
import errno
import json
import os
import socket

TIMEOUT = 6

# An attempt that dies here died BEFORE the boundary could have had any say: no name, or no
# route even with the whole internet available. A leg like that measures nothing anywhere.
UNROUTABLE = frozenset({errno.ENETUNREACH, errno.EHOSTUNREACH, errno.EAFNOSUPPORT})

DISCRIMINATING, CORROBORATING, UNMEASURABLE = "discriminating", "corroborating", "unmeasurable"


def classify(host, port):
    """(verdict, why) — what a "blocked" result from inside the boundary would be worth here.

    THREE OUTCOMES, NOT TWO, AND THE MIDDLE ONE IS THE WHOLE POINT. The contained probe scores a
    forbidden leg by connected-vs-not, so a leg only tells contained from uncontained when an
    UNCONTAINED container actually CONNECTS:

      discriminating  the handshake completed with full egress. Contained, the same attempt must
                      fail — so its failure is evidence, and this is the only outcome that is.
      corroborating   routable, but nothing answered: a timeout, or an active refusal. The
                      attempt fails the same way with the boundary present and absent, so the
                      leg is consistent with containment and proves none of it.
      unmeasurable    the name did not resolve, or there was no route at all.

    An earlier version of this file merged the first two under "measurable", reasoning that the
    packet "had somewhere to go, so a containment failure WOULD have shown up as a connection".
    That is false for a timeout and for a refusal, and measurably so: run the contained probe
    body against a container on the default bridge with no gateway and five of nine forbidden
    legs score PASS. They were then counted into EGRESS_CHECKS_PASSED and recorded in the
    verification stamp as "assertions proved" — a number that included legs proving nothing,
    which is the defect the stamp's own count was introduced to remove.

    Corroborating legs are still asserted. They just are not evidence, so they are not counted,
    and the run refuses to stamp unless at least one forbidden leg discriminates.
    """
    try:
        socket.create_connection((host, int(port)), timeout=TIMEOUT).close()
        return DISCRIMINATING, "connected with full egress — contained, it must not"
    except socket.gaierror as e:
        return UNMEASURABLE, "does not resolve (%s)" % (e.strerror or "gaierror")
    except socket.timeout:
        return CORROBORATING, "routed, no answer (timeout) — fails the same way either way"
    except OSError as e:
        if e.errno in UNROUTABLE:
            return UNMEASURABLE, "no route from an UNCONTAINED container (%s)" % (
                errno.errorcode.get(e.errno, e.errno))
        return CORROBORATING, "refused (%s) — fails the same way either way" % (
            errno.errorcode.get(e.errno, type(e).__name__))


MARK = {DISCRIMINATING: "x", CORROBORATING: "-", UNMEASURABLE: "~"}


def main():
    destinations = json.loads(os.environ["FORBIDDEN_JSON"])["destinations"]
    attempted, proving = [], []
    for d in destinations:
        host, port = d["host"], d["port"]
        verdict, why = classify(host, port)
        pair = "%s:%s" % (host, port)
        if verdict != UNMEASURABLE:
            attempted.append(pair)
        if verdict == DISCRIMINATING:
            proving.append(pair)
        print("  [%s] %-48s %s: %s" % (
            MARK[verdict], "%s (%s:%s)" % (d["label"], host, port), verdict.upper(), why))
    print("  %d of %d forbidden destination(s) can PROVE containment from this host; "
          "%d corroborate; %d cannot be measured at all."
          % (len(proving), len(destinations), len(attempted) - len(proving),
             len(destinations) - len(attempted)))
    print("REACHABLE_FROM_UNCONTAINED=" + ",".join(attempted))
    print("DISCRIMINATING=" + ",".join(proving))


if __name__ == "__main__":
    main()
