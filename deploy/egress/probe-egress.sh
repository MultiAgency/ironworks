#!/usr/bin/env bash
# probe-egress.sh — the black-box assertions behind the egress-containment claim.
#
# Run INSIDE the network namespace the IronClaw runtime uses, so what it measures is what the
# runtime can reach — not what a tool is allowed to ask for. That distinction is the whole
# point: `multi/verify/test_egress_closed.py` proves a confined MEMBER makes no network call,
# which is a statement about the model's surface. This proves the CONTAINER cannot reach the
# network at all, which survives a tool being re-enabled, a token being mishandled, and a pin
# bump that renames the tool taxonomy.
#
#   ./probe-egress.sh <container>                    # default: the MT container from compose
#   ./probe-egress.sh --network <net>                # a bare network namespace instead
#   EGRESS_PROXY=egress:3128 ./probe-egress.sh ...   # gateway mode: the provider is reached
#                                                    # through the allowlisting CONNECT proxy
#
# WITHOUT `EGRESS_PROXY` this measures the CURRENT state — including, today, a runtime with
# unrestricted outbound access, which is exactly what it should report. WITH it, it measures
# the design in SECURITY.md and additionally proves the allowlist itself:
# a non-allowlisted destination must be refused BY THE GATEWAY, not merely unreachable.
#
# Exit 0 only when every assertion holds. Anything else is non-zero, including "could not run"
# — an unevaluated boundary is never reported as a present one.
set -euo pipefail
. "$(dirname "$0")/../lib/fleet.sh"

PROVIDER_HOST="${PROVIDER_HOST:-cloud-api.near.ai}"
PROVIDER_PORT="${PROVIDER_PORT:-443}"
PROBE_IMAGE="${PROBE_IMAGE:-multiagency-data-account-service:latest}"   # python3, already local
EGRESS_PROXY="${EGRESS_PROXY:-}"

# TWO WAYS TO NAME THE TARGET, AND THEY MUST NOT BOTH WIN. `--network` set the kind AND the
# name; a positional then overwrote the name and left the kind, so
# `probe-egress.sh --network multi_inner multiclaw` probed a NETWORK CALLED multiclaw — a name
# that does not exist, silently measuring nothing that was asked for. Whichever form is used, a
# second target is a usage error and says so.
TARGET_KIND="container"
TARGET=""
TARGET_SET=""
while [ $# -gt 0 ]; do
  case "$1" in
    --network)
      # `${2-}`, not `$2`: with `set -u` a trailing `--network` after another target would abort
      # with exit 1 from the unbound reference before this message could name the real mistake.
      [ -z "$TARGET_SET" ] || {
        echo "!! two targets given: '$TARGET' ($TARGET_KIND) and network '${2-<missing>}'." >&2
        echo "   Name exactly one container, or one --network." >&2; exit 64; }
      TARGET_KIND="network"; TARGET="${2:?--network needs a name}"; TARGET_SET=1; shift ;;
    -*) echo "!! unknown flag: $1" >&2; exit 2 ;;
    *)
      [ -z "$TARGET_SET" ] || {
        echo "!! two targets given: '$TARGET' ($TARGET_KIND) and '$1'." >&2
        echo "   Name exactly one container, or one --network." >&2; exit 64; }
      TARGET="$1"; TARGET_SET=1 ;;
  esac
  shift
done
if [ "$TARGET_KIND" = container ] && [ -z "$TARGET" ]; then
  TARGET="$(fleet_mt_container)"
fi

case "$TARGET_KIND" in
  container) NETNS=(--network "container:$TARGET") ;;
  network)   NETNS=(--network "$TARGET") ;;
esac

# AUTO-DETECT THE GATEWAY from the target's own environment. A contained runtime reaches its
# provider THROUGH the proxy, so probing it in direct mode asserts the opposite of the truth
# and reports a working boundary as broken. Found the hard way, on the first live activation:
# `egress-control.sh verify` ran direct mode against a correctly contained runtime and failed
# leg 1. Reading the container's own HTTPS_PROXY means the probe cannot be run in the wrong
# mode by accident — the runtime's configuration decides.
if [ -z "$EGRESS_PROXY" ] && [ "$TARGET_KIND" = container ]; then
  detected="$(docker inspect "$TARGET" -f '{{range .Config.Env}}{{println .}}{{end}}' 2>/dev/null \
              | sed -n 's|^HTTPS_PROXY=http://||p' | head -1 || true)"
  if [ -n "$detected" ]; then
    EGRESS_PROXY="$detected"
    echo "   (detected the runtime's own HTTPS_PROXY: $EGRESS_PROXY — probing in gateway mode)"
  fi
