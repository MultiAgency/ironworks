#!/usr/bin/env bash
# Disposable cold-boot proof for the IronClaw 1.4 persistent-workspace contract.
#
# This shares no container, volume, network, port, database, or credential with production. It
# proves both canonical profiles with the exact pinned image and the same Linux security options
# committed in their Compose definitions, then deletes every disposable object it created.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO/deploy/lib/fleet.sh"

PIN="$(fleet_ironclaw_pin)"
IMAGE="${IRONCLAW_BOOT_IMAGE:-ironclaw:${PIN:0:9}}"
SUFFIX="$$"
NET="iw-workspace-proof-$SUFFIX"
PG="iw-workspace-proof-pg-$SUFFIX"
MT="iw-workspace-proof-mt-$SUFFIX"
SECRETARY="iw-workspace-proof-secretary-$SUFFIX"
MT_VOL="iw-workspace-proof-mt-data-$SUFFIX"
SECRETARY_VOL="iw-workspace-proof-secretary-data-$SUFFIX"
PGPW="workspace-proof-postgres"
MASTER_KEY="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" # gitleaks:allow — deterministic disposable proof input
TOKEN="abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789" # gitleaks:allow — deterministic disposable proof input

cleanup() {
  docker rm -f "$MT" "$SECRETARY" "$PG" >/dev/null 2>&1 || true
  docker volume rm "$MT_VOL" "$SECRETARY_VOL" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT HUP INT TERM

fail() { echo "!! $*" >&2; exit 1; }

image_rev="$(docker image inspect "$IMAGE" --format '{{index .Config.Labels "ironclaw.rev"}}')"
[ "$image_rev" = "$PIN" ] || fail "$IMAGE rev is $image_rev, expected $PIN"
version="$(docker run --rm --entrypoint /usr/local/bin/ironclaw "$IMAGE" --version)"
[ "$version" = "ironclaw 1.4.0" ] || fail "$IMAGE reports $version"

docker network create --internal "$NET" >/dev/null
docker volume create "$MT_VOL" >/dev/null
docker volume create "$SECRETARY_VOL" >/dev/null

assert_workspace_absent() {
  local volume=$1
  docker run --rm --network none --read-only --entrypoint /usr/bin/test \
    -v "$volume:/data" "$IMAGE" ! -e "$FLEET_WORKSPACE_ROOT"
}

assert_workspace() {
  local volume=$1 got
  got="$(docker run --rm --network none --read-only --entrypoint /usr/bin/stat \
    -v "$volume:/data" "$IMAGE" -c '%u:%g:%a' "$FLEET_WORKSPACE_ROOT")"
  [ "$got" = "1000:1000:755" ] || fail "$volume workspace ownership/mode is $got"
}

for volume in "$MT_VOL" "$SECRETARY_VOL"; do
  assert_workspace_absent "$volume"
  fleet_prepare_workspace "$IMAGE" "$volume"
  assert_workspace "$volume"
done

docker run -d --name "$PG" --network "$NET" \
  -e POSTGRES_PASSWORD="$PGPW" -e POSTGRES_DB=ironclaw \
  --tmpfs /var/lib/postgresql/data \
  --security-opt no-new-privileges:true \
  --cap-drop ALL --cap-add CHOWN --cap-add DAC_OVERRIDE --cap-add FOWNER \
  --cap-add SETGID --cap-add SETUID \
  postgres:16-alpine@sha256:57c72fd2a128e416c7fcc499958864df5301e940bca0a56f58fddf30ffc07777 >/dev/null

pg_ready=0
for _ in $(seq 1 60); do
  if docker exec "$PG" pg_isready -U postgres -d ironclaw >/dev/null 2>&1; then
    sleep 2
    if docker exec "$PG" pg_isready -U postgres -d ironclaw >/dev/null 2>&1; then
      pg_ready=1
      break
    fi
  fi
  sleep 1
done
[ "$pg_ready" -eq 1 ] || fail "disposable Postgres did not become ready"

runtime_security_args=(
  --security-opt no-new-privileges:true
  --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID
)

docker run -d --name "$MT" --network "$NET" "${runtime_security_args[@]}" \
  -v "$MT_VOL:/data" \
  -e IRONCLAW_REBORN_PROFILE=production \
  -e "IRONCLAW_REBORN_POSTGRES_URL=postgres://postgres:$PGPW@$PG:5432/ironclaw" \
  -e DATABASE_SSLMODE=disable \
  -e IRONCLAW_REBORN_ALLOW_REMOTE_POSTGRES_CLEAR_TEXT=true \
  -e IRONCLAW_REBORN_SECRET_MASTER_KEY="$MASTER_KEY" \
  -e IRONCLAW_REBORN_WEBUI_TOKEN="$TOKEN" \
  -e IRONCLAW_REBORN_WEBUI_USER_ID=reborn-cli \
  -e NEARAI_API_KEY=disposable-proof-key \
  -e NEARAI_BASE_URL=https://cloud-api.near.ai \
  -e IRONCLAW_REBORN_SERVE_HOST=0.0.0.0 \
  -e IRONCLAW_REBORN_POSTGRES_RESOURCE_GOVERNOR_SINGLETON=true \
  "$IMAGE" >/dev/null

docker run -d --name "$SECRETARY" --network "$NET" "${runtime_security_args[@]}" \
  -v "$SECRETARY_VOL:/data" \
  -e IRONCLAW_REBORN_PROFILE=hosted-single-tenant-volume \
  -e IRONCLAW_REBORN_WEBUI_TOKEN="$TOKEN" \
  -e IRONCLAW_REBORN_WEBUI_USER_ID=reborn-cli \
  -e NEARAI_API_KEY=disposable-proof-key \
  -e NEARAI_BASE_URL=https://cloud-api.near.ai \
  -e IRONCLAW_REBORN_SERVE_HOST=0.0.0.0 \
  "$IMAGE" >/dev/null

wait_runtime() {
  local container=$1 ready=0
  for _ in $(seq 1 90); do
    # The 1.4 MT profile serves /api/health before assembly. The authenticated admin route is
    # the second condition so this cannot certify only the startup listener.
    if docker exec "$container" curl -fsS http://127.0.0.1:3000/api/health >/dev/null 2>&1 \
      && docker exec "$container" curl -fsS -H "Authorization: Bearer $TOKEN" \
        http://127.0.0.1:3000/api/webchat/v2/admin/users >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 2
  done
  if [ "$ready" -ne 1 ]; then
    docker logs "$container" 2>&1 | tail -n 100 >&2 || true
    fail "$container did not complete runtime assembly"
  fi
}

assert_runtime_security() {
  local container=$1 volume=$2 uid cap_eff cap_add cap_drop security ports root
  uid="$(docker exec "$container" awk '/^Uid:/ {print $2}' /proc/1/status)"
  cap_eff="$(docker exec "$container" awk '/^CapEff:/ {print $2}' /proc/1/status)"
  cap_add="$(docker inspect "$container" --format '{{join .HostConfig.CapAdd ","}}')"
  cap_drop="$(docker inspect "$container" --format '{{join .HostConfig.CapDrop ","}}')"
  security="$(docker inspect "$container" --format '{{join .HostConfig.SecurityOpt ","}}')"
  ports="$(docker inspect "$container" --format '{{json .HostConfig.PortBindings}}')"
  # docker exec starts from the immutable container-config environment; the entrypoint exports
  # this default dynamically. Read pid 1's actual environment to prove the serving path value.
  root="$(docker exec --user 1000:1000 "$container" sh -c \
    "tr '\\0' '\\n' < /proc/1/environ | sed -n 's/^IRONCLAW_REBORN_WORKSPACE_ROOT=//p'")"

  [ "$uid" = 1000 ] || fail "$container pid 1 uid is $uid"
  [ "$cap_eff" = 0000000000000000 ] || fail "$container retained effective caps $cap_eff"
  [ "$cap_drop" = ALL ] || fail "$container cap_drop is $cap_drop"
  [ "$security" = no-new-privileges:true ] || fail "$container security_opt is $security"
  [ "$root" = "$FLEET_WORKSPACE_ROOT" ] || fail "$container workspace root is $root"
  case ",$cap_add," in
    *,CAP_CHOWN,*CAP_SETGID,*CAP_SETUID,|*,CAP_CHOWN,*CAP_SETUID,*CAP_SETGID,) ;;
    *) fail "$container cap_add is $cap_add" ;;
  esac
  [ "$(printf '%s' "$cap_add" | tr -cd ',' | wc -c | tr -d ' ')" = 2 ] \
    || fail "$container has unexpected added capabilities: $cap_add"
  case "$cap_add" in *DAC_OVERRIDE*) fail "$container gained DAC_OVERRIDE" ;; esac
  case "$ports" in *2222*) fail "$container publishes 2222: $ports" ;; esac
  if docker inspect "$container" --format '{{range .Config.Env}}{{println .}}{{end}}' \
    | sed 's/=.*//' | grep -q '^IRONCLAW_REBORN_SSH_PUBLIC_KEY$'; then
    fail "$container enables SSH"
  fi
  assert_workspace "$volume"
}

