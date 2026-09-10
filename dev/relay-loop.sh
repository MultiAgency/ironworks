#!/usr/bin/env bash
# COST: real model turns for as long as it runs. This is the production serving loop.
# Answer messages continuously, by running `ironworks relay` over and over.
#
#   dev/relay-loop.sh --registry <path> --journal <path> --channel <url>
#
# Every argument is passed straight to `relay`. This script adds repetition and nothing else,
# which is the whole design.
#
# # A supervisor, not a daemon
#
# `relay` handles one poll cycle and exits, deliberately: it is what makes every crash boundary a
# real process boundary, which is what `dev/prove.sh` provokes and asserts against. A long-running
# relay would turn those boundaries into moments inside one process and quietly weaken four proofs.
#
# So each cycle here is a **fresh process**. That property is not incidental — it is the reason
# this is a loop around a one-shot rather than a `--daemon` flag on the binary. Kill this script
# at any moment and the worst case is a cycle that died mid-turn, which is precisely the case the
# journal already recovers: the next cycle adopts the unresolved turn and replays one logical turn
# rather than submitting a second. Re-invoking IS the recovery.
#
# # It holds no state, on purpose
#
# The cursor lives in the journal, derived across every tenant from durable rows. A loop that
# remembered an offset would be a second answer to a question that must have exactly one, and the
# two would disagree the first time a cycle died between delivering and acknowledging.
#
# # Why it backs off
#
# The runtime's rate limit is 30 requests / 60 seconds, shared with everything else on that
# instance. A tight loop against an empty channel spends that budget on nothing, so an idle cycle
# waits and a failing one waits longer, up to a cap.
#
# # Why a usage error stops it
#
# A misconfiguration retried forever is an outage that looks like uptime: the process is up, the
# log scrolls, and nobody is being answered. Exit code 64 means the arguments are wrong, and no
# amount of waiting fixes that, so the loop stops and says so.

set -uo pipefail   # deliberately not -e: a failing cycle is information, not a reason to quit

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${IRONWORKS_BIN:-$ROOT/target/debug/ironworks}"
IDLE="${RELAY_IDLE:-5}"              # seconds between healthy cycles
BACKOFF_MAX="${RELAY_BACKOFF_MAX:-120}"

USAGE=64
CRASHED=75

if [ "$#" -eq 0 ]; then
  sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
  exit "$USAGE"
fi
[ -x "$BINARY" ] || { printf 'relay-loop: no binary at %s\n' "$BINARY" >&2; exit "$USAGE"; }

# Stop between cycles rather than inside one. A signal mid-turn is safe — the journal recovers it
# — but finishing the cycle in hand means the common case leaves nothing to recover.
stopping=0
trap 'stopping=1; printf "\nrelay-loop: stopping after this cycle\n"' INT TERM

wait_for() {
  local left="$1"
  while [ "$left" -gt 0 ] && [ "$stopping" -eq 0 ]; do
    sleep 1
    left=$(( left - 1 ))
  done
}

cycles=0
answered=0
backoff="$IDLE"

printf 'relay-loop: one process per cycle, %ss idle, backing off to %ss\n' "$IDLE" "$BACKOFF_MAX"

while [ "$stopping" -eq 0 ]; do
  "$BINARY" relay "$@"
  rc=$?
  cycles=$(( cycles + 1 ))

  case "$rc" in
    0)
      # A healthy cycle. It may or may not have answered anything; `relay` says which.
      answered=$(( answered + 1 ))
      backoff="$IDLE"
      ;;
    "$USAGE")
      printf 'relay-loop: relay rejected its arguments (exit %s). Nothing to retry.\n' "$rc" >&2
      exit "$USAGE"
      ;;
    "$CRASHED")
      # The deliberate crash affordance, which has no business in a running loop: it means
      # --crash-at was passed, and continuing would restart the same crash forever.
      printf 'relay-loop: relay took a deliberate crash point (exit %s). Remove --crash-at.\n' "$rc" >&2
      exit "$CRASHED"
      ;;
    *)
      # Everything else is treated as transient — the runtime down, the record store unreachable,
      # a turn that failed. The next cycle recovers from the journal, so retrying is safe; only
      # the interval changes.
      printf 'relay-loop: cycle %s exited %s, waiting %ss\n' "$cycles" "$rc" "$backoff" >&2
      wait_for "$backoff"
      backoff=$(( backoff * 2 ))
      [ "$backoff" -gt "$BACKOFF_MAX" ] && backoff="$BACKOFF_MAX"
      continue
      ;;
  esac

  wait_for "$IDLE"
done

printf 'relay-loop: stopped after %s cycle(s), %s healthy\n' "$cycles" "$answered"