fi

echo "== egress containment probe — $TARGET_KIND '$TARGET' =="
echo "   required destination: $PROVIDER_HOST:$PROVIDER_PORT"
echo "   mode: $([ -n "$EGRESS_PROXY" ] && echo "gateway via $EGRESS_PROXY" || echo "direct (no gateway configured)")"
echo

# One python process makes every connection attempt, so the legs share a timeout, a resolver
# and a failure vocabulary. `--network container:<name>` joins the target's namespace exactly,
# which is what makes this a probe OF the runtime rather than of something beside it.
probe_rc=0
PROBE_OUT="$(mktemp)"
trap 'rm -f "$PROBE_OUT"' EXIT
# The forbidden list is passed IN rather than written here: this probe carried four
# destinations where deploy/egress/proof/proof_checks.py carried ten, and this is the probe whose
# PASS writes the VERIFIED stamp. See forbidden-destinations.json for what that asymmetry meant.
FORBIDDEN_JSON="$(cat "$(dirname "$0")/forbidden-destinations.json")"

# `host.docker.internal` is a docker convenience name, not DNS, and on Linux it does not resolve
# inside a container unless something maps it. The CONTROL run below is an ordinary bridge
# container, so it takes the mapping as a flag.
#
# THE CONTAINED RUN CANNOT. `docker run --network container:<target> --add-host …` is refused
# outright — "conflicting options: custom host-to-IP mapping and the network mode" — because the
# joining container shares the target's networking, hosts file included. So the runtime is where
# the name has to be mapped, and `docker-compose.egress.yml` maps it on the `ironclaw` service.
# A runtime that predates that overlay change simply cannot resolve the name, which is why the
# contained legs below treat a RESOLUTION failure as unmeasured rather than as containment.
HOSTMAP=(--add-host "host.docker.internal:host-gateway")

# ── THE POSITIVE CONTROL ──────────────────────────────────────────────────────────────────────
# A NEGATIVE CHECK NEEDS ONE. Every forbidden leg asserts "this cannot be reached", which an
# empty network satisfies just as well as a working boundary — three legs were unreachable for
# reasons that had nothing to do with containment and passed on every host anyway. So attempt
# the same list from an UNCONTAINED container first: default bridge, no gateway, full egress.
# Whatever is unreachable THERE cannot be measured from inside the boundary, and a leg that
# cannot be measured is not a leg that passed.
echo "== control: the same destinations from an UNCONTAINED container =="
CONTROL_OUT="$(mktemp)"
trap 'rm -f "$PROBE_OUT" "$CONTROL_OUT"' EXIT
control_rc=0
# The source travels as ENV DATA, not inside the `-c` argument. `-c "$(cat file)"` puts the whole
# script through a double-quoted bash expansion, where a future `$` or backslash in it would be
# eaten silently — and the failure would look like a python bug in a file that reads correctly.
# A volume mount would avoid that too, but needs Docker Desktop file sharing that nothing else
# in this probe requires.
PROBE_SRC="$(cat "$(dirname "$0")/probe_attempts.py")"
docker run --rm "${HOSTMAP[@]}" -e "FORBIDDEN_JSON=$FORBIDDEN_JSON" \
  -e "PROBE_SRC=$PROBE_SRC" \
  "$PROBE_IMAGE" python3 -c 'import os; exec(os.environ["PROBE_SRC"])' \
  > "$CONTROL_OUT" 2>&1 || control_rc=$?
cat "$CONTROL_OUT"
if [ "$control_rc" -ne 0 ]; then
  echo "!! the control run could not be made, so no forbidden leg below can be believed." >&2
  echo "   Without it, 'unreachable' and 'contained' are the same observation." >&2
  exit 3
fi
MEASURABLE="$(sed -n 's/^REACHABLE_FROM_UNCONTAINED=//p' "$CONTROL_OUT" | tail -1 || true)"
# The legs that can PROVE containment: an uncontained container completed a handshake there, so
# the same attempt failing from inside is evidence. A leg that merely timed out or was refused
# fails identically with the boundary present and absent — asserted, but never counted.
DISCRIMINATING="$(sed -n 's/^DISCRIMINATING=//p' "$CONTROL_OUT" | tail -1 || true)"
echo

