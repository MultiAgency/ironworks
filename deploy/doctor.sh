#!/usr/bin/env bash
# doctor.sh — verify an isolated ironworks agent end to end, in one command.
#
# Turns "debugging archaeology" into "one command tells you what's wrong." Every
# check here is a failure mode we actually hit standing up the Multiplex agent:
# dead container, stale token, stock/blank persona, DNS/tunnel not routing, webhook
# secret mismatch, missing ingress rule, unregistered webhook, or an image whose
# Telegram auth model doesn't match the pin.
#
# Usage:
#   ./doctor.sh                 # check every agent in ~/.agency/agents/*.env
#   ./doctor.sh plex            # check just one (slug)
#   ./doctor.sh plex --deep     # also assert the pinned Telegram auth model (creates+deletes a test user)
#
# A whole-fleet run (no slug) ends with a coverage check: a running ironclaw-*
# container with no env file here is checked by nothing, so it is reported rather
# than silently skipped.
#
# Optional bot-token checks (getMe / getWebhookInfo) run when a token file exists at
# ~/.agency/<slug>.token. Everything else works without the bot token.
#
# NOTE: intentionally does NOT use `set -e` — a failed check must not abort the rest.

LIB_DIR="$(cd "$(dirname "$0")" && pwd)/lib"
. "$LIB_DIR/curl-private.sh"   # curl_bearer/curl_header/curl_tg: tokens off argv
# shellcheck disable=SC1091  # resolved from the script's own dir at runtime
. "$LIB_DIR/fleet.sh"
SECRETS_DIR="${MULTRON_SECRETS_DIR:-$HOME/.agency/agents}"
CF_CONFIG="${CLOUDFLARED_CONFIG:-$HOME/.cloudflared/config.yml}"
TOKEN_DIR="${MULTRON_TOKEN_DIR:-$HOME/.agency}"

DEEP=0; TARGET=""
for a in "$@"; do case "$a" in --deep) DEEP=1 ;; -*) ;; *) TARGET="$a" ;; esac; done

c_grn=$'\033[32m'; c_red=$'\033[31m'; c_yel=$'\033[33m'; c_off=$'\033[0m'
AGENT_RC=0
pass() { printf "  ${c_grn}PASS${c_off} %s\n" "$1"; }
fail() { printf "  ${c_red}FAIL${c_off} %s\n       -> %s\n" "$1" "$2"; AGENT_RC=1; }
skip() { printf "  ${c_yel}SKIP${c_off} %s (%s)\n" "$1" "$2"; }


