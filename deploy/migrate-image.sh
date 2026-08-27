#!/usr/bin/env bash
# Move one agent container to a different ironclaw image, preserving its
# volume, port, restart policy, and environment. Written for the
# repin of any rev — it was first written for the #7464 telegram strategy change,
# which is history rather than a property of this tool. The pin of record is
# IRONCLAW_PIN and is deliberately not restated here (UPGRADE.md); the telegram
# auth model that comes with it is asserted by `doctor.sh --deep`, not assumed.
#
#   ./deploy/migrate-image.sh ironclaw-ops ironclaw:pinned
#   ./deploy/migrate-image.sh ironclaw-ops ironclaw:pinned --rotate-token
#
# --rotate-token mints a fresh IRONCLAW_REBORN_WEBUI_TOKEN into the recreated
# container (the migration is the one natural rotation point — without this flag
# the old operator token is preserved forever) and updates the agent's
# ~/.agency/agents/<slug>.env to match.
#
# Steps: capture config -> stop -> back up the volume DB -> recreate on the
# target image -> wait for API -> assert no assembly errors and pairing mint
# returns 200. If the new binary can't deserialize stored extension state
# (downgrade across a manifest-schema change), it prints the surgery recipe
# instead of guessing.
set -euo pipefail
C="${1:?usage: migrate-image.sh <container> <image> [--rotate-token]}"
IMAGE="${2:?usage: migrate-image.sh <container> <image> [--rotate-token]}"
ROTATE=0
case "${3:-}" in --rotate-token) ROTATE=1 ;; '') ;; *) echo "!! unknown arg: $3" >&2; exit 2 ;; esac
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$REPO_DIR/deploy/lib/curl-private.sh"   # curl_bearer: operator token off argv
. "$REPO_DIR/deploy/lib/fleet.sh"

# --- persona-surface pin gate -----------------------------------------------------
# The fleet writes personas to an upstream-INTERNAL path ($FLEET_PERSONA_DST).
# Before moving an agent to a new image, prove the pinned source still derives
# that exact path: a silent upstream move would boot the new binary on a freshly
# seeded STOCK persona while the custom one sits ignored at the old path.
# Checks all three path segments at the IRONCLAW_PIN rev, before anything stops
# (doctor.sh's stray-seed sweep is the post-migration runtime backstop).
IRONCLAW_SRC="${IRONCLAW_SRC:-/opt/ironclaw-src}"
if [ -f "$REPO_DIR/IRONCLAW_PIN" ] && git -C "$IRONCLAW_SRC" rev-parse --git-dir >/dev/null 2>&1; then
  REV="$(fleet_ironclaw_pin)"
  if git -C "$IRONCLAW_SRC" rev-parse -q --verify "$REV^{commit}" >/dev/null; then
    miss=""
    git -C "$IRONCLAW_SRC" grep -q 'system/prompts/default-system\.md' "$REV" -- crates/ \
      || miss="$miss prompt-path-const"
    git -C "$IRONCLAW_SRC" grep -q '"hosted-single-tenant-volume"' "$REV" -- crates/app/ironclaw_config/ \
      || miss="$miss profile-storage-subdir"
    git -C "$IRONCLAW_SRC" grep -q '/data/ironclaw-reborn' "$REV" -- docker/reborn/entrypoint.sh \
      || miss="$miss entrypoint-home-default"
    if [ -n "$miss" ]; then
      echo "!! persona surface moved upstream at pin ${REV:0:9} (missing:$miss)" >&2
      echo "   $FLEET_PERSONA_DST is no longer derivable from the pinned source." >&2
      echo "   Locate the persona path in the pinned source and update deploy/lib/fleet.sh before migrating." >&2
      exit 1
    fi
    echo "   persona-surface gate: OK at pin ${REV:0:9}"
  else
    echo "   persona-surface gate: SKIPPED (pin ${REV:0:9} not in $IRONCLAW_SRC — git fetch first; UPGRADE.md step 2)"
  fi
else
  echo "   persona-surface gate: SKIPPED (no IRONCLAW_PIN or no checkout at $IRONCLAW_SRC — set IRONCLAW_SRC)"
