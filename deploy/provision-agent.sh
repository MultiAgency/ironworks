#!/usr/bin/env bash
# Provision one ISOLATED agent instance for a Telegram group — the "system for
# isolated agents in groups", instance-per-agent on the stock official ironclaw
# binary. Each agent is its own container + volume + bot + persona; the harness
# enforces isolation between them (separate processes, separate SQLite-VFS state).
#
# NOT the canonical IronWorks product path. This is the single-tenant fleet: a supported
# adjunct and shared operational tooling (README.md "What else is in this repository"). It
# predates the seam, which solved one-bot-many-groups a different way — many sealed members
# on ONE multi-tenant instance, organization-scoped, with guidance and confinement
# (multi/README.md). The two are not successor and predecessor: this path gives an agent its
# own tools, memory, volume, and bot, all of which the product path deliberately denies, and
# it gives it none of the tenant, organization, or service-definition constructs.
# Reach for the seam for organization-scoped business services and many-people chat; reach
# for this for a standalone group agent. Ceiling is Telegram bot creation (BotFather is
# manual), which is fine for a handful/dozens.
#
# PREREQUISITES you provide:
#   - a Telegram bot from BotFather (token + username)   [the one manual step]
#   - the ironclaw:main image built locally
#   - a running named cloudflared tunnel "ironclaw" (~/.cloudflared/config.yml)
#   - NEAR AI creds (NEARAI_API_KEY — always passed explicitly, never copied from
#     a running instance)
#
# Usage:
#   BASE_DOMAIN=example.com \
#   TELEGRAM_BOT_TOKEN=123:ABC TELEGRAM_BOT_USERNAME=AcmeMulti_bot \
#   NEARAI_API_KEY=sk-... PURPOSE="onboarding new Acme stakeholders" \
#     ./provision-agent.sh "Acme"
#
# BASE_DOMAIN is required and has no default: agent hostnames are <slug>.$BASE_DOMAIN,
# so a default would bake one deployment's domain into the repo. Keep it in ~/.agency.
#
# End to end: this stands the instance up AND wires Telegram — container, persona,
# DNS + tunnel ingress (config backed up first), a cloudflared SIGHUP hot-reload
# (does NOT drop other tunnels), telegram deployment config + install, and the
# activation restart that registers the webhook. Verified: the exact sequence used
# to stand up the Multiplex agent by hand. After it runs, only one manual step
# remains (printed at the end): add the bot to the group as an admin.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO_DIR/deploy/lib/curl-private.sh"   # curl_bearer/curl_tg: operator + bot tokens off argv
. "$REPO_DIR/deploy/lib/fleet.sh"
. "$REPO_DIR/deploy/lib/cloudflare.sh"   # cf_ingress_add/cf_reload/cf_wait_dns
. "$REPO_DIR/deploy/lib/telegram.sh"     # tg_extension_config/tg_extension_install/tg_webhook_info
NAME="${1:?usage: provision-agent.sh \"<Agent Name>\"  (also set TELEGRAM_BOT_TOKEN, TELEGRAM_BOT_USERNAME, PURPOSE)}"
BOT_TOKEN="${TELEGRAM_BOT_TOKEN:?set TELEGRAM_BOT_TOKEN (from BotFather)}"
BOT_USERNAME="${TELEGRAM_BOT_USERNAME:?set TELEGRAM_BOT_USERNAME (from BotFather, no @)}"
PURPOSE="${PURPOSE:-helping this group}"
AGENT_NAME="${AGENT_NAME:-Multi}"            # persona name in-group; default "Multi"
_inherited_persona_source="${PERSONA_SOURCE:-}"   # captured pre-default for the env-guard below
PERSONA_SOURCE="${PERSONA_SOURCE:-agent/identity/MULTI.template.md}"  # repo-relative or absolute persona file
BASE_DOMAIN="${BASE_DOMAIN:?set BASE_DOMAIN — the domain your agent hostnames live under; keep it in ~/.agency, never in the repo}"
TUNNEL="${CLOUDFLARED_TUNNEL:-ironclaw}"
CF_CONFIG="${CLOUDFLARED_CONFIG:-$HOME/.cloudflared/config.yml}"
# Images are built as rev-named tags (ironclaw:<9-char rev> from IRONCLAW_PIN);
# ironclaw:main is retagged to the current pin. See deploy/README.md.
IMAGE="${IRONCLAW_IMAGE:-ironclaw:main}"

