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
. ./smoke.sh            # smoke_matches/smoke_code — the checks below must be able to FAIL
# `set -a` AND REQUIRED, exactly as prod-up.sh and dev-up.sh load it: the `docker compose`
# invocations below interpolate POSTGRES_PASSWORD and ACCOUNT_DB_DSN from the ENVIRONMENT, so
# ACCOUNT_DB_PASSWORD has to be exported rather than merely set as a shell variable. (Contrast
# the plain `. "$CFG"` further down, whose REAL_* values are passed explicitly with `-e` and
# must NOT leak into every child.)
#
# THIS LINE WAS MISSING, and the shape of the failure is why it lasted. Run by hand it worked,
# because the operator had usually just run prod-up.sh in the same shell and inherited the
# export. Called from provision.sh step 2 it inherits nothing, so compose refused with
# "required variable ACCOUNT_DB_PASSWORD is missing a value" — AFTER step 1 had already minted
# and registered an org token, which then had to be compensated and re-verified revoked.
# Measured on the serve host 2026-09-01: a missing `set -a` turned a data-seeding step into a
# half-provisioned tenant.
[ -f "$FLEET_AGENCY_DIR/account-db.env" ] || {
  echo "!! no $FLEET_AGENCY_DIR/account-db.env — the DB password lives there; run prod-up.sh" >&2
  exit 1; }
set -a; . "$FLEET_AGENCY_DIR/account-db.env"; set +a
SLUG="${1:?usage: seed-real.sh <slug>  (real data under ~/.agency/account-data/<slug>/)}"
# Validate before $SLUG reaches a path or an in-container `sh -c "rm -rf /tmp/real-$SLUG"`:
# an unconstrained slug allows `../` traversal and shell-metacharacter injection into the
# container. provision.sh guards its own slug; seed-real.sh is documented for standalone use too.
fleet_slug_valid "$SLUG" || { echo "!! slug must be lowercase [a-z0-9-]: $SLUG" >&2; exit 1; }
DATA_DIR="$FLEET_AGENCY_DIR/account-data/$SLUG"
CFG="$FLEET_AGENCY_DIR/account-data/$SLUG.env"
CONT="$(fleet_account_service_container)"   # the same resolver provision.sh uses; this line and
                                            # its twin there used to be byte-identical literals

[ -d "$DATA_DIR" ] || { echo "!! no data dir: $DATA_DIR (put real *.json there, OUTSIDE the repo)" >&2; exit 1; }
ls "$DATA_DIR"/*.json >/dev/null 2>&1 || { echo "!! no *.json in $DATA_DIR" >&2; exit 1; }
[ -f "$CFG" ] || { echo "!! no config: $CFG  (set REAL_ORG_ID, REAL_ORG_NAME, REAL_ACCOUNT_TOKEN)" >&2; exit 1; }
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

# GO-LIVE IS THE POINT OF NO RETURN for this data, so these two abort the run on mismatch under
# the `set -euo pipefail` above rather than printing a number: if the real token cannot read its
# own org, or the store is not fail-closed to an unknown one, the operator must not be told the
# org is "seeded and org-scoped" and pointed at the bridge.
#
# The own-org count is asserted as ">= 1" rather than an exact number — the query is the first
# word of the first account's name, and how many of THIS operator's accounts share it is not
# knowable here (the dev fixtures can pin an exact count; real client data cannot).
echo "== 3. smoke: real token sees its org; an unknown token does NOT (isolation) =="
first="$(find "$DATA_DIR" -maxdepth 1 -name '*.json' | sort | head -1)"
qname="$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['account']['name'].split()[0])" "$first")"
smoke_matches "real token finds '$qname' (own org)" '>=1' "X-Service-Token: $REAL_ACCOUNT_TOKEN" "$qname"
smoke_code "unknown token /find_account" 401 "X-Service-Token: not-a-real-token" "/find_account?query=$qname" # gitleaks:allow — literal placeholder, a negative test (401), not a secret

echo
echo "DONE. Real org '$REAL_ORG_ID' is seeded and org-scoped. Its token lives in"
# shellcheck disable=SC2088 # display text for the operator — the literal ~/ path is intended
echo "~/.agency/account-identities/identities.json (hot-reloaded; survives restarts and dev-up.sh)."
echo "If this org is a bridge client, its ~/.agency/clients/<slug>.env carries the same token;"
echo "restart telegram_bridge.py only when registry entries change."