# NO "${HOSTMAP[@]}" HERE — see above; docker refuses it alongside `--network container:`.
docker run --rm "${NETNS[@]}" \
  -e "PROVIDER_HOST=$PROVIDER_HOST" -e "PROVIDER_PORT=$PROVIDER_PORT" \
  -e "EGRESS_PROXY=$EGRESS_PROXY" -e "FORBIDDEN_JSON=$FORBIDDEN_JSON" \
  -e "MEASURABLE=$MEASURABLE" -e "DISCRIMINATING=$DISCRIMINATING" -e "MODE=contained" \
  "$PROBE_IMAGE" python3 -c '
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
' | tee "$PROBE_OUT" || probe_rc=$?

# A PASSING probe against a CONTAINER stamps the verification, bound to that container's image
# id. `ironworks doctor` reads the stamp, and a rebuild or a pin bump invalidates it
# automatically — an inherited VERIFIED is the most dangerous kind of stale.
if [ "$probe_rc" -eq 0 ] && [ "$TARGET_KIND" = container ]; then
  IMAGE_ID="$(docker inspect -f '{{.Image}}' "$TARGET" 2>/dev/null || true)"

  # THE ALLOWLIST COMES FROM THE GATEWAY, not from this shell. `${EGRESS_ALLOW:-...}` recorded
  # whatever the OPERATOR's environment happened to hold, which is not the list the proxy under
  # test enforces and is not read by any leg above. Adding a host to the deployed gateway's
  # EGRESS_ALLOW therefore changed nothing the probe measured: legs 1/1b/1c still passed, and
  # the stamp still named the provider alone. Read the running gateway's own environment, and
  # if it cannot be read, DO NOT STAMP — an unverifiable allowlist is not a verified one.
  GW="${EGRESS_GATEWAY:-multi-egress-1}"
  ALLOWLIST="$(docker inspect "$GW" -f '{{range .Config.Env}}{{println .}}{{end}}' 2>/dev/null \
               | sed -n 's/^EGRESS_ALLOW=//p' | head -1 || true)"
  # The count the probe actually computed. This was the literal `0`, so every stamp on every
  # host recorded "checks_passed": 0 beside a VERIFIED state.
  CHECKS="$(sed -n 's/^EGRESS_CHECKS_PASSED=//p' "$PROBE_OUT" | tail -1)"

  if [ -z "$ALLOWLIST" ] || [ -z "$CHECKS" ]; then
    echo "!! probe passed but the verification could NOT be stamped:" >&2
    [ -z "$ALLOWLIST" ] && echo "   could not read EGRESS_ALLOW from gateway '$GW'" >&2
    [ -z "$CHECKS" ] && echo "   the probe reported no assertion count" >&2
    echo "   A stamp names what was proved; it cannot name a list nothing read." >&2
    exit 3
  fi

  # The GATEWAY's image too, not just the runtime's: the proxy is half of what was proved, and a
  # stamp that survives replacing it certifies a boundary nobody tested.
  GW_IMAGE="$(docker inspect -f '{{.Image}}' "$GW" 2>/dev/null || true)"
  LIB_DIR="$(cd "$(dirname "$0")/../lib" && pwd)" TARGET="$TARGET" IMAGE_ID="$IMAGE_ID" \
  ALLOWLIST="$ALLOWLIST" CHECKS="$CHECKS" GW_IMAGE="$GW_IMAGE" python3 -c "
import os, sys
sys.path.insert(0, os.environ['LIB_DIR'])
import egress_status as es
d = es.write_stamp(os.environ['TARGET'], os.environ['IMAGE_ID'], os.environ['ALLOWLIST'],
                   os.environ['CHECKS'], os.environ.get('GW_IMAGE') or None)
print('   verification stamped at ' + d['at_iso'] + ' for image ' + (d['image_id'] or '?')[:19])
print('   allowlist as enforced by the gateway: ' + d['allow'])
print('   assertions proved: %d' % d['checks_passed'])
print('   proof fingerprint: ' + (d['proof_fingerprint'] or '?')[:16]
      + ' (contract %d)' % d['proof_contract'])"
fi
exit "$probe_rc"
