#!/usr/bin/env bash
# Fail a served client closed unless its host-owned identity and Docker endpoint agree with the
# systemd instance. This runs as ironclaw-<client> immediately before `ironclaw serve`.
set -euo pipefail

client="${1:-}"
case "$client" in
  ""|*[!a-z0-9-]*|-*|*-) echo "invalid client service slug: $client" >&2; exit 1 ;;
esac

expected_home="/var/lib/ironclaw/$client"
expected_socket="/run/user/$(id -u)/docker.sock"

[ "${IRONCLAW_REBORN_HOME:-}" = "$expected_home" ] || {
  echo "IRONCLAW_REBORN_HOME does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_WORKSPACE_ROOT:-}" = "$expected_home/workspace" ] || {
  echo "IRONCLAW_REBORN_WORKSPACE_ROOT does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_DOCKER_HOST:-}" = "unix://$expected_socket" ] || {
  echo "IRONCLAW_REBORN_DOCKER_HOST does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_WEBUI_USER_ID:-}" = "$client" ] || {
  echo "IRONCLAW_REBORN_WEBUI_USER_ID must equal service instance $client" >&2; exit 1; }

case " $(id -Gn) " in
  *" docker "*) echo "ironclaw-$client must not belong to the shared docker group" >&2; exit 1 ;;
esac

tenant="$(/opt/ironclaw/bin/ironclaw config get identity.tenant)"
owner="$(/opt/ironclaw/bin/ironclaw config get identity.default_owner)"
[ "$tenant" = "$client" ] || {
  echo "[identity].tenant must equal service instance $client" >&2; exit 1; }
[ "$owner" = "$client" ] || {
  echo "[identity].default_owner must equal service instance $client" >&2; exit 1; }

[ -S "$expected_socket" ] || {
  echo "client rootless Docker socket is absent: $expected_socket" >&2; exit 1; }
[ -O "$expected_socket" ] || {
  echo "client rootless Docker socket is not owned by ironclaw-$client" >&2; exit 1; }