# Guard against a shell prepared for ANOTHER agent: the documented intake step
# (`set -a; source ~/.agency/agents/<slug>.env`) exports TELEGRAM_BOT_USERNAME,
# AGENT_HOSTNAME, CONTAINER, IRONCLAW_REBORN_WEBUI_TOKEN, TELEGRAM_WEBHOOK_SECRET,
# and PERSONA_SOURCE — inheriting them here silently cross-wires the
# new agent with the old agent's bot username, hostname, or persona file (the
# PERSONA_SOURCE swap is the worst: invisible in the provisioning output).
# Deliberate reuse (e.g. the AGENT_HOSTNAME repurpose override, or an explicit
# custom PERSONA_SOURCE) must say so with PROVISION_FROM_ENV=1.
if [ "${PROVISION_FROM_ENV:-}" != 1 ] && [ -n "${CONTAINER:-}${IRONCLAW_REBORN_WEBUI_TOKEN:-}${TELEGRAM_WEBHOOK_SECRET:-}${AGENT_HOSTNAME:-}${_inherited_persona_source}" ]; then
  echo "!! environment carries another agent's provisioning state" >&2
  echo "   (one of CONTAINER / IRONCLAW_REBORN_WEBUI_TOKEN / TELEGRAM_WEBHOOK_SECRET / AGENT_HOSTNAME / PERSONA_SOURCE is set)." >&2
  echo "   Run from a clean shell, or set PROVISION_FROM_ENV=1 to use the inherited values deliberately." >&2
  exit 1
fi

slug="$(printf '%s' "$NAME" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-//;s/-$//')"
[ -n "$slug" ] || { echo "!! could not derive a slug from '$NAME'" >&2; exit 1; }
container="ironclaw-$slug"
volume="ironclaw-$slug-data"
hostname="${AGENT_HOSTNAME:-$slug.$BASE_DOMAIN}"   # override to repurpose an existing host (e.g. point a new agent at an existing console hostname)
webhook_url="https://$hostname/webhooks/extensions/telegram/updates"

