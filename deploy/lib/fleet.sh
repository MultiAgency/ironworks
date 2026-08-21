# deploy/lib/fleet.sh — fleet-wide constants + helpers, sourced by
# provision-agent.sh, update-persona.sh, doctor.sh, and migrate-image.sh.
#
# The persona destination path lives HERE and nowhere else fleet-side. It
# mirrors three upstream-INTERNAL derivations (entrypoint home default +
# profile storage subdir + a pub(crate) prompt-path constant), none of which
# upstream promises to keep; migrate-image.sh gates every pin bump
# on all three still holding in the pinned source, and doctor.sh's stray-seed
# sweep is the runtime backstop if they silently move.
# shellcheck shell=bash

# shellcheck disable=SC2034  # consumed by the sourcing scripts, not here
FLEET_PERSONA_DST="/data/ironclaw-reborn/hosted-single-tenant-volume/system/prompts/default-system.md"

# fleet_agent_env <slug> — path of the agent's recorded secrets/env file.
fleet_agent_env() { printf '%s/%s.env\n' "${MULTRON_SECRETS_DIR:-$HOME/.agency/agents}" "$1"; }

# fleet_install_persona <container> <composed-prompt> — write a composed persona to the one
# path. Both writers (provision + update) go through here so the install mechanism exists once.
fleet_install_persona() {
  printf '%s\n' "$2" | docker exec -i "$1" sh -c "cat > '$FLEET_PERSONA_DST'"
}

# fleet_container <slug> — the CONTAINER= value recorded at provision time (never re-derive
# when the record exists), else ironclaw-<slug>.
fleet_container() {
  local envf c=""
  envf="$(fleet_agent_env "$1")"
  if [ -f "$envf" ]; then c="$(fleet_env_get "$envf" CONTAINER)"; fi
  printf '%s\n' "${c:-ironclaw-$1}"
}

# The lib dir + repo root, resolved from THIS file rather than from $0, so a helper
# behaves the same whichever script sourced it and from whatever cwd.
_FLEET_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FLEET_REPO_ROOT="$(cd "$_FLEET_LIB_DIR/../.." && pwd)"

# curl_bearer/curl_tg live next door and fleet_wait_api needs them. Sourcing here is
# idempotent (these are function definitions) and by absolute path, so a caller no longer
# has to know about curl-private.sh *or* be in the right cwd to reach it.
. "$_FLEET_LIB_DIR/curl-private.sh"

# fleet_env_get <env-file> <key> — read KEY=VALUE from an agent/client env file.
#
# ONE quoting rule for the whole fleet: leading/trailing whitespace tolerated, both quote
# styles stripped. A stricter reader here once rejected a padded value, SKIPPED the account
# deletion it was gating, and exited 0 with the sealed account still alive.
#
# Absent key prints nothing and still returns 0, deliberately: callers test emptiness, and
# non-zero would abort the assignment under `set -e`. No grep either — sed returns 0 on no
# match.
fleet_env_get() {
  local _v=""
  if [ -f "$1" ]; then
    _v="$(sed -n -E "s/^[[:space:]]*$2=[[:space:]]*(.*)\$/\1/p" "$1" | head -n1)"
    _v="${_v%"${_v##*[![:space:]]}"}"                       # rstrip
    case "$_v" in
      \"*\") _v="${_v#\"}"; _v="${_v%\"}" ;;
      \'*\') _v="${_v#\'}"; _v="${_v%\'}" ;;
    esac
  fi
  printf '%s\n' "$_v"
}

# fleet_json <python-expr> — evaluate a Python expression against JSON on stdin; `d` is the
# parsed document. Loud on purpose: a malformed body should say why, so a caller that wants
# silence asks for it with `2>/dev/null`.
fleet_json() { python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }

# fleet_require_container <name> — hard-fail unless the container is running. `grep -qx`, not
# a substring match: `secretary-ironclaw-1` must not satisfy a check for `ironclaw-1`.
fleet_require_container() {
  docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1" || {
    echo "!! container is not running: $1" >&2; return 1; }
}

# fleet_wait_api <operator-token> <api-base> — block until the operator API answers. The
# failure clause lives here so it cannot be the thing a caller forgets: without it a
# never-ready instance falls through to being configured anyway.
fleet_wait_api() {
  local _tok="$1" _api="$2"
  curl_bearer "$_tok" -s -o /dev/null --retry 20 --retry-delay 2 --retry-connrefused \
    --retry-all-errors --max-time 10 "$_api/api/webchat/v2/admin/users" || {
      echo "!! operator API never became ready at $_api (retried 20x/2s)" >&2; return 1; }
}

# fleet_model_pin — the model of record, shell side.
#
# NO fallback literal, deliberately. MODEL_PIN is tracked, so an unreadable pin means a broken
# checkout; a default here would be the one value that can SILENTLY outrank the pin, quietly
# moving partner data onto a model with weaker privacy guarantees. `MODEL` env wins for a
# one-off.
fleet_model_pin() {
  local _pin
  [ -n "${MODEL:-}" ] && { printf '%s\n' "$MODEL"; return 0; }
  _pin="$(cut -d'#' -f1 < "$FLEET_REPO_ROOT/MODEL_PIN" | head -n1 | tr -d '[:space:]')"
  [ -n "$_pin" ] || {
    echo "!! $FLEET_REPO_ROOT/MODEL_PIN names no model on its first line" >&2; return 1; }
  printf '%s\n' "$_pin"
}

# Two resolvers, and picking the wrong one is silent:
#   fleet_mt_container_configured — CONFIGURED name only (MT_CONTAINER > compose
#     container_name > default). No live lookup, so it is the right one for a REMOTE target:
#     what runs on this laptop says nothing about what runs there.
#   fleet_mt_container — the same, then resolved against LOCAL reality. If the configured name
#     is not running but the legacy one is, trust reality; that covers the window after a new
#     compose syncs but before MT is recreated.
#
# A missing compose is a HARD ERROR, never a guess: a provenance gate that checks the wrong
# container certifies nothing. `|| true` on the grep is required — under pipefail an unmatched
# grep aborts the assignment, so the `:=` default would never run.
fleet_mt_container_configured() {
  local _compose="$FLEET_REPO_ROOT/multi/instance/docker-compose.yml" _name
  [ -n "${MT_CONTAINER:-}" ] && { printf '%s\n' "$MT_CONTAINER"; return 0; }
  [ -f "$_compose" ] || {
    echo "!! MT compose not found: $_compose — cannot derive the container name" >&2; return 1; }
  _name="$(grep -E '^[[:space:]]*container_name:[[:space:]]' "$_compose" | head -1 | awk '{print $2}' || true)"
  printf '%s\n' "${_name:-multi-ironclaw-1}"
}

fleet_mt_container() {
  local _name
  _name="$(fleet_mt_container_configured)" || return 1
  if ! docker inspect "$_name" >/dev/null 2>&1 && docker inspect multi-ironclaw-1 >/dev/null 2>&1; then
    _name=multi-ironclaw-1
  fi
  printf '%s\n' "$_name"
}

# fleet_wait_health <url> [attempts] [compose-service] — poll a /health endpoint, then dump that
# service's compose logs and fail if it never came up.
fleet_wait_health() {
  local _url="$1" _n="${2:-30}" _svc="${3:-account-service}" i
  for i in $(seq 1 "$_n"); do
    if curl -sf "$_url" >/dev/null 2>&1; then echo "  healthy (attempt $i)"; return 0; fi
    sleep 2
  done
  echo "!! service did not become healthy at $_url" >&2
  docker compose logs --tail=40 "$_svc" || true
  return 1
}
