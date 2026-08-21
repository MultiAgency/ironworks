#!/usr/bin/env bash
# Nightly off-box backup of everything the serve host can't lose:
#   - both Postgres databases (client business data + IronClaw threads/memory), via pg_dump
#   - ~/.agency (client registry, identity file, bridge thread state, account-data, env files)
# restic = encrypted at rest, deduplicated, versioned; retention pruned below.
#
# Config: ~/.agency/backup.env (chmod 600) with at least:
#   RESTIC_REPOSITORY   e.g. sftp:u123@u123.your-storagebox.de:multi-backups   or  b2:bucket:path
#   RESTIC_PASSWORD     KEEP A COPY OFF-BOX (password manager) — restore is impossible without it
# First run once: restic init
#
# Restore drill (do it once after setup, from another machine):
#   restic snapshots; restic restore latest --target /tmp/restore
#   psql < ironclaw.sql / accounts.sql into fresh containers; copy .agency back.
set -euo pipefail
set -a; . "$HOME/.agency/backup.env"; set +a
# curl_tg keeps the alert bot token off argv. Sourced DEFENSIVELY: under `set -e` a hard `.`
# on a missing file aborts before a single dump is taken, so the ALERTING path would take down
# the BACKUP it exists to monitor. Degrade instead — back up without alerting, and say so.
if [ -f "$(dirname "$0")/../../deploy/lib/curl-private.sh" ]; then
  . "$(dirname "$0")/../../deploy/lib/curl-private.sh"
else
  echo "WARN: deploy/lib/curl-private.sh missing — backing up WITHOUT failure alerting" >&2
  curl_tg() { return 0; }   # no-op: never let a missing alert helper fail the backup
fi

WORK=$(mktemp -d); chmod 700 "$WORK"
# ONE exit handler: clean the tempdir AND alert on failure. A backup that RUNS but FAILS
# (container name drift, expired creds, full repo) otherwise surfaces only in the journal — i.e.
# only when someone needs a restore and finds none. Alert creds are OPTIONAL; the journal and a
# non-zero exit are the baseline. A run that never HAPPENS produces no failure and so cannot be
# caught here — multi-watchdog.sh carries that check.
_on_exit() {
  local rc=$?
  rm -rf "$WORK"
  if [ "$rc" -ne 0 ] && [ -n "${BACKUP_ALERT_BOT_TOKEN:-}" ] && [ -n "${BACKUP_ALERT_CHAT_ID:-}" ]; then
    curl_tg "$BACKUP_ALERT_BOT_TOKEN" sendMessage -sf -m 15 \
      --data-urlencode "chat_id=${BACKUP_ALERT_CHAT_ID}" \
      --data-urlencode "text=🔴 multi-serve BACKUP FAILED (rc=$rc) on $(hostname) $(date -u +%FT%TZ)" \
      >/dev/null 2>&1 || true
  fi
}
trap _on_exit EXIT

# instance project was renamed mt-experiment -> multi; match either container generation
docker exec "$(docker ps -qf 'name=(multi|mt-experiment)-db')" pg_dump -U postgres ironclaw > "$WORK/ironclaw.sql"
docker exec "$(docker ps -qf name=account-db)"                 pg_dump -U postgres accounts > "$WORK/accounts.sql"

# /opt/git = the bare git mirrors. /opt/ironworks = deployed scripts + recovery runbook.
# Edit this file in the tree, not on the box: `sync-vm.sh --apply` pushes the tracked tree over
# the VM, so an amendment made only there is reverted on the next deploy.
#
# DELIBERATE EXCLUSIONS: the ironclaw secret-store master key stays OUT, so a leaked restic
# repo+password cannot decrypt the secret store; `backup.env` holds RESTIC_PASSWORD, and backing
# it up inside the repo it unlocks is circular.
# LOAD-BEARING PRECONDITION: both MASTER_KEY and RESTIC_PASSWORD must live off-box. Excluding
# them WITHOUT off-box custody makes disk-loss recovery impossible — never remove these excludes
# without confirming the off-box copies exist.
restic backup "$WORK" "$HOME/.agency" /opt/git /opt/ironworks \
  --exclude /opt/ironworks/multi/instance/.env \
  --exclude "$HOME/.agency/backup.env" \
  --tag multi-serve
# Retention ceiling ~90 days:
# 14 daily + 8 weekly + 3 monthly — the oldest kept monthly is ≈90 days old.
restic forget --tag multi-serve --keep-daily 14 --keep-weekly 8 --keep-monthly 3 --prune
restic check --read-data-subset=5%
echo "backup OK: $(date -u +%FT%TZ)"
