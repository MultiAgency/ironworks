#!/usr/bin/env bash
# COST: free — offline fixture checks.
# Offline fixture checks for the served artifact preflight and direct systemd command.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/ironclaw-artifact-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
fixture_uid="$(id -u)"

# shellcheck disable=SC2016 # These variables belong to the generated fixture executables.
printf '#!/usr/bin/env bash\nprintf "%%s|%%s\\n" "${IRONCLAW_REBORN_SANDBOX_IMAGE:-}" "$*"\n' > "$tmp/ironclaw"
chmod 0755 "$tmp/ironclaw"
printf 'worker archive fixture\n' > "$tmp/worker.tar"
revision="$(sed -n '1{s/#.*//;s/[[:space:]]//g;p;}' "$root/IRONCLAW_PIN")"
binary_sha="$(sha256sum "$tmp/ironclaw" | awk '{print $1}')"
archive_sha="$(sha256sum "$tmp/worker.tar" | awk '{print $1}')"
image_id="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
{
  printf 'IRONCLAW_ARTIFACT_REVISION=%s\n' "$revision"
  printf 'IRONCLAW_BINARY_SHA256=%s\n' "$binary_sha"
  printf 'IRONCLAW_WORKER_ARCHIVE_SHA256=%s\n' "$archive_sha"
  printf 'IRONCLAW_REBORN_SANDBOX_IMAGE=%s\n' "$image_id"
} > "$tmp/manifest.env"

# shellcheck disable=SC2016 # Expanded when the generated Docker fixture is invoked.
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$MOCK_DOCKER_IMAGE_ID"\n' > "$tmp/bin/docker"
chmod 0755 "$tmp/bin/docker"
set -a
# shellcheck source=/dev/null
. "$tmp/manifest.env"
set +a
export IRONCLAW_REBORN_DOCKER_HOST="unix:///fixture/docker.sock"
export MOCK_DOCKER_IMAGE_ID="$image_id"
if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/absent.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "missing manifest was accepted" >&2
  exit 1
fi
PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/manifest.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid"

if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/manifest.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" \
  "$((fixture_uid + 1))" >/dev/null 2>&1; then
  echo "artifacts owned by a different uid were accepted" >&2
  exit 1
fi

sed 's/^IRONCLAW_BINARY_SHA256=.*/IRONCLAW_BINARY_SHA256=not-a-sha256/' \
  "$tmp/manifest.env" > "$tmp/malformed.env"
if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/malformed.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "malformed manifest SHA-256 was accepted" >&2
  exit 1
fi

sed 's/^IRONCLAW_REBORN_SANDBOX_IMAGE=.*/IRONCLAW_REBORN_SANDBOX_IMAGE=ironclaw-worker:latest/' \
  "$tmp/manifest.env" > "$tmp/mutable-image.env"
if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/mutable-image.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "mutable worker image name was accepted in the manifest" >&2
  exit 1
fi

other_image_id="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
if IRONCLAW_REBORN_SANDBOX_IMAGE="$other_image_id" PATH="$tmp/bin:$PATH" \
  "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/manifest.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "systemd environment differing from the manifest was accepted" >&2
  exit 1
fi

chmod 0664 "$tmp/manifest.env"
if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/manifest.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "group-writable artifact manifest was accepted" >&2
  exit 1
fi
chmod 0644 "$tmp/manifest.env"
mv "$tmp/manifest.env" "$tmp/manifest.real"
ln -s "$tmp/manifest.real" "$tmp/manifest.env"
if PATH="$tmp/bin:$PATH" "$root/deploy/check-ironclaw-artifacts.sh" \
  "$tmp/manifest.env" "$root/IRONCLAW_PIN" "$tmp/ironclaw" "$fixture_uid" \
  >/dev/null 2>&1; then
  echo "symlinked artifact manifest was accepted as a regular file" >&2
  exit 1
fi
rm "$tmp/manifest.env"
mv "$tmp/manifest.real" "$tmp/manifest.env"

unit="$root/deploy/ironworks-ironclaw@.service"
client_env_line="$(awk '/^EnvironmentFile=\/etc\/ironworks\/ironclaw-%i.env$/ { print NR }' "$unit")"
artifact_env_line="$(awk '/^EnvironmentFile=\/opt\/ironclaw\/artifact-manifest.env$/ { print NR }' "$unit")"
[ -n "$client_env_line" ] && [ -n "$artifact_env_line" ] \
  && [ "$artifact_env_line" -gt "$client_env_line" ] || {
  echo "artifact manifest must override the per-client environment" >&2
  exit 1
}
grep -q '^ExecStartPre=/opt/ironworks/deploy/check-ironclaw-artifacts.sh$' "$unit" || {
  echo "artifact preflight is absent from the IronClaw unit" >&2
  exit 1
}
# shellcheck disable=SC2016 # These assert literal production variables.
grep -q '^secure_file "/etc/ironworks/ironclaw-\$client.env" 0 "client environment"$' \
  "$root/deploy/check-ironclaw-instance.sh" || {
  echo "instance preflight does not require a root-owned client environment" >&2
  exit 1
}
# shellcheck disable=SC2016 # These assert literal production variables.
grep -q '^secure_file "\$expected_home/config.toml" "\$client_uid" "client config"$' \
  "$root/deploy/check-ironclaw-instance.sh" || {
  echo "instance preflight does not require a client-owned config" >&2
  exit 1
}
# shellcheck disable=SC2016 # The unit, not this test shell, expands IRONCLAW_PORT.
grep -q '^ExecStart=/opt/ironclaw/bin/ironclaw serve --host 127\.0\.0\.1 --port ${IRONCLAW_PORT}$' "$unit" || {
  echo "IronClaw unit does not execute the installed binary directly" >&2
  exit 1
}
grep -q 'IRONCLAW_PORT must be an integer from 1 through 65535' \
  "$root/deploy/check-ironclaw-instance.sh" || {
  echo "instance preflight does not validate the listening port" >&2
  exit 1
}

# The booted composition profile selects which constructor resolves the runtime policy, so the
# unit and its preflight must name one string, not two that happen to agree today.
unit_profile="$(sed -n 's/^Environment=IRONCLAW_REBORN_PROFILE=//p' "$unit")"
preflight_profile="$(sed -n \
  's/^\[ "\${IRONCLAW_REBORN_PROFILE:-}" = "\(.*\)" \] || {$/\1/p' \
  "$root/deploy/check-ironclaw-instance.sh")"
[ -n "$unit_profile" ] || {
  echo "IronClaw unit does not set IRONCLAW_REBORN_PROFILE" >&2
  exit 1
}
[ -n "$preflight_profile" ] || {
  echo "instance preflight does not require a composition profile" >&2
  exit 1
}
[ "$unit_profile" = "$preflight_profile" ] || {
  echo "unit profile '$unit_profile' and preflight profile '$preflight_profile' disagree" >&2
  exit 1
}

echo "artifact fixture checks passed"
