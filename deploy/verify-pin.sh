#!/usr/bin/env bash
# verify-pin.sh — assert a running ironclaw container was built at IRONCLAW_PIN.
#
# Compose deploys the floating tag `ironclaw:main`, so nothing otherwise guarantees the running
# binary matches IRONCLAW_PIN — provenance would be inferred, not checked. UPGRADE.md builds with
# `--label ironclaw.rev=<full SHA>`; this turns that label into an enforced check. Run it after
# any bump.
#
# For each container: compares its image's `ironclaw.rev` label to IRONCLAW_PIN's SHA.
#   MATCH      -> deployed rev == pin (ok)
#   MISMATCH   -> deployed rev != pin (FAIL — the running binary is not the pinned one)
#   UNLABELED  -> image predates the label rule (FAIL — provenance unverifiable; rebuild per UPGRADE.md)
# Exits non-zero if ANY checked container is not a verified MATCH, so it can gate a deploy.
#
# Usage:
#   ./deploy/verify-pin.sh                         # checks the MT instance (MT_CONTAINER or default)
#   ./deploy/verify-pin.sh multiclaw ironclaw-hq   # explicit container list
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
PIN_FILE="$REPO/IRONCLAW_PIN"
[ -f "$PIN_FILE" ] || { echo "!! no IRONCLAW_PIN at $PIN_FILE" >&2; exit 1; }
PIN="$(cut -d' ' -f1 "$PIN_FILE")"
[ -n "$PIN" ] || { echo "!! IRONCLAW_PIN is empty" >&2; exit 1; }

# The MT container name is derived, never hardcoded (the laptop/VM rename thrash this avoids).
# fleet_mt_container owns the precedence and the "hard error on a missing compose" rule — a
# provenance gate must NEVER silently check the wrong container. It honours MT_CONTAINER itself.
. "$REPO/deploy/lib/fleet.sh"

if [ "$#" -gt 0 ]; then
  containers=("$@")
else
  containers=("$(fleet_mt_container)")
fi

echo "== verify-pin: IRONCLAW_PIN = $PIN =="
fail=0
for c in "${containers[@]}"; do
  # ONE inspect for both fields: a missing container makes this call fail, which is exactly the
  # existence check the separate probe used to do in an extra daemon round-trip.
  if ! meta="$(docker inspect -f '{{.Config.Image}}{{"\n"}}{{index .Config.Labels "ironclaw.rev"}}' "$c" 2>/dev/null)"; then
    echo "  [FAIL] $c — not running / not found"; fail=1; continue
  fi
  img="${meta%%$'\n'*}"; rev="${meta#*$'\n'}"
  if [ -z "$rev" ] || [ "$rev" = "<no value>" ]; then
    echo "  [FAIL] $c ($img) — UNLABELED: image predates the ironclaw.rev rule; provenance unverifiable. Rebuild per UPGRADE.md."
    fail=1
  elif [ "$rev" = "$PIN" ]; then
    echo "  [ ok ] $c ($img) — rev matches pin"
  else
    echo "  [FAIL] $c ($img) — MISMATCH: image rev $rev != pin $PIN (running binary is not the pinned one)"
    fail=1
  fi
done
if [ "$fail" -ne 0 ]; then
  echo "!! verify-pin: at least one container's provenance is NOT verified against IRONCLAW_PIN" >&2
  exit 1
fi
echo "   verified: every checked container was built at IRONCLAW_PIN"
