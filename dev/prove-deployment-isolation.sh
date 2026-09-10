#!/usr/bin/env bash
# COST: root on a deployment host, against live per-client Docker daemons.
# Prove the two deployment conditions that must hold before a second sandbox-backed tenant:
# explicit, distinct IronClaw identities and mutually inaccessible per-client Docker daemons.
#
# Run as root on the deployment host after both client user daemons and control planes are up:
#
#   dev/prove-deployment-isolation.sh acme globex
#
# The proof creates one stopped, same-named sentinel in each private daemon and removes both on
# exit. Same-name success is the positive control that the calls reached distinct object
# namespaces; cross-socket denial measures the Unix authority boundary.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

left="${1:-}"
right="${2:-}"
sentinel="ironworks-isolation-proof-$$"
created_left=0
created_right=0

valid_slug() {
  case "$1" in ""|*[!a-z0-9-]*|-*|*-) return 1 ;; *) return 0 ;; esac
}

as_client() {
  local client="$1"
  shift
  sudo -u "ironclaw-$client" env \
    HOME="/var/lib/ironclaw/$client" \
    XDG_RUNTIME_DIR="/run/user/$(id -u "ironclaw-$client")" \
    DOCKER_HOST="unix:///run/user/$(id -u "ironclaw-$client")/docker.sock" \
    "$@"
}

config_value() {
  local client="$1" key="$2"
  sudo -u "ironclaw-$client" env \
    HOME="/var/lib/ironclaw/$client" \
    IRONCLAW_REBORN_HOME="/var/lib/ironclaw/$client" \
    /opt/ironclaw/bin/ironclaw config get "$key"
}

# shellcheck disable=SC2329 # Invoked by the EXIT trap below.
cleanup() {
  [ "$created_left" -eq 0 ] || as_client "$left" docker rm -f "$sentinel" >/dev/null 2>&1
  [ "$created_right" -eq 0 ] || as_client "$right" docker rm -f "$sentinel" >/dev/null 2>&1
}
on_exit cleanup

[ "${EUID:-$(id -u)}" -eq 0 ] || {
  give_up "authority" "run as root so both service identities can be exercised";
}
valid_slug "$left" && valid_slug "$right" && [ "$left" != "$right" ] || {
  give_up "arguments" "supply two distinct lowercase client slugs";
}
if ! command -v sudo >/dev/null || ! command -v docker >/dev/null \
  || ! command -v systemctl >/dev/null; then
  give_up "tools" "sudo, systemctl and the Docker CLI are required"
fi
[ -x /opt/ironclaw/bin/ironclaw ] || {
  give_up "ironclaw.binary" "/opt/ironclaw/bin/ironclaw is absent";
}
artifact_manifest=/opt/ironclaw/artifact-manifest.env
[ -r "$artifact_manifest" ] || {
  give_up "ironclaw.manifest" "$artifact_manifest is absent or unreadable";
}
image_rows="$(awk -F= '$1 == "IRONCLAW_REBORN_SANDBOX_IMAGE" { count++; value=substr($0, length($1) + 2) } END { if (count == 1) print value }' "$artifact_manifest")"
[[ "$image_rows" =~ ^sha256:[0-9a-f]{64}$ ]] || {
  give_up "ironclaw.worker-image" "manifest has no single immutable worker image ID";
}
worker_image="$image_rows"

