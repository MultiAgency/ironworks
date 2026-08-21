#!/usr/bin/env bash
# confine-existing.sh — re-confine EVERY already-provisioned client.
#
# provision.sh confines each client as it is minted, which only covers clients created after
# that wiring landed; anyone minted earlier still carries builtin.http at always_allow. Run this
# once on each instance that has a client registry, and after any pin bump (the tool taxonomy
# can change), then keep the printed record.
#
# Iterates CLIENTS_DIR/*.env (the registry = the source of truth for who is a client), confines
# each member with ITS OWN token via confine-member.sh, and FAILS CLOSED: a single client that
# cannot be certified confined aborts the run non-zero (so the record can never say "all done"
# while one client is still open). Idempotent — safe to re-run; already-confined clients re-verify.
#
# Usage (run on the host that holds the client registry):
#   IRONCLAW_API=http://127.0.0.1:3020 ./confine-existing.sh
#   IRONCLAW_API=... CLIENTS_DIR=~/.agency/clients ./confine-existing.sh   # explicit registry
set -euo pipefail
. "$(dirname "$0")/../../deploy/lib/fleet.sh"   # fleet_env_get: ONE env-file quoting rule
cd "$(dirname "$0")"
API="${IRONCLAW_API:?set IRONCLAW_API (the multi-tenant instance base URL)}"
CLIENTS_DIR="${CLIENTS_DIR:-$HOME/.agency/clients}"
[ -d "$CLIENTS_DIR" ] || { echo "!! no client registry at $CLIENTS_DIR" >&2; exit 1; }

shopt -s nullglob
envs=("$CLIENTS_DIR"/*.env)
[ "${#envs[@]}" -gt 0 ] || { echo "!! no *.env clients in $CLIENTS_DIR — nothing to confine" >&2; exit 1; }

stamp="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "== confine-existing @ $stamp — $API — ${#envs[@]} client(s) in $CLIENTS_DIR =="
done_ok=0
for f in "${envs[@]}"; do
  slug="$(basename "$f" .env)"
  # fleet_env_get tolerates both quote styles and padding, and returns empty (not non-zero) for
  # an absent key — so the check below stays the handler and names the offending client instead
  # of the script dying silently mid-back-fill under pipefail.
  tok="$(fleet_env_get "$f" IRONCLAW_TOKEN)"
  if [ -z "$tok" ]; then
    echo "  !! $slug: no IRONCLAW_TOKEN in $f — cannot confine (fail closed)" >&2
    exit 1
  fi
  echo "-- $slug --"
  if ! IRONCLAW_API="$API" IRONCLAW_MEMBER_TOKEN="$tok" ./confine-member.sh; then
    echo "  !! $slug: confinement FAILED — aborting the back-fill ($slug is still un-confined)" >&2
    exit 1
  fi
  done_ok=$((done_ok + 1))
done
echo "== confine-existing COMPLETE @ $stamp — $done_ok/${#envs[@]} client(s) confined & probed clean =="
echo "   record this line in the pilot/readiness log; re-run after any ironclaw pin bump."