for spec in "$MT:$MT_VOL" "$SECRETARY:$SECRETARY_VOL"; do
  container=${spec%%:*}
  volume=${spec#*:}
  wait_runtime "$container"
  assert_runtime_security "$container" "$volume"
  docker exec --user 1000:1000 "$container" sh -c \
    "printf persistent > '$FLEET_WORKSPACE_ROOT/.ironworks-boot-proof'"
  docker restart "$container" >/dev/null
  wait_runtime "$container"
  assert_runtime_security "$container" "$volume"
  marker="$(docker exec --user 1000:1000 "$container" \
    cat "$FLEET_WORKSPACE_ROOT/.ironworks-boot-proof")"
  [ "$marker" = persistent ] || fail "$container workspace marker did not survive restart"
  runtime_version="$(docker exec "$container" /usr/local/bin/ironclaw --version)"
  [ "$runtime_version" = "ironclaw 1.4.0" ] || fail "$container reports $runtime_version"
  echo "PASS $container: cold boot + uid/caps + health + restart + persistent workspace"
done

echo "PASS exact image: $IMAGE rev=$PIN version=ironclaw-1.4.0"
echo "PASS SSH disabled; 2222 unpublished; workspace=$FLEET_WORKSPACE_ROOT owner=1000:1000"
echo "WORKSPACE_BOOT_PROOF_PASS"
