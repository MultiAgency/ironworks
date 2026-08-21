#!/usr/bin/env bash
# PRODUCTION bring-up of the Account Store: postgres + service, NO demo identities, NO demo
# seed. The only valid credentials are per-client minted tokens in the hot-reloaded
# ~/.agency/account-identities/identities.json (written by provision.sh / register-identity.sh).
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh    # fleet_wait_health

docker network create multiagency-data >/dev/null 2>&1 || true
# strong DB password lives OUTSIDE the repo: ~/.agency/account-db.env (ACCOUNT_DB_PASSWORD=…,
# chmod 600, minted at first prod bring-up). The compose file has no password default —
# it refuses to interpolate without this file's value.
[ -f "$HOME/.agency/account-db.env" ] || { umask 077
  echo "ACCOUNT_DB_PASSWORD=$(openssl rand -hex 24)" > "$HOME/.agency/account-db.env"
  echo "minted ~/.agency/account-db.env (first bring-up)"; }
set -a; . "$HOME/.agency/account-db.env"; set +a
ACCOUNT_DEV_IDENTITIES='' docker compose up -d --build

fleet_wait_health http://127.0.0.1:8443/health

echo -n "no-token -> 401 (fail closed): "
curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:8443/list_accounts"
echo -n "demo token is DEAD in prod (want 401): "
curl -s -o /dev/null -w '%{http_code}\n' -H "X-Service-Token: mia_sales_token" "http://127.0.0.1:8443/list_accounts" # gitleaks:allow — dead demo token, negative test