for client in "$left" "$right"; do
  id "ironclaw-$client" >/dev/null 2>&1 || {
    verdict "$client.user" BLOCKED "service user does not exist"
    continue
  }
  systemctl is-active --quiet "ironworks-ironclaw@$client.service" || {
    verdict "$client.control-plane" BLOCKED "IronClaw service is not active"
    continue
  }
  [ -S "/run/user/$(id -u "ironclaw-$client")/docker.sock" ] || {
    verdict "$client.daemon" BLOCKED "private Docker socket is absent"
    continue
  }
  as_client "$client" docker info >/dev/null 2>&1 || {
    verdict "$client.daemon" BLOCKED "own daemon did not answer"
    continue
  }
  as_client "$client" docker image inspect "$worker_image" >/dev/null 2>&1 || {
    verdict "$client.worker-image" BLOCKED "$worker_image is absent from its daemon"
    continue
  }
  daemon_security="$(as_client "$client" docker info \
    --format '{{json .SecurityOptions}} {{.CgroupDriver}}' 2>/dev/null)" || daemon_security=""
  case "$daemon_security" in
    *rootless*\ systemd) verdict "$client.daemon-mode" PASS "rootless with systemd cgroups" ;;
    "") verdict "$client.daemon-mode" BLOCKED "daemon mode could not be read" ;;
    *)
      verdict "$client.daemon-mode" FAIL \
        "expected rootless with systemd cgroups: $daemon_security"
      ;;
  esac
  case " $(id -Gn "ironclaw-$client") " in
    *" docker "*) verdict "$client.docker-group" FAIL "service user belongs to shared docker group" ;;
    *) verdict "$client.docker-group" PASS "no shared docker-group authority" ;;
  esac

  tenant="$(config_value "$client" identity.tenant 2>/dev/null)" || tenant=""
  owner="$(config_value "$client" identity.default_owner 2>/dev/null)" || owner=""
  if [ "$tenant" = "$client" ] && [ "$owner" = "$client" ]; then
    verdict "$client.identity" PASS "tenant=$tenant owner=$owner"
  elif [ -z "$tenant" ] || [ -z "$owner" ]; then
    verdict "$client.identity" BLOCKED "identity config could not be read"
  else
    verdict "$client.identity" FAIL "tenant=$tenant owner=$owner; expected $client"
  fi
done

if [ "$blocked" -gt 0 ]; then
  summary 'prerequisites prevented the two-daemon measurement.'
fi

as_client "$left" docker create --name "$sentinel" --entrypoint /bin/true \
  "$worker_image" >/dev/null 2>&1 || {
  give_up "$left.sentinel" "could not create the positive control";
}
created_left=1
verdict "$left.sentinel" PASS "$sentinel exists in $left daemon"

as_client "$right" docker create --name "$sentinel" --entrypoint /bin/true \
  "$worker_image" >/dev/null 2>&1 || {
  verdict "$right.sentinel" FAIL "same name collided or create failed in $right daemon"
  # `summary` rather than `exit 2`: the sentinel below is the positive control every remaining
  # check reads, so the run stops — but only `summary` lets an earlier BLOCKED outrank this FAIL.
  summary 'the sentinel is the control the remaining checks read, so the run stops here.'
}
created_right=1
verdict "$right.sentinel" PASS "same name exists independently in $right daemon"

cross_socket() {
  local attacker="$1" target="$2" output result detail

  # The negative attempt is useful only while the attacker can reach its own control and the
  # target's owning identity can demonstrably reach the target daemon and sentinel. Check the
  # target immediately on both sides so that daemon disappearing mid-attempt cannot look like
  # isolation.
  if ! as_client "$attacker" docker container inspect "$sentinel" >/dev/null 2>&1; then
    verdict "$attacker.cross-socket" BLOCKED \
      "attacker's own sentinel was unavailable before the target-socket attempt"
    return
  fi
  if ! as_client "$target" docker container inspect "$sentinel" >/dev/null 2>&1; then
    verdict "$attacker.cross-socket" BLOCKED \
      "target owner could not read its sentinel immediately before the cross-socket attempt"
    return
  fi

  if output="$(sudo -u "ironclaw-$attacker" env \
    DOCKER_HOST="unix:///run/user/$(id -u "ironclaw-$target")/docker.sock" \
    docker info 2>&1)"; then
    result=FAIL
    detail="$attacker can control $target daemon"
  elif grep -qiE 'permission denied|operation not permitted|access denied|authorization denied|unauthorized|forbidden' \
    <<<"$output"; then
    result=PASS
    detail="$target socket returned a permission or authorization denial"
  else
    result=BLOCKED
    detail="$target socket failed without a classified permission or authorization denial"
  fi

  if ! as_client "$target" docker container inspect "$sentinel" >/dev/null 2>&1; then
    if [ "$result" = FAIL ]; then
      verdict "$attacker.cross-socket" FAIL "$detail; target sentinel then became unavailable"
    else
      verdict "$attacker.cross-socket" BLOCKED \
        "target owner could not read its sentinel immediately after the cross-socket attempt"
    fi
    return
  fi
  verdict "$attacker.cross-socket" "$result" "$detail"
}

cross_socket "$left" "$right"
cross_socket "$right" "$left"

summary 'distinct identities and Docker authority boundaries were measured.'
