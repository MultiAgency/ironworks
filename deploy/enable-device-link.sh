#!/usr/bin/env bash
# enable-device-link.sh — add MTProto api_id/api_hash to a fleet agent's Telegram config
# so the instance can run device-link (Telegram's v1.3.0 account-connection ceremony).
#
# WHAT DEVICE-LINK IS FOR (read before using this): device-link is MTProto *account*
# linking — it logs IronClaw into a Telegram account as a third-party client so the agent
# can act AS that account (the linked-account tools). On v1.3.0 (#7464) it replaced the old
# lightweight deep-link pairing as Telegram's `[channel.connection]` strategy. It fits the
# SINGLE-USER case: one person connects THEIR own account to a fleet agent. It is NOT the
# right tool for onboarding a GROUP of contributors to chat — that's heavyweight (each must
# MTProto-link their whole account) and unverified for group @mentions; use the multi-tenant
# seam bridge (multi/seam/telegram_bridge.py, chat.id binding) for many-people chat instead.
# See docs/ARCHITECTURE.md § Member access.
#
# A BOT-ONLY agent (no api creds) fails every device-link attempt with "cannot be completed
# for this account" (verified: multron failed until these were set, multimediator with creds
# succeeded). This tool sets those two fields on one agent so device-link can START.
#
# The extension-config PUT is FULL-REPLACE, so this rebuilds the complete Telegram config
# from ~/.agency (the source of truth provision-agent.sh wrote from) and ADDS the two api
# fields — it never drops the bot token or webhook secret. It then re-activates and
# RESTARTS the container (boot re-registers the webhook), so run it when a brief blip on
# that agent is fine.
#
# api creds come from my.telegram.org -> "API development tools" (one app's id+hash can be
# reused across all your bots — they identify the developer app, not the linked account).
#
# Usage (creds via env, kept off argv and out of any shared session):
#   TELEGRAM_API_ID=123456 TELEGRAM_API_HASH=xxxx…  ./deploy/enable-device-link.sh multron
set -euo pipefail

SLUG="${1:?usage: TELEGRAM_API_ID=… TELEGRAM_API_HASH=… ./enable-device-link.sh <slug>}"
API_ID="${TELEGRAM_API_ID:?set TELEGRAM_API_ID (from my.telegram.org — the numeric api_id)}"
API_HASH="${TELEGRAM_API_HASH:?set TELEGRAM_API_HASH (from my.telegram.org — the secret api_hash)}"

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO_DIR/deploy/lib/curl-private.sh"   # curl_bearer/curl_tg: tokens off argv
. "$REPO_DIR/deploy/lib/fleet.sh"           # fleet_agent_env: ONE secrets-dir rule, MULTRON_SECRETS_DIR honoured
. "$REPO_DIR/deploy/lib/telegram.sh"        # tg_extension_config/tg_extension_install/tg_webhook_info

ENVFILE="$(fleet_agent_env "$SLUG")"
[ -f "$ENVFILE" ] || { echo "!! no agent env: $ENVFILE" >&2; exit 1; }
set -a; . "$ENVFILE"; set +a          # IRONCLAW_API_BASE, IRONCLAW_REBORN_WEBUI_TOKEN, AGENT_HOSTNAME, CONTAINER, TELEGRAM_WEBHOOK_SECRET, TELEGRAM_BOT_USERNAME
API="${IRONCLAW_API_BASE:?}"; OP_TOKEN="${IRONCLAW_REBORN_WEBUI_TOKEN:?}"
BOT_USERNAME="${TELEGRAM_BOT_USERNAME:?}"; WH_SECRET="${TELEGRAM_WEBHOOK_SECRET:?}"
CONTAINER="${CONTAINER:?}"; HOSTN="${AGENT_HOSTNAME:?}"
WEBHOOK_URL="https://$HOSTN/webhooks/extensions/telegram/updates"

TOKEN_FILE="$HOME/.agency/$SLUG.token"
[ -f "$TOKEN_FILE" ] || { echo "!! no bot token file: $TOKEN_FILE" >&2; exit 1; }
BOT_TOKEN="$(cat "$TOKEN_FILE")"

