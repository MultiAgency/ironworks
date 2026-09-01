#!/usr/bin/env bash
# run-proof.sh — the disposable exact-path proof for network-level egress containment.
#
# THE QUESTION. The per-bearer tool disable is not a network boundary, and the container it
# protects can currently reach the whole internet (measured). The proposed boundary — the
# runtime on an `internal: true` network with one allowlisting CONNECT gateway — passed a
# mechanism prototype 8/8, but that prototype never ran IronClaw. So one thing has never been
# established: does the PINNED RUNTIME actually reach its model provider through it?
#
# This answers that with the real image, a real model turn, and a real allowlist, in a stack
# that shares nothing with the live one. Then it attacks the boundary.
#
#   ./deploy/egress/proof/run-proof.sh              full proof, then tear down
#   ./deploy/egress/proof/run-proof.sh --service-path  also drive the WHOLE IronWorks path
#   ./deploy/egress/proof/run-proof.sh --aide       also run the front desk's discovery suite
#   ./deploy/egress/proof/run-proof.sh --keep       leave it up for manual poking
#   ./deploy/egress/proof/run-proof.sh --down       tear down a kept stack
#
# It NEVER touches the live stack: distinct compose project, containers, volumes, port and
# freshly minted secrets. It refuses to run if its port or project name collide.
set -euo pipefail
cd "$(dirname "$0")"
HERE="$(pwd)"
REPO="$(cd ../../.. && pwd)"
# fleet_ironclaw_pin owns the pin parse; this script tagged its image from a hand-rolled
# `cut -d' '` that is not the parse rule the rest of the fleet uses.
. "$REPO/deploy/lib/fleet.sh"
PROJECT=egressproof
PORT="${PROOF_PORT:-3999}"
COMPOSE=(docker compose -f "$HERE/docker-compose.proof.yml")

teardown() {
  echo "== tearing down the disposable stack =="
  "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--down" ]; then teardown; exit 0; fi
# Every flag, not just $1 — `--keep --service-path` silently dropped the second one, and an
# unknown flag was ignored rather than refused. Same shape as probe-egress.sh next door.
KEEP=0 SERVICE_PATH=0 AIDE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1 ;;
    --service-path) SERVICE_PATH=1 ;;
    --aide) AIDE=1 ;;
    *) echo "!! unknown argument: $1 (usage: $0 [--keep] [--service-path] [--aide] | --down)" >&2
       exit 2 ;;
  esac
  shift
done

# ── guardrails: this must never be able to disturb the live environment ────────────────
if docker ps --format '{{.Names}}' | grep -q "^${PROJECT}-"; then
  echo "!! a previous proof stack is still up — run: $0 --down" >&2; exit 1
fi
# `fleet_first_free_port`, not `lsof`: the old `if lsof …; then` treated "lsof is not installed"
# (exit 127) exactly like "the port is free", so on a host without it — `ubuntu-latest`, which
# `.github/workflows/scheduled-integration.yml` runs this on — the header's claim that the script
# "refuses to run if its port or project name collide" held for the project name only.
if ! fleet_first_free_port "$PORT" >/dev/null; then
  echo "!! port $PORT is in use; set PROOF_PORT to something free" >&2; exit 1
fi

# ── disposable identities; the provider key is the one thing we cannot mint ────────────
# fleet_env_get, not a local sed: fleet.sh is sourced above and its header records why one
# quoting rule for the fleet matters — a stricter reader elsewhere rejected a padded value and
# SKIPPED the account deletion it was gating. The copy here tolerated no leading whitespace.
NEARAI_API_KEY="${NEARAI_API_KEY:-$(fleet_env_get "$REPO/multi/instance/.env" NEARAI_API_KEY)}"
[ -n "$NEARAI_API_KEY" ] || { echo "!! NEARAI_API_KEY not set and not readable from multi/instance/.env" >&2; exit 1; }
export NEARAI_API_KEY
PROOF_PGPW="$(openssl rand -hex 24)"
PROOF_MASTER_KEY="$(openssl rand -hex 32)"
PROOF_WEBUI_TOKEN="$(openssl rand -hex 32)"
PIN_SHORT="$(fleet_ironclaw_pin | cut -c1-9)"
PROOF_IMAGE="${PROOF_IMAGE:-ironclaw:$PIN_SHORT}"
export PROOF_PGPW PROOF_MASTER_KEY PROOF_WEBUI_TOKEN PROOF_IMAGE
export PROOF_PORT="$PORT"
API="http://127.0.0.1:$PORT"

