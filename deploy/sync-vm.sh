#!/usr/bin/env bash
# sync-vm.sh — reconcile the deployment host with this repo's TRACKED tree.
#
# The host is hand-copied, not a checkout, so it drifts silently: a fix can look committed
# and reviewed while the host still runs the old file.
#
# THE MANIFEST IS `git ls-files`, AND THAT IS A SECURITY PROPERTY. A naive `rsync -a ./`
# would ship admin/ and every *.env to the server; deriving the list from git makes anything
# gitignored structurally unreachable here. It syncs the WORKING TREE of tracked files, not
# the index — what you see in your editor is what ships.
#
# Usage:
#   ./deploy/sync-vm.sh                 # DRY RUN: what would change (default, safe)
#   ./deploy/sync-vm.sh --check         # drift report only; exit 1 if the VM is out of date
#   ./deploy/sync-vm.sh --apply         # actually push, then stamp DEPLOYED_MANIFEST.sha256
#
# The host is NOT hardcoded — infra addresses must never be committed (see .gitignore).
# Supply it per-run or from an untracked file, e.g. ~/.agency/vm.env with VM_HOST=user@ip:
#   set -a; . ~/.agency/vm.env; set +a; ./deploy/sync-vm.sh --check
set -euo pipefail

VM_HOST="${VM_HOST:?set VM_HOST (user@host) — keep it in ~/.agency/vm.env, never in the repo}"
VM_PATH="${VM_PATH:-/opt/ironworks}"
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=15)
. "$(git rev-parse --show-toplevel)/deploy/lib/fleet.sh"   # fleet_mt_container: one resolver

MODE="dry"
case "${1:-}" in
  --apply) MODE="apply" ;;
  --check) MODE="check" ;;
  "" | --dry-run) MODE="dry" ;;
  *) echo "usage: $0 [--check|--apply]   (default: dry run)" >&2; exit 2 ;;
esac

cd "$(git rev-parse --show-toplevel)"

# Local digests. macOS ships shasum, Linux sha256sum — accept either. This is an ARRAY, not
# a function, because the digest step runs under xargs and xargs cannot invoke shell functions.
if command -v sha256sum >/dev/null 2>&1; then SHA=(sha256sum); else SHA=(shasum -a 256); fi

tmp="$(mktemp -d)"
# One TCP+auth handshake for the whole run instead of six or seven to the same host. The master
# is closed explicitly before $tmp goes away, so no socket outlives the script.
SSH_OPTS+=(-o ControlMaster=auto -o ControlPath="$tmp/cm-%C" -o ControlPersist=30)
trap 'ssh "${SSH_OPTS[@]}" -O exit "$VM_HOST" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT
git ls-files -z > "$tmp/paths.z"
tr '\0' '\n' < "$tmp/paths.z" > "$tmp/paths.txt"
# Belt-and-braces on the manifest property: git ls-files already omits gitignored files,
# but an explicitly TRACKED secret would still ship. Refuse to transfer one, ever.
# (.env.example ends in .example, so it is deliberately not matched.)
# One pattern for both the test and the report: two copies can disagree, and the test is
# the copy that decides whether a secret ships.
SECRET_PATHS='\.(env|token|key|pem|p12|pfx)$|^(admin|ops)/|^multi/serve/SETUP\.md$'
if grep -Eq "$SECRET_PATHS" "$tmp/paths.txt"; then
  echo "!! manifest contains a secret/private path — aborting before any transfer:" >&2
  grep -E "$SECRET_PATHS" "$tmp/paths.txt" | sed 's/^/   /' >&2
  exit 4
fi

xargs -0 "${SHA[@]}" < "$tmp/paths.z" | awk '{print $2" "$1}' | LC_ALL=C sort > "$tmp/local.txt"

# Remote digests for exactly those paths. A missing file is reported, never assumed equal.
# shellcheck disable=SC2029  # $VM_PATH expands client-side on purpose
ssh "${SSH_OPTS[@]}" "$VM_HOST" "cd '$VM_PATH' 2>/dev/null || exit 9
  while IFS= read -r f; do
    if [ -f \"\$f\" ]; then echo \"\$f \$(sha256sum \"\$f\" | cut -d' ' -f1)\"
    else echo \"\$f MISSING\"; fi
  done | LC_ALL=C sort" < "$tmp/paths.txt" > "$tmp/remote.txt" || {
    echo "!! cannot read $VM_PATH on $VM_HOST (exit $?)" >&2; exit 1; }

