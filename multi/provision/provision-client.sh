#!/usr/bin/env bash
# Provision ONE client as a sealed account on the multi-tenant "multi" instance.
#
# Creates a Member user — its own (tenant,user) scope + api_token. That account's agent,
# conversations, memory, and data are invisible to every other client (proven: a cross-account
# request for another account's thread returns 404, and no account can execute code — SecureDefault).
#
# TOKEN CUSTODY — INVARIANT, DO NOT BREAK:
#   The returned token belongs to the SEAM, never to the client. Store it in the registry
#   (~/.agency/clients/<slug>.env, chmod 600); the bridge uses it on the client's behalf.
#   NEVER hand this token to a client, a partner, a contributor, or any end user, and never
#   expose an API path that lets them present their own.
#   Why: the member's no-egress guarantee is a per-bearer tool disable, and per-bearer state
#   is reversible BY THE BEARER. Whoever holds this token can re-enable `builtin.http` and
#   exfiltrate that client's private context to any public host — silently, with no other
#   control in the system standing in the way (ironclaw's builtin policy grants wildcard
#   PUBLIC egress by default; only private-IP ranges are blocked). Handing out the token
#   doesn't weaken the confinement, it voids it.
#   (An earlier version of this header said "hand it to their channel/agent" — that predates
#   the confinement and was wrong. See SECURITY.md § Trust boundaries.)
#
# Usage:
#   IRONCLAW_API=https://your-agency-instance.example.com \
#   IRONCLAW_OPERATOR_TOKEN=<operator token> \
#   ./provision-client.sh "Acme Corp"          # human-readable
#   ./provision-client.sh --env "Acme Corp"    # machine-readable KEY=VALUE lines (for scripts)
set -euo pipefail
. "$(dirname "$0")/../../deploy/lib/fleet.sh"   # curl_bearer (token off argv) + fleet_json
ENV_OUT=""
if [ "${1:-}" = "--env" ]; then ENV_OUT=1; shift; fi
API="${IRONCLAW_API:?set IRONCLAW_API (the multi-tenant instance base URL)}"
OP="${IRONCLAW_OPERATOR_TOKEN:?set IRONCLAW_OPERATOR_TOKEN (a current operator/admin token)}"
NAME="${1:?client display name, e.g. \"Acme Corp\"}"

json_str() { python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$1"; }

resp=$(curl_bearer "$OP" -sS -w '\n%{http_code}' --max-time 20 -X POST \
  -H 'content-type: application/json' \
  -d "{\"display_name\":$(json_str "$NAME"),\"role\":\"member\"}" \
  "$API/api/webchat/v2/admin/users")
code="${resp##*$'\n'}"; body="${resp%$'\n'*}"
if [ "$code" != "200" ] && [ "$code" != "201" ]; then
  echo "!! provision failed (HTTP $code): $body" >&2
  [ "$code" = "401" ] && echo "   -> operator token invalid/expired; refresh it." >&2
  exit 1
fi
uid=$(printf '%s' "$body" | fleet_json "d['user']['user_id']")
token=$(printf '%s' "$body" | fleet_json "d['api_token']")

if [ -n "$ENV_OUT" ]; then
  echo "IRONCLAW_USER_ID=$uid"
  echo "IRONCLAW_TOKEN=$token"
  exit 0
fi

# human mode: never echo the token — write it to a chmod-600 file and print the path.
slug="$(printf '%s' "$NAME" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-//;s/-$//')"
umask 077; mkdir -p "$FLEET_AGENCY_DIR/sealed"
out="$FLEET_AGENCY_DIR/sealed/${slug:-client}.env"
{ echo "IRONCLAW_USER_ID=$uid"; echo "IRONCLAW_TOKEN=$token"; } > "$out"
echo "✅ provisioned sealed account for: $NAME"
echo "   user_id : $uid"
echo "   token   : written to $out (chmod 600 — not echoed)"
echo
echo "   The token grants ONLY this account's scope. Its threads/memory/data are inaccessible to"
echo "   every other client, and it cannot execute code. It is $NAME's access — handle the file accordingly."
