#!/usr/bin/env bash
# Fail closed unless installed bytes and this client's daemon match the root-owned build manifest.
set -euo pipefail

manifest="${1:-/opt/ironclaw/artifact-manifest.env}"
pin="${2:-/opt/ironworks/IRONCLAW_PIN}"
binary="${3:-/opt/ironclaw/bin/ironclaw}"
# Tests can name their fixture owner's numeric uid. Production supplies no fourth argument and
# therefore requires every shared artifact and the repository pin to remain root-owned.
expected_uid="${4:-0}"

fail() { echo "IronClaw artifact check: $*" >&2; exit 1; }
[[ "$expected_uid" =~ ^[0-9]+$ ]] || fail "expected artifact owner uid is malformed"
file_uid() { stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1"; }
file_mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }
secure_file() {
  local path="$1" label="$2" mode owner
  [ -f "$path" ] && [ ! -L "$path" ] || fail "$label is absent or not a regular file: $path"
  [ -r "$path" ] || fail "$label is unreadable: $path"
  owner="$(file_uid "$path")" || fail "cannot read $label ownership: $path"
  [ "$owner" = "$expected_uid" ] || fail "$label is not owned by expected uid $expected_uid"
  mode="$(file_mode "$path")" || fail "cannot read $label mode: $path"
  (( (8#$mode & 0022) == 0 )) || fail "$label is writable by group or other"
}
secure_file "$manifest" "manifest"
secure_file "$pin" "IRONCLAW_PIN"
secure_file "$binary" "installed binary"
[ -x "$binary" ] || fail "installed binary is not executable"

# One comparison, not three: the field names in order are exactly the four the builder writes.
# That is at once the line count, the one-of-each rule and the no-unknown-field rule, and unlike
# `wc -l` it also sees a final line the builder did not terminate.
expected_keys='IRONCLAW_ARTIFACT_REVISION
IRONCLAW_BINARY_SHA256
IRONCLAW_WORKER_ARCHIVE_SHA256
IRONCLAW_REBORN_SANDBOX_IMAGE'
[ "$(awk -F= '{ print $1 }' "$manifest")" = "$expected_keys" ] \
  || fail "manifest is not exactly the four expected fields, one each, in the builder's order"

manifest_value() {
  awk -F= -v key="$1" '$1 == key { print substr($0, length(key) + 2) }' "$manifest"
}
revision="$(manifest_value IRONCLAW_ARTIFACT_REVISION)"
binary_sha="$(manifest_value IRONCLAW_BINARY_SHA256)"
archive_sha="$(manifest_value IRONCLAW_WORKER_ARCHIVE_SHA256)"
image_id="$(manifest_value IRONCLAW_REBORN_SANDBOX_IMAGE)"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || fail "manifest revision is not a full lowercase Git revision"
[[ "$binary_sha" =~ ^[0-9a-f]{64}$ ]] || fail "manifest binary SHA-256 is malformed"
[[ "$archive_sha" =~ ^[0-9a-f]{64}$ ]] || fail "manifest archive SHA-256 is malformed"
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail "manifest worker image ID is malformed"

pinned="$(sed -n '1{s/#.*//;s/[[:space:]]//g;p;}' "$pin")"
[ "$revision" = "$pinned" ] || fail "manifest revision does not equal IRONCLAW_PIN"
[ "$(sha256sum "$binary" | awk '{print $1}')" = "$binary_sha" ] || fail "installed binary SHA-256 does not match manifest"

[ "${IRONCLAW_REBORN_SANDBOX_IMAGE:-}" = "$image_id" ] \
  || fail "IronClaw worker image environment does not match manifest"
[ -n "${IRONCLAW_REBORN_DOCKER_HOST:-}" ] || fail "IRONCLAW_REBORN_DOCKER_HOST is unset"
command -v docker >/dev/null 2>&1 || fail "Docker CLI is unavailable"
daemon_id="$(docker --host "$IRONCLAW_REBORN_DOCKER_HOST" image inspect --format '{{.Id}}' "$image_id" 2>/dev/null)" \
  || fail "manifest worker image is absent from this client's Docker daemon"
[ "$daemon_id" = "$image_id" ] || fail "client daemon resolved a different worker image ID"
