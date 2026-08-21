#!/usr/bin/env bash
# Serve-host watchdog: checks the three things serving depends on and tells a human when one
# breaks. Alert on state CHANGE (down or recovered) + an hourly reminder while down — no spam.
#
# Config: ~/.agency/watchdog.env (chmod 600):
#   WATCHDOG_BOT_TOKEN   a Telegram bot that can post to the team chat (the team/ops bot is fine)
#   TEAM_CHAT_ID         where alerts land (e.g. the leads/ops supergroup)
set -euo pipefail
set -a; . "$HOME/.agency/watchdog.env"; set +a
. "$(dirname "$0")/../../deploy/lib/curl-private.sh"   # curl_tg: bot token off argv
STATE="$HOME/.agency/watchdog.state"

fails=""
curl -sf -m 10 http://127.0.0.1:3020/api/health >/dev/null 2>&1 || fails="$fails ironclaw(:3020)"
curl -sf -m 10 http://127.0.0.1:8443/health     >/dev/null 2>&1 || fails="$fails account-service(:8443)"
systemctl is-active --quiet bridge                        || fails="$fails bridge"
# ACTIVE IS NOT WORKING. `is-active` can read green while the bridge cannot receive a single
# message — a revoked bot token logs `poll error:` every 3s in a live, "active" process. So probe
# FUNCTION, not liveness. Threshold, not presence: the loop retries every 3s, so a real outage
# produces dozens in 5m where a blip produces one or two.
# Scoped to the CURRENT invocation, not a bare time window: `--since -5 min` alone keeps counting
# errors from the process you just replaced, so a fixed bridge reads DOWN for another five
# minutes. By InvocationID a restart resets the signal instantly, and a bridge that restarts and
# fails again still accrues errors in its new invocation, so nothing is masked.
if systemctl is-active --quiet bridge; then
  inv=$(systemctl show -p InvocationID --value bridge 2>/dev/null || true)
  if [ -n "$inv" ]; then
    poll_errs=$(journalctl _SYSTEMD_INVOCATION_ID="$inv" --since "-5 min" --no-pager 2>/dev/null | grep -c 'poll error:' || true)
  else   # older systemd without InvocationID: fall back to the time window
    poll_errs=$(journalctl -u bridge --since "-5 min" --no-pager 2>/dev/null | grep -c 'poll error:' || true)
  fi
  [ "${poll_errs:-0}" -ge 5 ] && fails="$fails bridge-polling(${poll_errs}err/5m)"
fi

# A TIMER THAT NEVER FIRES IS INVISIBLE TO EVERY CHECK ABOVE — and to multi-backup.sh, which
# alerts when a run FAILS but cannot alert about one that never happened. A masked or un-enabled
# timer just stops producing backups, and nothing says so until someone needs a restore.
# Two layers, cheap first: the timer must be scheduled, and the repo must hold a recent snapshot
# (which also covers a timer that fires into a no-op).
if systemctl list-unit-files multi-backup.timer >/dev/null 2>&1; then
  systemctl is-active --quiet multi-backup.timer || fails="$fails backup-timer(inactive)"
fi

# Nightly at 03:30 with up to 15m jitter means a healthy repo is never more than ~28h stale, so
# 36h fires after exactly one missed night and never on jitter or a slow run.
# THROTTLED: this ticks every 5 minutes and `restic snapshots` is a REMOTE call — probe hourly at
# most and reuse the cached verdict, so monitoring cannot become the load. Degrades silently when
# restic or backup.env is absent: a missing backup stack must not fail the SERVING checks.
BACKUP_MAX_AGE_H="${BACKUP_MAX_AGE_H:-36}"
AGE_STATE="$HOME/.agency/watchdog.backup-age"
if [ -f "$HOME/.agency/backup.env" ] && command -v restic >/dev/null 2>&1; then
  age_checked=0; age_verdict=""
  if [ -f "$AGE_STATE" ]; then
    { read -r age_checked; read -r age_verdict; } < "$AGE_STATE" || true
  fi
  if [ $(( $(date -u +%s) - ${age_checked:-0} )) -ge 3600 ]; then
    # Subshell so backup.env's RESTIC_* never leak into the alerting path's environment.
    age_verdict=$(
      set -a; . "$HOME/.agency/backup.env"; set +a
      # `date -d` is GNU-only, as is everything else here — this script is serve-host-only by
      # construction. A miss reads as "unreadable" and alerts rather than going quiet, which is
      # the right way round. `exit` is local to this command-substitution subshell.
      ts=$(restic snapshots --tag multi-serve --latest 1 --json 2>/dev/null \
           | sed -n 's/.*"time":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1 || true)
      [ -n "$ts" ] || { echo "backup-snapshots(unreadable)"; exit 0; }
      snap=$(date -u -d "$ts" +%s 2>/dev/null || echo 0)
      [ "$snap" -ne 0 ] || { echo "backup-snapshots(unparsable-date)"; exit 0; }
      age_h=$(( ($(date -u +%s) - snap) / 3600 ))
      [ "$age_h" -ge "$BACKUP_MAX_AGE_H" ] && echo "backup-stale(${age_h}h)"
      exit 0
    )
    printf '%s\n%s\n' "$(date -u +%s)" "$age_verdict" > "$AGE_STATE"
  fi
  if [ -n "$age_verdict" ]; then fails="$fails $age_verdict"; fi
fi

now=$(date -u +%s)
prev_fails=""; prev_alert=0
[ -f "$STATE" ] && { read -r prev_fails; read -r prev_alert; } < "$STATE" || true

# Returns curl's exit status (NOT `|| true`): the caller must know whether the alert actually SENT,
# so a failed send is retried next tick instead of being silently marked as delivered.
alert() {
  curl_tg "$WATCHDOG_BOT_TOKEN" sendMessage -sf -m 15 \
    --data-urlencode "chat_id=${TEAM_CHAT_ID}" --data-urlencode "text=$1" >/dev/null
}

if [ -n "$fails" ]; then
  if [ "$fails" != "$prev_fails" ] || [ $((now - ${prev_alert:-0})) -ge 3600 ]; then
    # Advance the alert timestamp ONLY if the send succeeded. If it failed (Telegram/network blip),
    # keep the old timestamp so the retry condition stays true and the next tick re-alerts —
    # otherwise a failed first alert would suppress the outage notice for a full hour.
    if alert "🔴 multi-serve DOWN:$fails ($(hostname), $(date -u +%FT%TZ))"; then stamp="$now"; else stamp="$prev_alert"; fi
    printf '%s\n%s\n' "$fails" "$stamp" > "$STATE"
  else
    printf '%s\n%s\n' "$fails" "$prev_alert" > "$STATE"
  fi
elif [ -n "$prev_fails" ]; then
  # Clear the down-state ONLY once the recovery notice actually sent; else leave state so the next
  # tick retries it (don't lose the "recovered" message to a transient send failure).
  if alert "🟢 multi-serve recovered:$prev_fails back up ($(hostname), $(date -u +%FT%TZ))"; then
    printf '\n0\n' > "$STATE"
  fi
fi
