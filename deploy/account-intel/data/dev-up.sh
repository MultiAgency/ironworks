#!/usr/bin/env bash
# Reproducible dev/demo bring-up of the Account Store:
#   postgres + read-only Account Service (identity-implies-org) + seed
#   (regression fixtures AND sales-loop candidates).
# DEV identities are injected HERE (not committed as live compose config): well-known demo
# tokens exist only on stacks brought up by this dev script. Production uses prod-up.sh,
# where the only identities are per-client minted tokens in the hot-reloaded file.
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh    # fleet_wait_health (+ curl_header/fleet_json, which smoke.sh uses)
. ./smoke.sh            # smoke_code/smoke_matches — the checks below must be able to FAIL

# PROD GUARD: this script injects well-known demo tokens. On a host where real client
# identities are registered they would become live org credentials — refuse unless the
# operator explicitly overrides with ALLOW_DEV=1.
# `identities.load()`, not inline JSON: that module is the one reader and the one writer of this
# file, and it alone distinguishes an ABSENT map from a CORRUPT one — the distinction this guard
# depends on, since a map that cannot be read must fail closed rather than read as empty.
# Interpolating the path into Python source is also the shape deprovision.sh stopped using.
IDF="$FLEET_AGENCY_DIR/account-identities/identities.json"
if [ "${ALLOW_DEV:-}" != 1 ] && [ -s "$IDF" ] && \
   [ "$(PYTHONPATH="$FLEET_REPO_ROOT/deploy/lib" ACCOUNT_IDENTITIES_FILE="$IDF" \
        python3 -c 'import identities; print(len(identities.load()))' 2>/dev/null || echo 1)" != 0 ]; then
  echo "!! $IDF is non-empty — this host already has registered org identities (which ones is" >&2
  echo "   not checked: the gate fails closed on any). If any are real clients, demo tokens here" >&2
  echo "   would be live credentials on a production store. Confirm, then set ALLOW_DEV=1 to override." >&2
  exit 1
fi

# Strong DB password, same file prod uses (minted on first bring-up; never a compose default).
# NOTE: a volume initdb'd on the old mia_local_pw default keeps that password — for dev,
# `docker volume rm multiagency-data_account-db-data` and re-run (dev data is reseedable).
[ -f "$FLEET_AGENCY_DIR/account-db.env" ] || { umask 077
  echo "ACCOUNT_DB_PASSWORD=$(openssl rand -hex 24)" > "$FLEET_AGENCY_DIR/account-db.env"
  echo "minted ~/.agency/account-db.env (first bring-up)"; }
set -a; . "$FLEET_AGENCY_DIR/account-db.env"; set +a   # compose interpolation needs it exported

# WHY THESE ARE LITERAL, AND WHY THAT IS SAFE — read before "fixing" them to random values.
# Three things contain them, and the third is the reason they must NOT be randomised:
#   1. The service binds 127.0.0.1 only (docker-compose.yml), so these reach nothing off-box.
#   2. The PROD GUARD above refuses to run on any host that has a registered org identity, so
#      they cannot be injected onto a store that holds real client data.
#   3. prod-up.sh ASSERTS — `smoke_code "demo token /list_accounts" 401` — that
#      `mia_sales_token` gets a 401 in production, and aborts the prod bring-up if it does not.
#      That check means something ONLY because the value is well-known: it proves the DEV
#      credential specifically is dead there. Mint these randomly and that assertion decays
#      into "some arbitrary string 401s", which its no-token neighbour already covers.
# So a well-known value here buys a real cross-environment guarantee, and costs nothing that
# (1) and (2) do not already close.
export ACCOUNT_DEV_IDENTITIES='{"mia_sales_token":"multiagency-sales","acme_token":"acme-sales","rival_token":"rival-sales"}'

echo "== ensuring the shared external network exists =="
docker network create multiagency-data >/dev/null 2>&1 && echo "  created" || echo "  already present"

echo "== migrating account-db before account-service starts =="
./migrate.sh apply
docker compose up -d account-service
echo "== waiting for account-service health =="
fleet_wait_health http://127.0.0.1:8443/health
fleet_wait_health http://127.0.0.1:8443/ready

echo "== seeding =="
docker compose exec -T account-service python /app/deploy/account-intel/data/seed.py
# demo candidates via the one parameterized seeder
docker compose exec -T \
  -e REAL_DATA_DIR=/app/deploy/account-intel/data/candidates \
  -e SALES_ORG=multiagency-sales -e SALES_ORG_NAME="MultiAgency Sales" \
  account-service python /app/deploy/account-intel/data/seed_real.py

# ISOLATION IS THE CLAIM THIS STACK MAKES, so each line below aborts the bring-up on mismatch
# rather than printing a number nobody reads. The own-org check is asserted too, and that is not
# decoration: on an empty database every one of the three zeros below passes, so without it the
# isolation result is indistinguishable from a seed that never ran.
#
# 1, not "some": `candidates/northwind.json` is the only seeded name matching 'northwind'.
echo "== smoke: identity implies org + cross-org isolation =="
smoke_matches "sales token finds 'northwind' (own org)" 1 "X-Service-Token: mia_sales_token" northwind # gitleaks:allow — synthetic dev token
smoke_matches "rival token does NOT see 'northwind'" 0 "X-Service-Token: rival_token" northwind # gitleaks:allow — synthetic dev token
smoke_matches "rival token cannot assert another org via X-Org-Id" 0 \
  "X-Service-Token: rival_token" northwind -H "X-Org-Id: multiagency-sales" # gitleaks:allow — synthetic dev token
smoke_code "unknown token /find_account" 401 "X-Service-Token: bogus" "/find_account?query=northwind"
echo "== account store up + seeded =="
