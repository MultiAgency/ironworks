#!/usr/bin/env bash
# Reproducible dev/demo bring-up of the Account Store:
#   postgres + read-only Account Service (identity-implies-org) + seed
#   (regression fixtures AND sales-loop candidates).
# DEV identities are injected HERE (not committed as live compose config): well-known demo
# tokens exist only on stacks brought up by this dev script. Production uses prod-up.sh,
# where the only identities are per-client minted tokens in the hot-reloaded file.
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh    # fleet_json + fleet_wait_health

# PROD GUARD: this script injects well-known demo tokens. On a host where real client
# identities are registered they would become live org credentials — refuse unless the
# operator explicitly overrides with ALLOW_DEV=1.
IDF="$HOME/.agency/account-identities/identities.json"
if [ "${ALLOW_DEV:-}" != 1 ] && [ -s "$IDF" ] && \
   [ "$(python3 -c "import json;print(len(json.load(open('$IDF'))))" 2>/dev/null || echo 1)" != 0 ]; then
  echo "!! $IDF is non-empty — this host already has registered org identities (which ones is"
  echo "   not checked: the gate fails closed on any). If any are real clients, demo tokens here"
  echo "   would be live credentials on a production store. Confirm, then set ALLOW_DEV=1 to override."
  exit 1
fi

# Strong DB password, same file prod uses (minted on first bring-up; never a compose default).
# NOTE: a volume initdb'd on the old mia_local_pw default keeps that password — for dev,
# `docker volume rm multiagency-data_account-db-data` and re-run (dev data is reseedable).
[ -f "$HOME/.agency/account-db.env" ] || { umask 077
  echo "ACCOUNT_DB_PASSWORD=$(openssl rand -hex 24)" > "$HOME/.agency/account-db.env"
  echo "minted ~/.agency/account-db.env (first bring-up)"; }
set -a; . "$HOME/.agency/account-db.env"; set +a   # compose interpolation needs it exported

export ACCOUNT_DEV_IDENTITIES='{"mia_sales_token":"multiagency-sales","acme_token":"acme-sales","rival_token":"rival-sales"}'

echo "== ensuring the shared external network exists =="
docker network create multiagency-data >/dev/null 2>&1 && echo "  created" || echo "  already present"

echo "== bringing up account-db + account-service =="
docker compose up -d --build

echo "== waiting for account-service health =="
fleet_wait_health http://127.0.0.1:8443/health

echo "== seeding =="
docker compose exec -T account-service python /app/deploy/account-intel/data/seed.py
# demo candidates via the one parameterized seeder
docker compose exec -T \
  -e REAL_DATA_DIR=/app/deploy/account-intel/data/candidates \
  -e SALES_ORG=multiagency-sales -e SALES_ORG_NAME="MultiAgency Sales" \
  account-service python /app/deploy/account-intel/data/seed_real.py

echo "== smoke: identity implies org + cross-org isolation =="
BASE="http://127.0.0.1:8443"
echo -n "  sales token finds 'northwind' (own org): "
curl -s -H "X-Service-Token: mia_sales_token" "$BASE/find_account?query=northwind" | fleet_json "d.get('match_count', d.get('error'))" # gitleaks:allow — synthetic dev token
echo -n "  rival token does NOT see 'northwind' (want 0): "
curl -s -H "X-Service-Token: rival_token" "$BASE/find_account?query=northwind" | fleet_json "d.get('match_count', d.get('error'))" # gitleaks:allow — synthetic dev token
echo -n "  caller CANNOT assert another org via X-Org-Id (rival token + X-Org-Id: multiagency-sales, want 0): "
curl -s -H "X-Service-Token: rival_token" -H "X-Org-Id: multiagency-sales" "$BASE/find_account?query=northwind" | fleet_json "d.get('match_count', d.get('error'))" # gitleaks:allow — synthetic dev token
echo -n "  unknown token -> 401: "
curl -s -o /dev/null -w '%{http_code}\n' -H "X-Service-Token: bogus" "$BASE/find_account?query=northwind"
echo "== account store up + seeded =="
