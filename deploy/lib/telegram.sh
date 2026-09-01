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

# tg_webhook_get <bot-token> — the registered webhook URL and the last delivery error,
# TAB-separated, from ONE getWebhookInfo call. Both views below read from here, so the response
# shape is known in exactly one place and a caller needing both fields pays for one round trip.
#
# TAB-separated, and the URL first, so an EMPTY url stays positional — an unregistered bot
# reports `url: ""`, and that is precisely the case a caller must be able to see rather than
# have collapse into a missing field.
#
# `d.get("last_error_message") or "(none)"`, not `.get(k, default)`: a two-argument .get only
# substitutes the default when the KEY IS ABSENT, so a key present with a null returns the null
# and prints the string "None" into the operator's line. Verified against both response shapes.
tg_webhook_get() {
  curl_tg "$1" getWebhookInfo -s --max-time 12 \
    | python3 -c "import sys,json;d=json.load(sys.stdin)['result'];print(d.get('url','') + chr(9) + (d.get('last_error_message') or '(none)'))"
}

# tg_webhook_url <bot-token> — just the registered URL; empty when the bot has no webhook.
#
# THIS EXISTS BECAUSE tg_webhook_info CANNOT ANSWER IT. That helper prints
# `registered | last_error: (none)` and never a URL, so a caller comparing its output against an
# expected URL can never match. repoint-hostname.sh did exactly that: the comparison fell through
# on every run, including the fully successful one, and exited 1 after the container restart and
# before the new hostname was recorded. A question the library could not answer got asked of the
# helper that looked closest, which is the failure this function closes.
tg_webhook_url() {
  tg_webhook_get "$1" | cut -f1
}

# tg_webhook_info <bot-token> — one line: registration state + last delivery error.
#
# SPLIT WITH PARAMETER EXPANSION, NOT `IFS=$'\t' read`. TAB is IFS *whitespace*, and read
# discards leading IFS whitespace before assigning — so on the unregistered case, where the URL
# field is empty, the leading tab was eaten and the ERROR landed in the url variable. Every bot
# then reported `registered`, which is the opposite of what this line exists to say.
tg_webhook_info() {
  local _r _url _err
  _r="$(tg_webhook_get "$1")"
  _url="${_r%%	*}"     # up to the first TAB
  _err="${_r#*	}"      # after the first TAB
  if [ -n "$_url" ]; then printf 'registered | last_error: %s\n' "$_err"
  else printf 'NOT-registered | last_error: %s\n' "$_err"; fi
}

# tg_send <bot-token> <chat-id> <text> — one Bot API sendMessage.
#
# TWO COPIES, ONE LINE APART IN SPIRIT: `multi/serve/multi-watchdog.sh` alerted the team chat and
# `multi/serve/multi-backup.sh` alerted on a failed backup, in the same directory, with the same
# flags — and they had ALREADY diverged on the thing that matters, which is what a failed SEND
# means.
#
# THIS RETURNS CURL'S EXIT STATUS AND SWALLOWS NOTHING, because the two callers need opposite
# things and both are right. The watchdog must know: a failed alert has to be retried on the next
# tick rather than recorded as delivered, so it calls this bare. The backup calls it from an EXIT
# trap where a non-zero would overwrite the run's real exit code, so it appends `|| true` — at
# the call site, where a reader can see the decision instead of inheriting it from a helper.
#
# `--data-urlencode` for both fields: an alert body carries a hostname, a return code and a
# timestamp, and an unencoded `&` in any of them would silently truncate the message. `-f` so an
# HTTP error is a non-zero exit rather than a 200-shaped success.
tg_send() {
  curl_tg "$1" sendMessage -sf -m 15 \
    --data-urlencode "chat_id=$2" --data-urlencode "text=$3" >/dev/null
}
