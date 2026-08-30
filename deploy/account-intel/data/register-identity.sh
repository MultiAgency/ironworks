#!/usr/bin/env bash
# Register (or update) one org token in the Account Service's HOT-RELOADED identity file —
# ~/.agency/account-identities/identities.json, read via ACCOUNT_IDENTITIES_FILE (service.py).
# Atomic tmp+rename in the mounted DIRECTORY, then polls until the service resolves the token.
# No restart, no interruption for current clients. Never touches the repo or the container.
#
# The org token is a CLIENT-DATA credential — it is passed via the ORG_TOKEN env var, NOT on the
# command line, so it never appears in the process table (`ps`) on a shared host. The poll
# below likewise keeps the token off curl's argv, via curl_header from deploy/lib/curl-private.sh.
#
# Usage: ORG_TOKEN=<token> register-identity.sh <org_id>     [ACCOUNT_BASE=http://127.0.0.1:8443]
set -euo pipefail
# fleet.sh, not curl-private.sh directly: it brings curl_header in with it and it owns the two
# other rules this script was carrying its own copy of — FLEET_AGENCY_DIR (`AGENCY_DIR` was
# spelled out inline here, so this script honoured a knob the rest of the fleet resolves in one
# place) and fleet_http_code (see the poll below).
. "$(dirname "$0")/../../lib/fleet.sh"
ORG="${1:?usage: ORG_TOKEN=<token> register-identity.sh <org_id>}"
TOKEN="${ORG_TOKEN:?set ORG_TOKEN=<token> in the environment (not on argv — it is a secret)}"
ACCOUNT_BASE="${ACCOUNT_BASE:-http://127.0.0.1:8443}"
IDENT_DIR="$FLEET_AGENCY_DIR/account-identities"

umask 077
# deploy/lib/identities.py is the one reader AND writer of this map. The corrupt-file refusal
# that used to live here in a heredoc lives there now, where the two REMOVAL paths get it too —
# they were the copies that did not have it. ORG_TOKEN stays in the environment, never argv.
ORG_TOKEN="$TOKEN" ACCOUNT_IDENTITIES_FILE="$IDENT_DIR/identities.json" \
  python3 "$(dirname "$0")/../../lib/identities.py" add "$ORG"

for _ in $(seq 1 10); do
  # fleet_http_code: a connection refusal (service down) makes curl exit non-zero, which under
  # `set -e` would kill the script on the FIRST iteration — never retrying, never reaching the
  # "is the data stack running?" diagnostic below (written for exactly this case). The helper
  # swallows that into a sentinel code so the loop runs and the diagnostic prints.
  code=$(fleet_http_code curl_header "X-Service-Token: $TOKEN" \
    -s -o /dev/null -w '%{http_code}' "$ACCOUNT_BASE/list_accounts")
  [ "$code" = "200" ] && { echo "   identity live for org '$ORG' (hot reload — no restart)"; exit 0; }
  sleep 1
done
echo "!! token for org '$ORG' not resolving (HTTP $code) — is the data stack running with the" >&2
echo "   identities mount? One-time after upgrading: cd $(dirname "$0") && docker compose up -d account-service" >&2
exit 1