LC_ALL=C join "$tmp/local.txt" "$tmp/remote.txt" > "$tmp/joined.txt" || true
awk '$3=="MISSING"{print $1}' "$tmp/joined.txt" > "$tmp/missing.txt"
awk '$3!="MISSING" && $2!=$3{print $1}' "$tmp/joined.txt" > "$tmp/differs.txt"
n_same="$(awk '$2==$3' "$tmp/joined.txt" | wc -l | tr -d ' ')"
n_diff="$(wc -l < "$tmp/differs.txt" | tr -d ' ')"
n_miss="$(wc -l < "$tmp/missing.txt" | tr -d ' ')"
n_all="$(wc -l < "$tmp/paths.txt" | tr -d ' ')"

# A verification tool that silently drops rows is worse than none: if join loses a path
# (collation mismatch, odd filename) that file reads as "fine" while never being compared.
n_seen=$(( n_same + n_diff + n_miss ))
if [ "$n_seen" -ne "$n_all" ]; then
  echo "!! compared only $n_seen of $n_all tracked files — report is NOT trustworthy" >&2
  LC_ALL=C comm -23 <(LC_ALL=C sort "$tmp/paths.txt") \
                    <(awk '{print $1}' "$tmp/joined.txt" | LC_ALL=C sort) \
    | sed 's/^/   UNCOMPARED  /' >&2
  exit 3
fi

echo "== $VM_HOST:$VM_PATH vs tracked tree ($n_all files) =="
echo "   identical $n_same · differs $n_diff · missing $n_miss"
[ "$n_miss" -gt 0 ] && sed 's/^/   MISSING  /' "$tmp/missing.txt"
[ "$n_diff" -gt 0 ] && sed 's/^/   DIFFERS  /' "$tmp/differs.txt"

# SYNCED IS NOT LIVE, and this check runs even when everything matches — because a matching
# file is EXACTLY when a stale process hides. The hash compare cannot see that the bridge
# loaded multi/seam/*.py at boot and still runs that code in memory.
ssh "${SSH_OPTS[@]}" "$VM_HOST" '
  started="$(systemctl show -p ActiveEnterTimestamp --value bridge 2>/dev/null)"
  newest="$(ls -t /opt/ironworks/multi/seam/*.py 2>/dev/null | head -1)"
  if [ -n "$started" ] && [ -n "$newest" ]; then
    s_e=$(date -d "$started" +%s 2>/dev/null || echo 0)
    n_e=$(stat -c %Y "$newest" 2>/dev/null || echo 0)
    if [ "$s_e" -gt 0 ] && [ "$n_e" -gt "$s_e" ]; then
      echo "   STALE PROCESS: bridge started $started but $(basename "$newest") is newer"
      echo "                  -> it is running seam code from before that file landed; restart it"
    else
      echo "   bridge process is newer than the seam files (not stale)"
    fi
  fi' 2>/dev/null || true

if [ "$n_diff" -eq 0 ] && [ "$n_miss" -eq 0 ]; then
  echo "   VM matches the tracked tree."
  exit 0
fi

# SYNCED IS NOT LIVE. Copying persona.py changed nothing until the bridge restarted, so say
# out loud which services still hold stale code in memory once the files land.
restart_hint() {
  cat "$tmp/differs.txt" "$tmp/missing.txt" > "$tmp/changed.txt"
  echo "   -- after applying, these need a restart to take effect --"
  grep -q '^multi/seam/'            "$tmp/changed.txt" && echo "      bridge          : sudo systemctl restart bridge"
  # -p is LOAD-BEARING when the running project name differs from the compose file's `name:`.
  # A bare `up -d` then does NOT upgrade the running stack — it builds a second one on FRESH
  # EMPTY volumes (no sealed accounts, no confinement, no client threads) and looks healthy.
  # So the hint prints the project actually running rather than assuming.
  if grep -q '^multi/instance/' "$tmp/changed.txt"; then
    # Ask the MT container itself for its project. Do NOT substring-match on `ironclaw`:
    # on a box also running `secretary-ironclaw-1` that returns `secretary` and names the
    # wrong project — worse than no hint, since this mitigates the blank-stack failure above.
    # Two names are tried because the VM keeps the legacy one until MT is recreated.
    # `|| true`: under pipefail an unmatched grep aborts the assignment, so the := default
    # would never run — the emptiness test IS the fallback and must stay reachable.
    # _configured, NOT the local-reality resolver: this queries the VM, and what runs on this
    # laptop says nothing about what runs there.
    mt_name="$(fleet_mt_container_configured)"
    # shellcheck disable=SC2029  # $mt_name expands client-side on purpose (derived from the local compose)
    proj="$(ssh "${SSH_OPTS[@]}" "$VM_HOST" \
      "for c in '$mt_name' multi-ironclaw-1; do \
         docker inspect -f '{{index .Config.Labels \"com.docker.compose.project\"}}' \"\$c\" 2>/dev/null && break; \
       done" 2>/dev/null | head -1 || true)"
    echo "      MT instance     : cd $VM_PATH/multi/instance && docker compose -p ${proj:-<RUNNING-PROJECT>} up -d"
    [ -n "$proj" ] && echo "                        (-p $proj = the project actually running; a bare up -d starts a BLANK stack)"
  fi
  # Sourcing account-db.env is REQUIRED: the compose fail-fasts on ${ACCOUNT_DB_PASSWORD:?},
  # so without it the command aborts having done nothing.
  if grep -q '^deploy/account-intel/' "$tmp/changed.txt"; then
    echo "      account service : cd $VM_PATH/deploy/account-intel/data \\"
    echo "                          && set -a && . ~/.agency/account-db.env && set +a \\"
    echo "                          && docker compose up -d --build"
    # Migrations apply BEFORE the restart, or new code queries columns that do not exist.
    if grep -qE '^deploy/account-intel/data/migrate-[0-9]+.*\.sql' "$tmp/changed.txt"; then
      echo "      !! MIGRATION CHANGED — apply it BEFORE that restart:"
      grep -E '^deploy/account-intel/data/migrate-[0-9]+.*\.sql' "$tmp/changed.txt" \
        | sed "s|^|         docker exec -i <account-db> psql -U postgres -d accounts < $VM_PATH/|"
    fi
  fi
  return 0
}