check_agent() {
  local envf="$1"; AGENT_RC=0
  local slug; slug=$(basename "$envf" .env)
  local API TOK HOST SECRET BOTU
  API=$(fleet_env_get "$envf" IRONCLAW_API_BASE)
  TOK=$(fleet_env_get "$envf" IRONCLAW_REBORN_WEBUI_TOKEN)
  HOST=$(fleet_env_get "$envf" AGENT_HOSTNAME)
  SECRET=$(fleet_env_get "$envf" TELEGRAM_WEBHOOK_SECRET)
  BOTU=$(fleet_env_get "$envf" TELEGRAM_BOT_USERNAME)
  local port="${API##*:}" container; container="$(fleet_container "$slug")"
  # A local instance has no public hostname: it is reached on loopback only and
  # fronts no Telegram bot (ironclaw-hq, the confidentiality-demo rig, is one).
  # Its container/API/persona checks all still apply; the public-route ones describe a
  # surface it does not have, so they SKIP rather than FAIL. (The --deep auth-model
  # check is a pure admin-API call and still runs — it asserts the image, not a bot.)
  local kind="agent" localonly=0
  [ -z "$HOST" ] && { localonly=1; kind="local instance"; }
  echo "=== $kind: $slug   container=$container  api=$API${BOTU:+  bot=@$BOTU}  host=${HOST:-(loopback only)} ==="

  # 1) container running
  if docker ps --format '{{.Names}}' | grep -qx "$container"; then pass "container running"
  else fail "container running" "docker start $container  (or re-run provision-agent.sh)"; fi

  # 2) operator API healthy
  local code
  code=$(curl_bearer "$TOK" -s -o /dev/null -w '%{http_code}' --max-time 8 "$API/api/webchat/v2/admin/users")
  if [ "$code" = 200 ]; then pass "operator API 200"; else fail "operator API (HTTP $code)" "token stale or container unhealthy"; fi

  # 3) custom persona installed AND intact: sentinel slug + body hash must verify
  # (the old >200-bytes/not-stock heuristic couldn't tell a truncated or
  # cross-wired persona from a healthy one)
  local vout vrc
  vout=$(docker exec "$container" sh -c "cat '$FLEET_PERSONA_DST' 2>/dev/null" 2>/dev/null \
    | "$LIB_DIR/compose-persona" verify --slug "$slug"); vrc=$?
  case "$vrc" in
    0) pass "persona sentinel verified ($vout)" ;;
    *) fail "persona: $vout" "reinstall via deploy/update-persona.sh $slug <persona-file> (restarts the container)" ;;
  esac

  # 3b) stray stock seed anywhere in the volume = upstream moved the prompt path
  # on a pin bump and seeded a stock prompt at the NEW location (the runtime
  # would serve THAT while the custom file at the pinned path still looks fine).
  # migrate-image.sh gates on this too; this is the live check.
  local strays
  strays=$(docker exec "$container" sh -c \
    "find /data/ironclaw-reborn -name '*.md' -exec grep -l 'You are IronClaw Agent' {} + 2>/dev/null" 2>/dev/null)
  if [ -n "$strays" ]; then
    fail "stray stock prompt in volume: $(printf '%s' "$strays" | tr '\n' ' ')" \
      "upstream seeded a stock prompt — the persona path likely moved on a pin bump; update the path in deploy/lib/fleet.sh and re-stamp the persona"
  else pass "no stray stock prompt in volume"; fi

  # 4) public webhook reachable (unsigned POST -> 401 means the route is live)
  if [ "$localonly" = 1 ]; then skip "public webhook" "local instance — no AGENT_HOSTNAME, not publicly routed"
  else
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 12 -X POST -H 'content-type: application/json' "https://$HOST/webhooks/extensions/telegram/updates" -d '{}')
    if [ "$code" = 401 ]; then pass "public webhook reachable (401 = live)"; else fail "public webhook (HTTP $code via https://$HOST)" "DNS/tunnel/ingress — check cloudflared is up and the ingress rule exists"; fi
  fi

  # 5) signed delivery accepted (secret matches) — skip if we do not hold the secret
  if [ "$localonly" = 1 ]; then skip "signed delivery" "local instance — no public webhook to sign for"
  elif [ -z "$SECRET" ]; then skip "signed delivery" "no TELEGRAM_WEBHOOK_SECRET in env"
  else
    code=$(curl_header "X-Telegram-Bot-Api-Secret-Token: $SECRET" -s -o /dev/null -w '%{http_code}' --max-time 10 -X POST -H 'content-type: application/json' "https://$HOST/webhooks/extensions/telegram/updates" -d '{"update_id":1}')
    if [ "$code" = 200 ]; then pass "signed delivery accepted (200)"; else fail "signed delivery (HTTP $code)" "webhook secret mismatch between config and TELEGRAM_WEBHOOK_SECRET"; fi
  fi

  # 6) cloudflared ingress rule present.
  # Anchored on the whole rule, not a substring: a bare `grep -q "$HOST"` PASSES for an agent
  # with no rule of its own whenever some OTHER hostname contains this one as a substring
  # (plex.example.com inside multiplex.example.com) — a health check confirming the wrong
  # answer. provision-agent.sh had the same bug on the insert side; both now match the rule.
  if [ "$localonly" = 1 ]; then skip "cloudflared ingress rule" "local instance — nothing should route to it publicly"
  elif awk -v h="$HOST" '$1=="-" && $2=="hostname:" && $3==h {found=1} END{exit !found}' "$CF_CONFIG" 2>/dev/null; then pass "cloudflared ingress rule present"
  else fail "cloudflared ingress rule" "add '  - hostname: $HOST / service: http://localhost:$port' to $CF_CONFIG and SIGHUP cloudflared"; fi

  # 7) bot-token checks (optional): getMe + getWebhookInfo
  local tf="$TOKEN_DIR/$slug.token"
  if [ -f "$tf" ]; then
    local BT me wh
    BT=$(tr -d '[:space:]' < "$tf")
    me=$(curl_tg "$BT" getMe -s --max-time 10 | fleet_json "d['result']['username'] if d.get('ok') else 'ERR'" 2>/dev/null)
    if [ "$me" = "$BOTU" ]; then pass "bot token valid (@$me)"; else fail "bot token" "getMe='$me' (expected @$BOTU) — wrong/revoked token"; fi
    wh=$(curl_tg "$BT" getWebhookInfo -s --max-time 12 | fleet_json "('registered' if d['result'].get('url') else 'NONE')+' pending='+str(d['result'].get('pending_update_count'))+' err='+str(d['result'].get('last_error_message'))" 2>/dev/null)
    case "$wh" in registered*err=None) pass "telegram webhook $wh" ;; registered*) fail "telegram webhook $wh" "delivery is erroring — check host reachability" ;; *) fail "telegram webhook $wh" "not registered — restart the container to re-register on boot" ;; esac
  else
    skip "bot token + webhook-registration checks" "no $tf"
  fi

  # 8) deep: assert the pinned Telegram auth model (device-link build, not deep-link).
  # On v1.3.0 Telegram declares method="device_link" and registers NO pairing service, so
  # pairing/mint 404s for extension_id=telegram (the generic route still exists — registry-
  # driven, per-extension; #7464). That 404 is BY DESIGN (see UPGRADE.md, migrate-image.sh).
  # The old round-trip (create -> mint deep link -> simulate /start -> connected?) drove a
  # ceremony telegram no longer registers and is retired with intake/provision-user.sh. We
  # assert instead that the running binary IS the device-link build — a 200 here means it is
  # an OLD deep-link build, not the pin. Pure admin-API call: no webhook surface/secret needed.
  if [ "$DEEP" = 1 ]; then
    local cu ut uid mint
    cu=$(curl_bearer "$TOK" -s -X POST -H 'content-type: application/json' -d '{"display_name":"doctor-authmodel-probe","role":"member"}' "$API/api/webchat/v2/admin/users")
    ut=$(printf '%s' "$cu" | fleet_json "d['api_token']" 2>/dev/null); uid=$(printf '%s' "$cu" | fleet_json "d['user']['user_id']" 2>/dev/null)
    if [ -z "$ut" ]; then fail "telegram auth model (setup)" "could not create probe user"; else
      mint=$(curl_bearer "$ut" -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{}' "$API/api/webchat/v2/extensions/telegram/pairing/mint")
      case "$mint" in
        404) pass "telegram auth model (device-link build; deep-link mint absent, as pinned)";;
        200) fail "telegram auth model (mint 200)" "running binary is a deep-link build, not the pinned device-link rev (#7464)";;
        *)   fail "telegram auth model (mint HTTP $mint)" "unexpected — check docker logs $container";;
      esac
      curl_bearer "$TOK" -s -o /dev/null -X DELETE "$API/api/webchat/v2/admin/users/$uid"   # cleanup probe user
    fi
  fi

  echo
  return $AGENT_RC
}

