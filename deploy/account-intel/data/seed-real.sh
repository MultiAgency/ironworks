#!/usr/bin/env bash
# Go-live on-ramp: load REAL account data into the Account Store, in a distinct REAL org,
# from JSON kept OUTSIDE the repo. Never commits customer data; the demo org/candidates are
# left untouched (so the demos keep working).
#
# You provide, OUTSIDE the repo (matching the ~/.agency secrets convention):
#   ~/.agency/account-data/<slug>/*.json   — one account per file (candidate shape, FACTS only:
#                                             record_id, account{...}, contacts[], activities[])
#   ~/.agency/account-data/<slug>.env      — REAL_ORG_ID, REAL_ORG_NAME, REAL_ACCOUNT_TOKEN (secret)
#
# Then:  ./seed-real.sh <slug>
#
# Result: a real org in the store + a real token->org identity; the agent still reaches NONE
# of it directly (brokered, read-only, org-scoped). To go live, point the bridge's
# ACCOUNT_TOKEN at REAL_ACCOUNT_TOKEN (printed at the end) and restart telegram_bridge.
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh    # curl_header (org token off argv) + fleet_json/fleet_require_container
SLUG="${1:?usage: seed-real.sh <slug>  (real data under ~/.agency/account-data/<slug>/)}"
# Validate before $SLUG reaches a path or an in-container `sh -c "rm -rf /tmp/real-$SLUG"`:
# an unconstrained slug allows `../` traversal and shell-metacharacter injection into the
# container. provision.sh guards its own slug; seed-real.sh is documented for standalone use too.
case "$SLUG" in *[!a-z0-9-]*|'') echo "!! slug must be lowercase [a-z0-9-]: $SLUG" >&2; exit 1;; esac
DATA_DIR="$HOME/.agency/account-data/$SLUG"
CFG="$HOME/.agency/account-data/$SLUG.env"
CONT="${ACCOUNT_SERVICE_CONTAINER:-multiagency-data-account-service-1}"

[ -d "$DATA_DIR" ] || { echo "!! no data dir: $DATA_DIR (put real *.json there, OUTSIDE the repo)"; exit 1; }
ls "$DATA_DIR"/*.json >/dev/null 2>&1 || { echo "!! no *.json in $DATA_DIR"; exit 1; }
[ -f "$CFG" ] || { echo "!! no config: $CFG  (set REAL_ORG_ID, REAL_ORG_NAME, REAL_ACCOUNT_TOKEN)"; exit 1; }
# plain source (no set -a): REAL_* stay shell variables; the seeder gets its inputs
# explicitly via `docker compose exec -e` below, never by wholesale env inheritance
. "$CFG"
: "${REAL_ORG_ID:?set REAL_ORG_ID in $CFG}"
: "${REAL_ACCOUNT_TOKEN:?set REAL_ACCOUNT_TOKEN in $CFG}"
REAL_ORG_NAME="${REAL_ORG_NAME:-$REAL_ORG_ID}"
fleet_require_container "$CONT" || { echo "   run ./dev-up.sh first" >&2; exit 1; }

echo "== 1. register the real token->org (hot-reloaded identity file — no restart) =="
ORG_TOKEN="$REAL_ACCOUNT_TOKEN" ./register-identity.sh "$REAL_ORG_ID"

echo "== 2. copy real JSON into the container and seed into org '$REAL_ORG_ID' =="
docker exec "$CONT" sh -c "rm -rf /tmp/real-$SLUG && mkdir -p /tmp/real-$SLUG"
docker cp "$DATA_DIR/." "$CONT:/tmp/real-$SLUG/"
docker compose exec -T \
  -e REAL_DATA_DIR="/tmp/real-$SLUG" -e SALES_ORG="$REAL_ORG_ID" -e SALES_ORG_NAME="$REAL_ORG_NAME" \
  account-service python /app/deploy/account-intel/data/seed_real.py
docker exec "$CONT" sh -c "rm -rf /tmp/real-$SLUG"   # do not leave customer data in the container fs

echo "== 3. smoke: real token sees its org; the demo token does NOT (isolation) =="
BASE=http://127.0.0.1:8443
first="$(find "$DATA_DIR" -maxdepth 1 -name '*.json' | sort | head -1)"
qname="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['account']['name'].split()[0])" "$first")"
echo -n "   real token finds '$qname' (own org): "
curl_header "X-Service-Token: $REAL_ACCOUNT_TOKEN" -s "$BASE/find_account?query=$qname" | fleet_json "d.get('match_count', d.get('error'))"
echo -n "   unknown token -> 401 (fail closed): "
curl -s -o /dev/null -w '%{http_code}\n' -H "X-Service-Token: not-a-real-token" "$BASE/find_account?query=$qname" # gitleaks:allow — literal placeholder, a negative test (401), not a secret

echo
echo "DONE. Real org '$REAL_ORG_ID' is seeded and org-scoped. Its token lives in"
# shellcheck disable=SC2088 # display text for the operator — the literal ~/ path is intended
echo "~/.agency/account-identities/identities.json (hot-reloaded; survives restarts and dev-up.sh)."
echo "If this org is a bridge client, its ~/.agency/clients/<slug>.env carries the same token;"
echo "restart telegram_bridge.py only when registry entries change."
