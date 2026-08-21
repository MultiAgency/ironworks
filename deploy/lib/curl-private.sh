#!/usr/bin/env bash
# curl-private.sh — sourceable helpers that keep secrets OFF the process argv.
#
# Why: `curl -H "Authorization: Bearer $TOK" ...` and `curl ".../bot$TOK/..."` put the secret on
# the command line, where any user on the host can read it via `ps` for the life of the request.
# These move it into a chmod-600 curl config file passed with `-K`, so it never appears on argv;
# every non-secret option (-s, -X, -d, -o, --max-time, …) is passed normally and merged.
#
# Usage (source it):  . "$(dirname "$0")/../lib/curl-private.sh"   (adjust the relative path)
#   curl_bearer "$TOK"      -s -X POST -d @- "$API/path"     # Authorization: Bearer <tok>
#   curl_header "X-Service-Token: $TOK"  -s "$API/path"      # arbitrary secret header
#   curl_tg     "$BOT_TOKEN" getWebhookInfo  -s --max-time 12 # Telegram Bot API, token off the URL
#
# Notes: tokens here are hex/base64 (no embedded quotes), so the simple `header = "..."` quoting is
# safe. Each helper returns curl's own exit status and removes its temp file even on failure.

_curl_with_config() {           # $1 = config-file content (secret); rest = curl args
  local _cfg _rc
  _cfg="$(mktemp)" || return 1
  chmod 600 "$_cfg"
  printf '%s\n' "$1" > "$_cfg"
  shift
  curl -K "$_cfg" "$@"; _rc=$?
  rm -f "$_cfg"
  return "$_rc"
}

curl_bearer() {                 # $1 = token; rest = curl args
  local _tok="$1"; shift
  _curl_with_config "header = \"Authorization: Bearer ${_tok}\"" "$@"
}

curl_header() {                 # $1 = full header line (e.g. "X-Service-Token: abc"); rest = curl args
  local _hdr="$1"; shift
  _curl_with_config "header = \"${_hdr}\"" "$@"
}

curl_tg() {                     # $1 = bot token; $2 = API method; rest = curl args
  local _bt="$1" _method="$2"; shift 2
  _curl_with_config "url = \"https://api.telegram.org/bot${_bt}/${_method}\"" "$@"
}
