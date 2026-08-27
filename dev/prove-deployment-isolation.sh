#!/usr/bin/env bash
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
set -uo pipefail

left="${1:-}"
right="${2:-}"
sentinel="ironworks-isolation-proof-$$"
created_left=0
created_right=0
pass=0
fail=0
blocked=0

verdict() {
  printf '%-9s %-34s %s\n' "$2" "$1" "${3:-}"
  case "$2" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) fail=$((fail + 1)) ;;
    BLOCKED) blocked=$((blocked + 1)) ;;
  esac
}

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

cleanup() {
  [ "$created_left" -eq 0 ] || as_client "$left" docker rm -f "$sentinel" >/dev/null 2>&1
  [ "$created_right" -eq 0 ] || as_client "$right" docker rm -f "$sentinel" >/dev/null 2>&1
}
trap cleanup EXIT

[ "${EUID:-$(id -u)}" -eq 0 ] || {
  verdict "authority" BLOCKED "run as root so both service identities can be exercised"
  exit 3
}
valid_slug "$left" && valid_slug "$right" && [ "$left" != "$right" ] || {
  verdict "arguments" BLOCKED "supply two distinct lowercase client slugs"
  exit 3
}
if ! command -v sudo >/dev/null || ! command -v docker >/dev/null \
  || ! command -v systemctl >/dev/null; then
  verdict "tools" BLOCKED "sudo, systemctl and the Docker CLI are required"
  exit 3
fi
[ -x /opt/ironclaw/bin/ironclaw ] || {
  verdict "ironclaw.binary" BLOCKED "/opt/ironclaw/bin/ironclaw is absent"
  exit 3
}

for client in "$left" "$right"; do
  id "ironclaw-$client" >/dev/null 2>&1 || {
    verdict "$client.user" BLOCKED "service user does not exist"
    continue
  }
  systemctl is-active --quiet "rustworks-ironclaw@$client.service" || {
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
  as_client "$client" docker image inspect ironclaw-worker:latest >/dev/null 2>&1 || {
    verdict "$client.worker-image" BLOCKED "ironclaw-worker:latest is absent from its daemon"
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
  printf '\nBLOCKED — prerequisites prevented the two-daemon measurement.\n'
  exit 3
fi

as_client "$left" docker create --name "$sentinel" --entrypoint /bin/true \
  ironclaw-worker:latest >/dev/null 2>&1 || {
  verdict "$left.sentinel" BLOCKED "could not create the positive control"
  exit 3
}
created_left=1
verdict "$left.sentinel" PASS "$sentinel exists in $left daemon"

as_client "$right" docker create --name "$sentinel" --entrypoint /bin/true \
  ironclaw-worker:latest >/dev/null 2>&1 || {
  verdict "$right.sentinel" FAIL "same name collided or create failed in $right daemon"
  exit 2
}
created_right=1
verdict "$right.sentinel" PASS "same name exists independently in $right daemon"

if sudo -u "ironclaw-$left" env \
  DOCKER_HOST="unix:///run/user/$(id -u "ironclaw-$right")/docker.sock" \
  docker info >/dev/null 2>&1; then
  verdict "$left.cross-socket" FAIL "$left can control $right daemon"
else
  verdict "$left.cross-socket" PASS "$right socket denied"
fi

if sudo -u "ironclaw-$right" env \
  DOCKER_HOST="unix:///run/user/$(id -u "ironclaw-$left")/docker.sock" \
  docker info >/dev/null 2>&1; then
  verdict "$right.cross-socket" FAIL "$right can control $left daemon"
else
  verdict "$right.cross-socket" PASS "$left socket denied"
fi

printf '\n%d PASS, %d FAIL, %d BLOCKED\n' "$pass" "$fail" "$blocked"
[ "$fail" -eq 0 ] || exit 2
printf 'PASS — distinct identities and Docker authority boundaries were measured.\n'