# shellcheck disable=SC2154  # `rc` is set by the trap body itself, at fire time
trap 'rc=$?; [ "$KEEP" -eq 1 ] || teardown; exit $rc' EXIT

echo "== disposable egress proof =="
echo "   image   : $PROOF_IMAGE   (IRONCLAW_PIN $PIN_SHORT)"
echo "   api     : $API"
echo "   allow   : ${PROOF_EGRESS_ALLOW:-cloud-api.near.ai:443}"
echo

"${COMPOSE[@]}" up -d --wait --wait-timeout 180 db gw ing >/dev/null
"${COMPOSE[@]}" up -d ic >/dev/null
echo "   waiting for the contained runtime to answer..."
ready=0
for _ in $(seq 1 60); do
  if curl -sf -m 3 "$API/api/health" >/dev/null 2>&1; then ready=1; break; fi
  sleep 3
done
if [ "$ready" -ne 1 ]; then
  echo "!! the contained runtime never became healthy — this is a RESULT, not a script failure." >&2
  echo "   IronClaw logs (last 40):" >&2
  "${COMPOSE[@]}" logs --tail=40 ic >&2 || true
  echo "   gateway decisions:" >&2
  "${COMPOSE[@]}" logs --tail=40 gw >&2 || true
  exit 1
fi
echo "   healthy."
echo

# ── the proof itself, in python so every leg shares one vocabulary ─────────────────────
# `|| rc=$?` rather than a bare call: under `set -e` a failing proof_checks.py aborted the script
# here, which skipped the gateway decision log below — the one output that says WHICH destination
# the runtime asked for, i.e. the diagnostic you want on precisely the run that failed. The
# capture also makes the `[ "$rc" -eq 0 ]` guard and the final `exit $rc` mean something; before
# this, both could only ever see 0 and the script's failure came entirely from the EXIT trap.
rc=0
PROOF_API="$API" PROOF_PROJECT="$PROJECT" PROOF_COMPOSE="$HERE/docker-compose.proof.yml" \
PROOF_OPERATOR="$PROOF_WEBUI_TOKEN" REPO="$REPO" \
  python3 "$HERE/proof_checks.py" || rc=$?

# Step 8: the whole product path, not just the runtime. Only worth running once the raw model
# proof passes — a service-path failure under a broken boundary would say nothing.
if [ "$SERVICE_PATH" -eq 1 ] && [ "$rc" -eq 0 ]; then
  echo
  echo "== SERVICE PATH: bridge -> seam -> IronClaw -> gateway -> provider =="
  PROOF_API="$API" PROOF_OPERATOR="$PROOF_WEBUI_TOKEN" REPO="$REPO" \
    python3 "$HERE/service_path_checks.py" || rc=$?
fi

# Step 9: THE FRONT DESK. `deploy/secretary/test_aide_discovery.py` needs an MT instance plus an
# operator token to mint a throwaway account — exactly what this stack already stands up. It had
# no scheduled home anywhere and had never been run in any form, recorded as needing a new CI
# secret. It does not: NEARAI_API_KEY is exported above and the webui token is minted per run,
# so the only thing missing was the invocation.
#
# BEHIND ITS OWN FLAG because the Secretary is a separate application with its own trust domain
# (README.md § "What else is in this repository"). Folding it into --service-path would quietly
# widen what that flag attests to.
if [ "$AIDE" -eq 1 ] && [ "$rc" -eq 0 ]; then
  echo
  echo "== FRONT DESK: aide discovery behaviour against a fresh sealed account =="
  AIDE_SUITE="$REPO/deploy/secretary/test_aide_discovery.py"
  if [ ! -f "$AIDE_SUITE" ]; then
    # A missing suite is not a passing one. Without this, --aide on a tree that had moved the
    # file would print the banner above and nothing else, and the run would stay green.
    echo "!! --aide requested but $AIDE_SUITE is missing — refusing to report a pass" >&2
    rc=1
  else
    ( cd "$REPO/deploy/secretary" \
      && IRONCLAW_API="$API" WEBUI_TOKEN="$PROOF_WEBUI_TOKEN" \
         python3 "$AIDE_SUITE" ) || rc=$?
  fi
fi

echo
echo "== gateway decision log (every destination the runtime actually asked for) =="
"${COMPOSE[@]}" logs gw 2>/dev/null | sed 's/^[^|]*| //' | sort | uniq -c | sort -rn | head -30

if [ "$KEEP" -eq 1 ]; then
  echo
  echo "stack left UP (--keep). Tear down with: $0 --down"
  trap - EXIT
fi
exit $rc
