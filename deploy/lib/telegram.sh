# deploy/lib/telegram.sh — the Telegram extension surface: config CAS, install, webhook probe.
#
# SECRETS NEVER REACH argv. The bot token, webhook secret and any extra secret handle travel in
# the environment and out over stdin (--data @-). TG_EXTRA_HANDLES is an env var rather than a
# positional for that reason: device-link's telegram_api_hash is a secret, and a positional
# would put it in the process table.
#
# shellcheck shell=bash

_TG_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=deploy/lib/curl-private.sh
. "$_TG_LIB_DIR/curl-private.sh"

# tg_extension_config <op-token> <api> <webhook-url> <bot-username> <revision> <idempotency-key>
#   env in:  TG_BOT_TOKEN, TG_WH_SECRET   (required)
#            TG_EXTRA_HANDLES             (optional JSON array of {"handle","value"} objects)
#   prints:  the HTTP status of the PUT
#
# Full group replace under optimistic concurrency: a wrong expected_revision 409s and changes
# nothing, so re-running is safe either way.
tg_extension_config() {
  local _op="$1" _api="$2" _url="$3" _user="$4" _rev="$5" _idem="$6"
  TG_URL="$_url" TG_USER="$_user" TG_REV="$_rev" TG_IDEM="$_idem" python3 - <<'PY' \
    | curl_bearer "$_op" -s -o /dev/null -w '%{http_code}' -X PUT \
        -H 'content-type: application/json' --data @- \
        "$_api/api/webchat/v2/operator/extension-configuration/extension.telegram"
import os, json
values = [
    {"handle": "telegram_bot_token",      "value": os.environ["TG_BOT_TOKEN"]},
    {"handle": "telegram_webhook_secret", "value": os.environ["TG_WH_SECRET"]},
    {"handle": "telegram_webhook_url",    "value": os.environ["TG_URL"]},
    {"handle": "bot_username",            "value": os.environ["TG_USER"]},
]
values += json.loads(os.environ.get("TG_EXTRA_HANDLES") or "[]")
print(json.dumps({"values": values,
                  "expected_revision": int(os.environ["TG_REV"]),
                  "idempotency_key": os.environ["TG_IDEM"]}))
PY
}

# tg_extension_install <op-token> <api> <client-action-id> — activate the telegram extension.
# Prints the HTTP status; the caller decides what a non-2xx means for it.
tg_extension_install() {
  curl_bearer "$1" -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' \
    -d "{\"package_ref\":{\"kind\":\"extension\",\"id\":\"telegram\"},\"client_action_id\":\"$3\"}" \
    "$2/api/webchat/v2/extensions/install"
}

# tg_webhook_info <bot-token> — one line: registration state + last delivery error.
tg_webhook_info() {
  curl_tg "$1" getWebhookInfo -s --max-time 12 \
    | python3 -c "import sys,json;d=json.load(sys.stdin)['result'];print('registered' if d.get('url') else 'NOT-registered','| last_error:',d.get('last_error_message','(none)'))"
}
