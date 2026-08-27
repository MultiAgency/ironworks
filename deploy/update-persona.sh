#!/usr/bin/env bash
# Reinstall an agent's persona from the repo — the SAME compose as provisioning
# (deploy/lib/compose-persona: leading-header strip, first-H1 drop, literal
# {{slot}} fill, sentinel stamp), then restart the container so the new system
# prompt takes effect.
#
#   ./deploy/update-persona.sh multimediator agent/identity/MULTIMEDIATOR.md
#
# {{AGENT_NAME}}/{{PURPOSE}} slot values come from the agent's recorded
# ~/.agency/agents/<slug>.env (provision-agent.sh writes them), overridable by
# AGENT_NAME=/PURPOSE= in the environment. The container name comes from the
# recorded CONTAINER= value, falling back to ironclaw-<slug>. A persona
# that still needs a slot nobody supplied fails loudly instead of installing
# a literal {{AGENT_NAME}} into the live prompt.
set -euo pipefail
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO_DIR/deploy/lib/fleet.sh"
SLUG="${1:?usage: update-persona.sh <slug> <persona-file>}"
PERSONA="${2:?usage: update-persona.sh <slug> <persona-file>}"
case "$PERSONA" in /*) ;; *) PERSONA="$REPO_DIR/$PERSONA" ;; esac
[ -f "$PERSONA" ] || { echo "!! persona not found: $PERSONA" >&2; exit 1; }
C="$(fleet_container "$SLUG")"
envf="$(fleet_agent_env "$SLUG")"

recorded() {  # recorded <VAR> — value from the agent env file, or empty
  [ -f "$envf" ] || return 0
  # shellcheck disable=SC1090  # per-agent env path is dynamic by design
  ( set +eu; unset AGENT_NAME PURPOSE; . "$envf" 2>/dev/null
    eval "printf '%s' \"\${$1:-}\"" )
}
AGENT_NAME="${AGENT_NAME:-$(recorded AGENT_NAME)}"
PURPOSE="${PURPOSE:-$(recorded PURPOSE)}"
slot_args=()
[ -n "$AGENT_NAME" ] && slot_args+=(--slot "AGENT_NAME=$AGENT_NAME")
[ -n "$PURPOSE" ] && slot_args+=(--slot "PURPOSE=$PURPOSE")

# compose fully BEFORE touching the container — piping compose straight into
# `cat > DST` would truncate the live prompt even when compose fails
persona_prompt="$("$REPO_DIR/deploy/lib/compose-persona" compose \
  --persona "$PERSONA" --tail "$REPO_DIR/agent/identity/_operational-tail.md" \
  --slug "$SLUG" ${slot_args[@]+"${slot_args[@]}"})"
fleet_install_persona "$C" "$persona_prompt"
docker restart "$C" >/dev/null
echo "persona $(basename "$PERSONA") installed into $C and restarted"
