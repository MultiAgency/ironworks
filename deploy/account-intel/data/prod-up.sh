#!/usr/bin/env bash
# PRODUCTION bring-up of the Account Store: postgres + service, NO demo identities, NO demo
# seed. The only valid credentials are per-client minted tokens in the hot-reloaded
# ~/.agency/account-identities/identities.json (written by provision.sh / register-identity.sh).
set -euo pipefail
cd "$(dirname "$0")"
. ../../lib/fleet.sh    # fleet_wait_health (+ curl_header, which smoke.sh uses)
. ./smoke.sh            # smoke_code — the checks below must be able to FAIL

docker network create multiagency-data >/dev/null 2>&1 || true
# strong DB password lives OUTSIDE the repo: ~/.agency/account-db.env (ACCOUNT_DB_PASSWORD=…,
# chmod 600, minted at first prod bring-up). The compose file has no password default —
# it refuses to interpolate without this file's value.
[ -f "$FLEET_AGENCY_DIR/account-db.env" ] || { umask 077
  echo "ACCOUNT_DB_PASSWORD=$(openssl rand -hex 24)" > "$FLEET_AGENCY_DIR/account-db.env"
  echo "minted ~/.agency/account-db.env (first bring-up)"; }
# `set -a` IS REQUIRED and must not be "unified" to a plain source (CONTRIBUTING.md,
# "Sourcing an env file"): docker-compose.yml interpolates ${ACCOUNT_DB_PASSWORD:?} and
# docker compose reads it from ITS environment. Nothing below names the variable, so the
# dependency is invisible in this file — which is exactly how it would get removed.
set -a; . "$FLEET_AGENCY_DIR/account-db.env"; set +a
mkdir -p "$FLEET_AGENCY_DIR/account-identities"
chmod 700 "$FLEET_AGENCY_DIR/account-identities"
if [ ! -e "$FLEET_AGENCY_DIR/account-identities/identities.json" ]; then
  umask 077
  printf '{}\n' > "$FLEET_AGENCY_DIR/account-identities/identities.json"
fi
ACCOUNT_DEV_IDENTITIES='' ./migrate.sh apply
ACCOUNT_DEV_IDENTITIES='' docker compose up -d account-service
fleet_wait_health http://127.0.0.1:8443/health
fleet_wait_health http://127.0.0.1:8443/ready

# THE TWO PROD INVARIANTS. Both abort the bring-up on mismatch, under the `set -euo pipefail`
# above: a production store that answers anything but 401 to an anonymous caller, or that
# honours the well-known dev token, must not be reported as up.
#
# The second one is also the load-bearing half of dev-up.sh's argument for using literal demo
# tokens: it is what proves THAT credential specifically is dead here.
echo "== smoke: the store is fail-closed and the dev credential is dead =="
smoke_code "no-token /list_accounts" 401 "" /list_accounts
smoke_code "demo token /list_accounts" 401 "X-Service-Token: mia_sales_token" /list_accounts # gitleaks:allow — dead demo token, negative test