fi

# --- target-image provenance ------------------------------------------------------
# The gate above proves the pinned SOURCE derives the persona path; it says nothing about
# the image being migrated TO. `ironclaw.rev` (UPGRADE.md step 4) ties the two together.
# Unlabeled only WARNs — pre-label images must stay migratable, which is the one thing an
# operator always needs. A label DISAGREEING with the pin is fatal: deployed state and the
# pin contradict each other.
if [ -z "${REV:-}" ]; then
  echo "   image provenance: SKIPPED (no pin resolved above — nothing to compare against)"
else
img_rev="$(docker inspect -f '{{index .Config.Labels "ironclaw.rev"}}' "$IMAGE" 2>/dev/null || true)"
case "$img_rev" in
  ''|'<no value>')
    echo "   !! image provenance: $IMAGE carries no ironclaw.rev label — cannot prove it was built"
    echo "      from ${REV:-IRONCLAW_PIN}. Rebuild per UPGRADE.md step 4 (docker build --label ironclaw.rev=\$REV)"
    echo "      to make this checkable; continuing (pre-label image)." ;;
  "$REV")
    echo "   image provenance: OK ($IMAGE built from pin ${REV:0:9})" ;;
  *)
    echo "!! image provenance MISMATCH: $IMAGE was built from ${img_rev:0:9}, but IRONCLAW_PIN is ${REV:0:9}." >&2
    echo "   Migrating would deploy a rev the pin does not describe. Retag/rebuild, or update IRONCLAW_PIN first." >&2
    exit 1 ;;
esac
fi

port=$(docker inspect "$C" --format '{{(index (index .HostConfig.PortBindings "3000/tcp") 0).HostPort}}')
# Select the data volume by DESTINATION, never by position: Docker guarantees no ordering
# for .Mounts, and `docker rm` below destroys the original container config before the
# recreate binds whatever came back — so a wrong pick here is unrecoverable. Every ironclaw
# container has exactly one mount today, which is why the positional form never failed.
vol=$(docker inspect "$C" --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}')
[ -n "$vol" ] || { echo "!! $C has no volume mounted at /data — refusing to recreate it blind" >&2; exit 1; }
envfile=$(mktemp); trap 'rm -f "$envfile"' EXIT; chmod 600 "$envfile"
docker inspect "$C" --format '{{range .Config.Env}}{{println .}}{{end}}' | grep -v '^$' > "$envfile"
echo "== $C -> $IMAGE  (port $port, volume $vol)"

replace_token_line() {  # replace_token_line <file> <new-token> — rewrite the IRONCLAW_REBORN_WEBUI_TOKEN= line
  python3 - "$1" "$2" <<'PY'
import sys
f, tok = sys.argv[1:3]
lines = open(f).read().splitlines()
out = [f"IRONCLAW_REBORN_WEBUI_TOKEN={tok}" if l.startswith("IRONCLAW_REBORN_WEBUI_TOKEN=") else l for l in lines]
open(f, "w").write("\n".join(out) + "\n")
PY
}
NEW_TOK=""
if [ "$ROTATE" = 1 ]; then
  grep -q '^IRONCLAW_REBORN_WEBUI_TOKEN=' "$envfile" || { echo "!! $C has no IRONCLAW_REBORN_WEBUI_TOKEN in its env — nothing to rotate" >&2; exit 1; }
  NEW_TOK=$(openssl rand -hex 32)
  replace_token_line "$envfile" "$NEW_TOK"
  echo "   rotating operator token (fresh token in the recreated container)"
fi

docker stop "$C" >/dev/null
docker run --rm -v "$vol:/data" alpine sh -c \
  'cp /data/ironclaw-reborn/hosted-single-tenant-volume/reborn-local-dev.db \
      /data/ironclaw-reborn/hosted-single-tenant-volume/reborn-local-dev.db.bak-migrate && echo "   db backup: ok"'