# Every check above is driven by an env file, so a running agent container with
# none is never looked at — and that silence reads as "fleet green" when it means
# "never looked". A real agent sat in exactly that state: a custom,
# unstamped persona that no check covered while the fleet reported all-clear.
# Whole-fleet mode only. There is deliberately no suppression list: an uncovered
# ironclaw-* container is either mid-provision, a leftover, or hand-built — and all
# three resolve by fixing the state, not by silencing the check.
check_fleet_coverage() {
  AGENT_RC=0
  local covered running c
  covered=$(for f in "$SECRETS_DIR"/*.env; do [ -e "$f" ] || continue; fleet_container "$(basename "$f" .env)"; done)
  running=$(docker ps --format '{{.Names}}' 2>/dev/null | grep '^ironclaw-')
  echo "=== fleet coverage: running agent containers vs $SECRETS_DIR/*.env ==="
  for c in $running; do
    printf '%s\n' "$covered" | grep -qx "$c" && continue
    fail "$c is running with no env file — no check above ran against it, persona included" \
      "add $SECRETS_DIR/<slug>.env for it, or stop the container if it is a leftover"
  done
  [ "$AGENT_RC" = 0 ] && pass "every running agent container has an env file"
  echo
  return $AGENT_RC
}

# --- run ------------------------------------------------------------------------
[ -d "$SECRETS_DIR" ] || { echo "no agents dir: $SECRETS_DIR"; exit 1; }
overall=0
if [ -n "$TARGET" ]; then
  f="$SECRETS_DIR/$TARGET.env"
  [ -f "$f" ] || { echo "no such agent env: $f"; exit 1; }
  check_agent "$f" || overall=1
else
  found=0
  for f in "$SECRETS_DIR"/*.env; do [ -e "$f" ] || continue; found=1; check_agent "$f" || overall=1; done
  [ "$found" = 1 ] || { echo "no agent env files in $SECRETS_DIR"; exit 1; }
  check_fleet_coverage || overall=1
fi
[ "$overall" = 0 ] && echo "${c_grn}all checks passed${c_off}" || echo "${c_red}some checks failed — see -> lines above${c_off}"
exit $overall