# DIFFERS does not say WHICH SIDE IS AHEAD, and --apply resolves it as "the tree wins" every
# time — wrong whenever someone edited the box directly, which happens because there is no pull
# path. mtime is the only directional signal and it is WEAK (a .pre-* restore carries a fresh
# timestamp over older content), so this WARNS and names files to diff rather than deciding.
remote_newer() {
  [ -s "$tmp/differs.txt" ] || return 0
  if stat -f %m . >/dev/null 2>&1; then LSTAT=(stat -f '%m'); else LSTAT=(stat -c '%Y'); fi
  while IFS= read -r f; do printf '%s %s\n' "$f" "$("${LSTAT[@]}" "$f")"; done \
    < "$tmp/differs.txt" | LC_ALL=C sort > "$tmp/lmtime.txt"
  # shellcheck disable=SC2029  # $VM_PATH expands client-side on purpose
  ssh "${SSH_OPTS[@]}" "$VM_HOST" "cd '$VM_PATH' 2>/dev/null || exit 9
    while IFS= read -r f; do
      [ -f \"\$f\" ] && echo \"\$f \$(stat -c %Y \"\$f\" 2>/dev/null || echo 0)\"
    done | LC_ALL=C sort" < "$tmp/differs.txt" > "$tmp/rmtime.txt" 2>/dev/null || return 0
  LC_ALL=C join "$tmp/lmtime.txt" "$tmp/rmtime.txt" 2>/dev/null | awk '$3 > $2 {print $1}'
}

if [ "$MODE" = "check" ]; then
  restart_hint
  echo "!! VM is out of date" >&2
  exit 1
fi

if [ "$MODE" = "dry" ]; then
  restart_hint
  echo "   (dry run — nothing changed. Re-run with --apply to push.)"
  exit 0
fi

# LOCAL GATE — is this tree quiet, and does it agree with itself?
#
# `--apply` ships the WORKING TREE of every tracked path: not your diff, not what is staged. With
# more than one session editing the repo, another lane's half-finished edit rides along unseen.
#
# python3, not `find -newermt`: that flag rejects a RELATIVE timestamp under bfs and BSD find,
# and it fails by printing an error and exiting 0 — so the check reads as "nothing recent" and
# the gate silently never fires.
recently_touched() {
  python3 - "$tmp/paths.txt" <<'PYEOF'
import os, sys, time
cut = time.time() - 90
for line in open(sys.argv[1]):
    f = line.strip()
    try:
        if f and os.path.getmtime(f) > cut:
            print(f)
    except OSError:
        pass
PYEOF
}
STIRRING="$(recently_touched || true)"
if [ -n "$STIRRING" ] && [ "${SYNC_FORCE:-}" != 1 ]; then
  echo "!! REFUSING TO APPLY — these tracked files changed in the last 90 seconds:" >&2
  printf '%s\n' "$STIRRING" | sed 's/^/     /' >&2
  echo "   Someone (or something) is still writing. Applying now ships a mid-edit tree and" >&2
  echo "   pins the blame on whoever restarts next. Wait for the tree to settle, re-run the" >&2
  echo "   suite, then apply — or SYNC_FORCE=1 if you are certain the writer is you." >&2
  exit 3
fi

SPLIT="$(tr '\n' '\0' < "$tmp/paths.txt" | xargs -0 git diff --name-only -- 2>/dev/null || true)"
if [ -n "$SPLIT" ]; then
  echo "   NOTE: index and worktree differ for these shipped files —" >&2
  printf '%s\n' "$SPLIT" | sed 's/^/         /' >&2
  echo "         --apply ships the WORKTREE version; a commit would capture the INDEX one." >&2
  echo "         Not fatal, and not blocked: staging is not this script's business. But if you" >&2
  echo "         did not intend two versions, reconcile them before the deploy becomes history." >&2
fi

NEWER="$(remote_newer || true)"
if [ -n "$NEWER" ] && [ "${SYNC_FORCE:-}" != 1 ]; then
  echo "!! REFUSING TO APPLY — these differ AND the VM copy has the newer timestamp:" >&2
  printf '%s\n' "$NEWER" | sed 's/^/     /' >&2
  echo "   Someone may have edited them on the box. Read the diff before choosing a winner —" >&2
  echo "   a newer mtime is not proof of newer content (a .pre-* restore looks newer too):" >&2
  echo "     ssh \$VM_HOST \"cat $VM_PATH/<path>\" | diff <path> -" >&2
  echo "   Then merge the VM's version into the tree, or SYNC_FORCE=1 to push the tree's." >&2
  exit 3
fi

echo "== applying =="
rsync -rlptv -e "ssh ${SSH_OPTS[*]}" --files-from="$tmp/paths.txt" ./ "$VM_HOST:$VM_PATH/"

# Stamp the deployed state so drift is checkable FROM THE VM later, without this laptop.
# Reuse the digests from the compare rather than re-hashing: this path is only reachable after
# the gate proved the tree did not change, so a second pass can only reproduce the same bytes.
# shellcheck disable=SC2029  # $VM_PATH is MEANT to expand client-side (it is our variable)
ssh "${SSH_OPTS[@]}" "$VM_HOST" \
  "cat > '$VM_PATH/DEPLOYED_MANIFEST.sha256' && cd '$VM_PATH' \
   && date -u +'deployed %Y-%m-%dT%H:%M:%SZ' >> DEPLOYED_MANIFEST.sha256" < "$tmp/local.txt"
echo "   stamped $VM_PATH/DEPLOYED_MANIFEST.sha256"
restart_hint

# Deliberately NOT deleting VM-side extras: removing files on a production box is how you
# lose something nobody modeled. Report them and let a human decide.

# Did the push break what was already running? Files landing is not the same as services
# surviving, and the cheapest moment to learn otherwise is now, not at the next restart.
# Advisory only: a red line here means look, not that the sync failed.
ssh "${SSH_OPTS[@]}" "$VM_HOST" '
  for probe in "MT:3020/api/health" "account-service:8443/health"; do
    name="${probe%%:*}"; hp="${probe#*:}"
    if curl -sf -m 5 "http://127.0.0.1:${hp}" >/dev/null 2>&1; then echo "   health OK   $name"
    else echo "   HEALTH FAIL $name — check it"; fi
  done
  b="$(systemctl is-active bridge 2>/dev/null || true)"
  [ -n "$b" ] && echo "   bridge      $b"' 2>/dev/null || echo "   (health probe unavailable)"

# Operator time log, if this machine keeps one (untracked). Set TIME_LOG and TIME_LOG_SLUG
# alongside VM_HOST in ~/.agency/vm.env. UNSET is a deliberate no-op; SET-BUT-UNUSABLE is not —
# a typo'd path falling through the `-f` test in silence lets an operator believe every --apply
# was recorded while nothing was written. Warns rather than exits: the deploy itself succeeded.
LOG="${TIME_LOG:-}"
if [ -n "$LOG" ]; then
  if [ -f "$LOG" ] && [ -w "$LOG" ]; then
    printf '%s,%s,provisioning,"sync-vm --apply: %s file(s) pushed",2,n,n,n,"operator deploy"\n' \
      "$(date +%F)" "${TIME_LOG_SLUG:-unassigned}" "$(( n_diff + n_miss ))" >> "$LOG"
  else
    echo "   !! TIME_LOG is set to '$LOG' but it is not a writable file — this deploy was NOT" >&2
    echo "      recorded. Fix the path in ~/.agency/vm.env, or unset TIME_LOG to silence this." >&2
  fi
fi
echo "== done =="
