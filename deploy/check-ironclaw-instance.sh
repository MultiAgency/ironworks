#!/usr/bin/env bash
# Fail a served client closed unless its host-owned identity, Docker endpoint and booted
# composition profile agree with the systemd instance. This runs as ironclaw-<client> immediately
# before `ironclaw serve`.
#
# The profile is checked because it is not a preference: it selects which constructor resolves the
# runtime policy this deployment runs under, and only one of IronClaw's four boot arms reads that
# policy from configuration at all. DESIGN.md's upstream contract carries the reading.
# dev/test-ironclaw-artifacts.sh asserts this expectation and the unit's Environment= line are the
# same string, so the two cannot drift apart.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: check-ironclaw-instance.sh CLIENT" >&2
  exit 64
fi
client="$1"
case "$client" in
  ""|*[!a-z0-9-]*|-*|*-) echo "invalid client service slug: $client" >&2; exit 1 ;;
esac

expected_home="/var/lib/ironclaw/$client"
expected_socket="/run/user/$(id -u)/docker.sock"
client_uid="$(id -u)"

file_uid() { stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1"; }
file_mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }
secure_file() {
  local path="$1" expected_uid="$2" label="$3" mode owner
  [ -f "$path" ] && [ ! -L "$path" ] || {
    echo "$label is absent or not a regular file: $path" >&2; exit 1; }
  owner="$(file_uid "$path")" || {
    echo "cannot read $label ownership: $path" >&2; exit 1; }
  [ "$owner" = "$expected_uid" ] || {
    echo "$label is not owned by expected uid $expected_uid" >&2; exit 1; }
  mode="$(file_mode "$path")" || {
    echo "cannot read $label mode: $path" >&2; exit 1; }
  (( (8#$mode & 0022) == 0 )) || {
    echo "$label is writable by group or other" >&2; exit 1; }
}

secure_file "/etc/ironworks/ironclaw-$client.env" 0 "client environment"
secure_file "$expected_home/config.toml" "$client_uid" "client config"

port="${IRONCLAW_PORT:-}"
if ! [[ "$port" =~ ^[1-9][0-9]{0,4}$ ]] || (( 10#$port > 65535 )); then
  echo "IRONCLAW_PORT must be an integer from 1 through 65535" >&2
  exit 1
fi

[ "${IRONCLAW_REBORN_HOME:-}" = "$expected_home" ] || {
  echo "IRONCLAW_REBORN_HOME does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_WORKSPACE_ROOT:-}" = "$expected_home/workspace" ] || {
  echo "IRONCLAW_REBORN_WORKSPACE_ROOT does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_DOCKER_HOST:-}" = "unix://$expected_socket" ] || {
  echo "IRONCLAW_REBORN_DOCKER_HOST does not match service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_WEBUI_USER_ID:-}" = "$client" ] || {
  echo "IRONCLAW_REBORN_WEBUI_USER_ID must equal service instance $client" >&2; exit 1; }
[ "${IRONCLAW_REBORN_PROFILE:-}" = "hosted-single-tenant-volume-sandboxed" ] || {
  echo "IRONCLAW_REBORN_PROFILE must be hosted-single-tenant-volume-sandboxed" >&2; exit 1; }

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