docker rm "$C" >/dev/null
docker run -d --name "$C" --restart unless-stopped -p "127.0.0.1:$port:3000" \
  -v "$vol:/data" --env-file "$envfile" "$IMAGE" >/dev/null
echo "   recreated"

tok=$(docker exec "$C" printenv IRONCLAW_REBORN_WEBUI_TOKEN)
# fleet_wait_api fails loudly if the recreated container never answers, so we stop here rather
# than reporting success and running the deserialization checks against a dead API. (This site
# used --retry 25 where the other five used 20; the helper standardises on 20.)
fleet_wait_api "$tok" "http://127.0.0.1:$port"
echo "   API up"

if docker logs "$C" --since 2m 2>&1 | grep -q "unknown variant"; then
  hash=$(docker run --rm -v "$vol:/data" alpine sh -c \
    "apk add -q sqlite; sqlite3 -readonly /data/ironclaw-reborn/hosted-single-tenant-volume/reborn-local-dev.db \
     \"SELECT path FROM root_filesystem_entries WHERE path LIKE '/system/extensions/.installations/installations/%' AND CAST(contents AS TEXT) LIKE '%telegram%';\"" \
    | sed 's|.*/sha256_||; s|\.json||' | head -1)
  echo "!! stored telegram install doesn't deserialize on $IMAGE." >&2
  if [ -z "$hash" ]; then
    # Empty hash = the locator query matched nothing (schema/path drift). DO NOT print a
    # DELETE recipe — an empty hash yields `LIKE '%%'`, which matches EVERY row and would wipe
    # the whole filesystem table if pasted. Fall back to manual inspection.
    echo "   Could NOT locate the offending install hash (query matched nothing — schema/path drift?)."
    echo "   Inspect manually before deleting anything; do NOT run a blanket DELETE. Read the"
    echo "   /system/extensions rows first (sqlite3 -readonly on the volume's reborn-local-dev.db),"
    echo "   identify the specific install path, and delete only that exact path."
    exit 1
  fi
  echo "   Fix (backs up again, deletes only that install, then rerun this script):"
  printf '%s\n' "   docker run --rm -v $vol:/data alpine sh -c 'DB=/data/ironclaw-reborn/hosted-single-tenant-volume/reborn-local-dev.db; cp \$DB \$DB.bak2; apk add -q sqlite; sqlite3 \$DB \"DELETE FROM root_filesystem_entries WHERE path LIKE '\\''%$hash%'\\'';\"'"
  echo "   then: docker restart $C && reinstall telegram via the API"
  exit 1
fi

mint=$(curl_bearer "$tok" -s -o /dev/null -w '%{http_code}' --max-time 10 -X POST \
  -H 'content-type: application/json' -d '{}' \
  "http://127.0.0.1:$port/api/webchat/v2/extensions/telegram/pairing/mint")
# Both codes are healthy here; WHICH one is right is a property of the PINNED REV, not a
# constant. Telegram's `[channel.connection] strategy` decides whether it registers a pairing
# service: `web_generated_code` does (mint 200), `device_link` does not (404 for
# extension_id=telegram only — the generic route still exists and the registry decides
# per-extension, channel_pairing.rs). Upstream has moved this BOTH ways (#7464 to device_link,
# #7766 back), so this script reports which it saw and `doctor.sh --deep` is what asserts the
# running image AGREES with the pin.
case "$mint" in
  200) echo "   pairing: telegram registers a pairing service (mint HTTP 200)";;
  404) echo "   pairing: telegram registers no pairing service (mint HTTP 404)";;
  *)   echo "!! pairing mint returned HTTP $mint — check docker logs $C" >&2; exit 1;;
esac
if [ "$ROTATE" = 1 ]; then
  sec="$(fleet_agent_env "${C#ironclaw-}")"
  if [ -f "$sec" ]; then
    replace_token_line "$sec" "$NEW_TOK"
    echo "   operator token rotated; $sec updated"
  else
    echo "   operator token rotated; no $sec to update — the new token lives only in the container env (docker exec $C printenv IRONCLAW_REBORN_WEBUI_TOKEN)"
  fi
fi
echo "== $C healthy on $IMAGE =="
