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
# The contained leg travels the same way and for the same reason. It was 157 lines inlined in
# a `python3 -c '...'` here — the block that computes the count written into the verification
# stamp, and the one part of this probe no linter or test runner could see.
CONTAINED_SRC="$(cat "$(dirname "$0")/probe_contained.py")"
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
  -e "PROBE_SRC=$CONTAINED_SRC" \
  "$PROBE_IMAGE" python3 -c 'import os; exec(os.environ["PROBE_SRC"])' \
  | tee "$PROBE_OUT" || probe_rc=$?

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