# Current active_revision — the FULL-REPLACE PUT needs it as expected_revision (optimistic
# concurrency; a wrong value 409s and changes nothing, so this is safe either way). It lives
# in the agent's virtual FS (DB), not an API read; pull it via docker + sqlite3.
command -v sqlite3 >/dev/null || { echo "!! sqlite3 not on PATH (needed to read the config revision)" >&2; exit 1; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
docker cp "$CONTAINER:/data/ironclaw-reborn/hosted-single-tenant-volume/reborn-local-dev.db" "$TMP/a.db" >/dev/null 2>&1 \
  || { echo "!! could not read $CONTAINER's DB (is it running?)" >&2; exit 1; }
REV="$(sqlite3 "$TMP/a.db" "SELECT CAST(contents AS TEXT) FROM root_filesystem_entries WHERE path='/tenants/reborn-cli/shared/extension-admin-configuration/groups/extension.telegram.json';" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("active_revision",0))')"
[ -n "$REV" ] || { echo "!! could not read active_revision" >&2; exit 1; }
echo "== $SLUG: telegram config at revision $REV -> replacing with +api_id/+api_hash =="

# Full-replace config: every existing handle rebuilt from ~/.agency + the two new api fields.
# Secrets (bot token, webhook secret, api_hash) travel via env into the heredoc and out over
# stdin (--data @-), never argv. api_id is non-secret.
IDEM="$SLUG-devlink-$(openssl rand -hex 6)"
cfg_status=$(
  TG_BOT_TOKEN="$BOT_TOKEN" TG_WH_SECRET="$WH_SECRET" \
  TG_EXTRA_HANDLES="$(API_HASH="$API_HASH" API_ID="$API_ID" python3 -c 'import os, json; print(json.dumps([
      {"handle": "telegram_api_id",   "value": os.environ["API_ID"]},
      {"handle": "telegram_api_hash", "value": os.environ["API_HASH"]}]))')" \
  tg_extension_config "$OP_TOKEN" "$API" "$WEBHOOK_URL" "$BOT_USERNAME" "$REV" "$IDEM")
case "$cfg_status" in
  2??) echo "-- config replaced (HTTP $cfg_status)";;
  409) echo "!! HTTP 409 — revision moved under us; re-run (it changed nothing)" >&2; exit 1;;
  *)   echo "!! config PUT FAILED (HTTP $cfg_status) — nothing installed/restarted" >&2; exit 1;;
esac

inst_status=$(tg_extension_install "$OP_TOKEN" "$API" "devlink-$SLUG")
case "$inst_status" in 2??) echo "-- telegram re-activated (HTTP $inst_status)";; *) echo "!! install FAILED (HTTP $inst_status)" >&2; exit 1;; esac

echo "-- restarting $CONTAINER (boot re-registers the webhook)…"
docker restart "$CONTAINER" >/dev/null
fleet_wait_api "$OP_TOKEN" "$API"

wh=$(tg_webhook_info "$BOT_TOKEN")
echo "-- webhook: $wh"

# Verify device-link now INITIATES (was failing "cannot be completed" while bot-only). Start a
# flow, read its step, then cancel it so nothing is left pending.
start=$(curl_bearer "$OP_TOKEN" -s -X POST -H 'content-type: application/json' \
  -d '{"provider":"telegram","extension_name":"telegram","mode":"default"}' \
  "$API/api/reborn/product-auth/device-link/start")
# One interpreter, three fields — this spawned three pythons to re-parse the same string.
# Tab-separated so an empty field stays positional; `|| true` keeps a malformed body on the
# existing "?"/empty path rather than aborting under pipefail.
IFS=$'\t' read -r step flow inv <<<"$(printf '%s' "$start" | python3 -c 'import sys,json
d = json.load(sys.stdin)
print("\t".join([d.get("device_link", {}).get("step", "?"), d.get("flow_id", ""), d.get("invocation_id", "")]))' 2>/dev/null || printf '?\t\t')"
[ -n "$flow" ] && curl_bearer "$OP_TOKEN" -s -o /dev/null -X POST -H 'content-type: application/json' \
  -d "{\"flow_id\":\"$flow\",\"invocation_id\":\"$inv\"}" "$API/api/reborn/product-auth/device-link/cancel" || true
if [ "$step" = "failed" ]; then
  echo "!! device-link still reports step=failed — api creds may be wrong; check the values" >&2; exit 1
fi
echo "== done: $SLUG can now start device-link (step=$step). Members can link their account. =="