# --- compose the persona FIRST, so any bad input fails before a container, volume,
# or DNS record exists (the old sed pipeline died here on a '/' in PURPOSE, after
# docker run — leaving an orphan the re-run guard then blocked) ---------------------
case "$PERSONA_SOURCE" in /*) ;; *) PERSONA_SOURCE="$REPO_DIR/$PERSONA_SOURCE" ;; esac
[ -f "$PERSONA_SOURCE" ] || { echo "!! persona source not found: $PERSONA_SOURCE" >&2; exit 1; }
persona_prompt="$("$REPO_DIR/deploy/lib/compose-persona" compose \
  --persona "$PERSONA_SOURCE" --tail "$REPO_DIR/agent/identity/_operational-tail.md" \
  --slug "$slug" --slot "AGENT_NAME=$AGENT_NAME" --slot "PURPOSE=$PURPOSE")"

# --- allocate the next free loopback port from 3001 up ---------------------------
# ONE lsof for the whole scan. This used to spawn a fresh lsof per candidate — and lsof walks
# every open fd on the box each time — so on a host already running twenty agents it cost twenty
# full scans to find port 3021, getting slower as the fleet grew.
# `|| true`: no listeners at all is a legitimate empty result, not a failure, and under pipefail
# an empty grep would abort the assignment.
listening="$(lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | awk 'NR>1 {n=split($9,a,":"); print a[n]}' | sort -u || true)"
port=""
for p in $(seq 3001 3099); do
  if ! printf '%s\n' "$listening" | grep -qx "$p"; then port="$p"; break; fi
done
[ -n "$port" ] || { echo "!! no free port in 3001-3099" >&2; exit 1; }

# --- secrets: fresh operator token + webhook secret per instance -----------------
OP_TOKEN="$(openssl rand -hex 32)"
WH_SECRET="$(openssl rand -hex 32)"
# NEAR AI: always explicit — never harvested from a running container (which key a
# new agent got was an accident of container ordering, and it spread one key fleet-wide).
NEARAI_API_KEY="${NEARAI_API_KEY:?set NEARAI_API_KEY explicitly for this agent (NEAR console; never copy the key from another instance)}"
NEARAI_BASE_URL="${NEARAI_BASE_URL:-https://cloud-api.near.ai}"

echo "== provisioning agent =="
echo "  name/slug   : $NAME / $slug"
echo "  container   : $container   volume: $volume   port: 127.0.0.1:$port"
echo "  hostname    : $hostname"
echo "  persona     : $AGENT_NAME  (purpose: $PURPOSE)"
echo

# --- 1) run the stock ironclaw instance ------------------------------------------
if docker ps -a --format '{{.Names}}' | grep -qx "$container"; then
  echo "!! container $container already exists — pick a different name or remove it" >&2; exit 1
fi
# 1.4 secure-default freeze: no SSH key/port, sandbox-proxy override, extra sandbox domains, or
# memory-curation setting is supplied here. The stock contained workspace default remains in use.
docker run -d --name "$container" --restart unless-stopped \
  -p "127.0.0.1:$port:3000" -v "$volume:/data" \
  -e IRONCLAW_REBORN_PROFILE=hosted-single-tenant-volume \
  -e IRONCLAW_REBORN_SERVE_HOST=0.0.0.0 \
  -e IRONCLAW_REBORN_WEBUI_USER_ID=reborn-cli \
  -e IRONCLAW_REBORN_WEBUI_TOKEN="$OP_TOKEN" \
  -e NEARAI_BASE_URL="$NEARAI_BASE_URL" \
  -e NEARAI_API_KEY="$NEARAI_API_KEY" \
  -e IRONCLAW_REBORN_LOG=info \
  "$IMAGE" >/dev/null
echo "-- container up; waiting for API --"
# fleet_wait_api owns the failure clause. That matters here: a dead API after all retries used
# to be SKIPPED at two of this script's three wait sites (they had no `||` clause at all), and
# the script would go on to install a persona into a non-responsive instance. All three now
# fail loudly, because the helper does.
fleet_wait_api "$OP_TOKEN" "http://127.0.0.1:$port"
echo "   API ready"

# --- 2) install the composed persona (built above, before any resource existed) ---
echo "-- persona source: $(basename "$PERSONA_SOURCE")"
fleet_install_persona "$container" "$persona_prompt"
docker restart "$container" >/dev/null
fleet_wait_api "$OP_TOKEN" "http://127.0.0.1:$port"
echo "-- persona '$AGENT_NAME' installed"

# --- 3) cloudflared: DNS route + ingress rule (config backed up) + SIGHUP reload --
cloudflared tunnel route dns "$TUNNEL" "$hostname" 2>&1 | sed 's/^/   dns: /' || true
cp "$CF_CONFIG" "$CF_CONFIG.bak.$(date +%s 2>/dev/null || echo prev)" 2>/dev/null || \
  cp "$CF_CONFIG" "$CF_CONFIG.bak" 2>/dev/null || true
cf_ingress_add "$CF_CONFIG" "$hostname" "$port"

cf_reload "$hostname"

# --- 3b) wait until the new hostname publicly resolves --------------------------
cf_wait_dns "$hostname"

# --- 4) configure Telegram + activate (config -> install -> restart registers webhook) ---
API="http://127.0.0.1:$port"
IDEM="$slug-tg-$(openssl rand -hex 6)"
cfg_status=$(TG_BOT_TOKEN="$BOT_TOKEN" TG_WH_SECRET="$WH_SECRET" \
  tg_extension_config "$OP_TOKEN" "$API" "$webhook_url" "$BOT_USERNAME" 0 "$IDEM")
# Don't just report the status — ACT on it. A non-2xx config (409 stale revision, 401 bad token)
# means Telegram was NOT configured; continuing would install+restart an agent that never
# registers its webhook and looks provisioned but is deaf.
case "$cfg_status" in
  2??) echo "-- telegram config saved (HTTP $cfg_status)" ;;
  *) echo "!! telegram config FAILED (HTTP $cfg_status) — not installing; fix and re-run" >&2; exit 1 ;;
esac
inst_status=$(tg_extension_install "$OP_TOKEN" "$API" "activate-tg-$slug")
case "$inst_status" in
  2??) echo "-- telegram extension install accepted (HTTP $inst_status)" ;;
  *) echo "!! telegram extension install FAILED (HTTP $inst_status)" >&2; exit 1 ;;
esac
docker restart "$container" >/dev/null   # boot activation registers the webhook with Telegram
fleet_wait_api "$OP_TOKEN" "$API"
wh=$(tg_webhook_info "$BOT_TOKEN")
echo "-- telegram webhook: $wh"

# --- write per-agent secrets to a gitignored file (never to stdout) --------------
SECRETS_DIR="${MULTRON_SECRETS_DIR:-$FLEET_AGENCY_DIR/agents}"
umask 077; mkdir -p "$SECRETS_DIR"
secfile="$SECRETS_DIR/$slug.env"
q() { local s=$1 sq=\'; printf "'%s'" "${s//$sq/$sq\\$sq$sq}"; }   # single-quote for source-compat (PURPOSE has spaces)
cat > "$secfile" <<EOF2
# ironworks agent "$NAME" — keep private; never commit
IRONCLAW_API_BASE=http://127.0.0.1:$port
IRONCLAW_REBORN_WEBUI_TOKEN=$OP_TOKEN
TELEGRAM_BOT_USERNAME=$BOT_USERNAME
AGENT_HOSTNAME=$hostname
TELEGRAM_WEBHOOK_SECRET=$WH_SECRET
CONTAINER=$container
AGENT_NAME=$(q "$AGENT_NAME")
PURPOSE=$(q "$PURPOSE")
PERSONA_SOURCE=$(q "$PERSONA_SOURCE")
EOF2
chmod 600 "$secfile"

echo
echo "================= agent ready — 1 manual step left ================="
echo "1) Add @$BOT_USERNAME to the group AND make it an ADMIN. (Admin upgrades a basic"
echo "   group to a supergroup and guarantees the bot receives @mentions — basic groups"
echo "   with privacy-mode ON silently drop them. Learned the hard way with Multiplex.)"
echo "   The group is served once the bot is an admin. Provisioning mints NO per-member link,"
echo "   and whether the pinned rev offers a per-member connect ceremony at all varies —"
echo "   upstream has moved it both ways (#7464, #7766). See deploy/README.md."
echo "===================================================================="
echo
echo "agent '$NAME' -> $container on 127.0.0.1:$port, https://$hostname, bot @$BOT_USERNAME"
echo "secrets written to: $secfile  (chmod 600)"
