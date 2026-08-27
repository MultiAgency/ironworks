#!/usr/bin/env bash
# Prove that a CHAINED turn recovers across process death, and that a turn which cannot recover
# never holds the channel.
#
#   dev/probe-up.sh              # runtime + disposable record store
#   dev/prove-continuation.sh    # this
#
# Every other recovery proof drives `ironworks turn`, and `turn` never chains — it has no
# conversation, so `core::continuation` always answers `Fresh`. That left the whole chained half of
# ADR 0005 unproven, and `docs/experiments/0033-the-loop-ran-and-a-chained-turn-could-not-recover.md`
# found out the hard way: a chained turn took a real `service_unavailable` and could never be
# rebuilt, because recovery reconstructed it as fresh and fully supplied whatever it had chosen.
#
# So this proof exists to make that case deterministic rather than waiting for a 503.
#
#   1. chain.established     a first turn answers and records a thread pointer.
#                            The POSITIVE CONTROL: without it every check below is vacuous,
#                            because a turn that never chained recovers trivially.
#   2. chain.is_real         the second turn actually chains and carries the book by thread
#                            rather than re-sending it. Asserted on the relay's own report, so a
#                            regression to `Fresh` fails here rather than passing check 3 by
#                            never having been chained at all.
#   3. chain.crashed         `--crash-at before-response` leaves it SUBMITTED_UNKNOWN: dispatched,
#                            with no response id, which is the state a 503 produces.
#   4. chain.recovers        the next process rebuilds it BYTE-IDENTICALLY and replays the key.
#                            This is the check that failed before the continuation decision was
#                            journalled.
#   5. cursor.released       whatever happened above, the channel offset advanced. A turn that
#                            cannot be resolved must settle rather than be retried forever: the
#                            safe cursor is the lowest OPEN update across every tenant, so one
#                            stuck turn otherwise deafens every conversation in the process while
#                            the loop backs off politely and looks healthy.
#
# Check 5 is deliberately independent of check 4. They fail for different reasons and the operator
# response differs: 4 failing means an answer was lost, 5 failing means every later answer is too.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT" || exit 3

STORE_PORT="${PROBE_STORE_PORT:-3996}"
CHAN_PORT="${PROVE_CONTINUATION_PORT:-3993}"
GUIDANCE="fixtures/account-coordination/guidance.md"
SLUG="coordination-eval"
GROUP="g-chain"
# Two short questions in ONE conversation. Short on purpose: each is a real model turn, and the
# runtime refuses anything outlasting 30s, which would make this proof measure the provider.
ASK_A="Name one account. One line."
ASK_B="Name one more account. One line."

# Fresh ids per run, for the reason `dev/prove-delivery.sh` records: the relay derives the
# idempotency key from the update id, so a re-run with the same id collides with a turn the
# RUNTIME still remembers, under a body built at a different second.
UID_BASE=$(( $(date +%s) % 900000 + 2000 ))
A=$UID_BASE
B=$(( UID_BASE + 1 ))

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-continuation.XXXXXX")"
CHAN_PID=""
cleanup() {
  if [ -n "$CHAN_PID" ]; then
    disown "$CHAN_PID" 2>/dev/null || true
    kill "$CHAN_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

PASSED=0; FAILED=0
verdict() { printf '%-9s %-26s %s\n' "$2" "$1" "${3:-}"; }
pass() { verdict "$1" PASS "${2:-}"; PASSED=$((PASSED+1)); }
fail() { verdict "$1" FAIL "${2:-}"; FAILED=$((FAILED+1)); }
blocked() { verdict "$1" BLOCKED "$2"; printf '\n%d passed, %d failed, blocked\n' "$PASSED" "$FAILED"; exit 3; }

for name in IRONWORKS_BASE IRONWORKS_TOKEN IRONWORKS_RECORDS IRONWORKS_RECORDS_TOKEN; do
  [ -n "${!name:-}" ] || blocked "environment" "$name is unset"
done
curl -fsS --max-time 3 "$IRONWORKS_BASE/api/health" >/dev/null 2>&1 \
  || blocked "runtime.reachable" "$IRONWORKS_BASE did not answer /api/health"
curl -fsS --max-time 3 "http://127.0.0.1:$STORE_PORT/health" >/dev/null 2>&1 \
  || blocked "store.reachable" "no record store on 127.0.0.1:$STORE_PORT"
cargo build -q -p ironworks-cli -p ironworks-devchannel 2>/dev/null \
  || blocked "build" "cargo build failed"
BIN=./target/debug/ironworks
CHAN=./target/debug/ironworks-devchannel
JOURNAL="$WORK/serve.journal"

cat > "$WORK/registry.json" <<JSON
{"tenants":[{
  "slug": "$SLUG",
  "channel": "$GROUP",
  "service": "relationship-intelligence",
  "guidance": "$GUIDANCE",
  "records_credential": "env:IRONWORKS_RECORDS_TOKEN",
  "runtime_credential": "env:IRONWORKS_TOKEN"
}]}
JSON

# The channel is restarted between phases so each `relay` cycle sees exactly the updates that
# phase is about. A real channel would simply not have delivered the later message yet; this is
# the fixture's way of saying the same thing.
start_channel() {
  # `disown` first: without it the shell announces the kill on stderr, and a proof that prints
  # "Terminated: 15" beside its verdicts teaches its reader to skim them.
  if [ -n "$CHAN_PID" ]; then
    disown "$CHAN_PID" 2>/dev/null || true
    kill "$CHAN_PID" 2>/dev/null || true
  fi
  "$CHAN" --port "$CHAN_PORT" "$@" >"$WORK/channel.log" 2>&1 &
  CHAN_PID=$!
  for _ in $(seq 1 30); do
    curl -fsS --max-time 1 "http://127.0.0.1:$CHAN_PORT/poll?offset=0" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  blocked "channel.up" "the disposable channel did not start"
}

relay() {
  "$BIN" relay --registry "$WORK/registry.json" --journal "$JOURNAL" \
    --channel "http://127.0.0.1:$CHAN_PORT" "$@" 2>&1
}
outcome_of() { "$BIN" status --journal "$JOURNAL" 2>/dev/null | grep -o "key=upd-$1 .*outcome=[a-z_]*" | grep -o 'outcome=[a-z_]*' | cut -d= -f2; }

# ── 1. the positive control: a first turn answers and leaves a thread pointer ──────────────
start_channel --update "$A=$GROUP:$ASK_A" --chat-kind "$A=private"
one="$(relay)"
if grep -q "update $A delivery=delivered" <<<"$one"; then
  pass "chain.established" "update $A answered and delivered"
else
  printf '%s\n' "$one" | tail -5
  blocked "chain.established" "the first turn did not deliver; nothing below would mean anything"
fi

# ── 2. the second turn really chains ───────────────────────────────────────────────────────
# Asserted separately from recovery. If continuation regressed to always-Fresh, check 4 would
# pass while proving nothing, because a fresh turn's rebuild matches trivially.
start_channel --update "$A=$GROUP:$ASK_A" --update "$B=$GROUP:$ASK_B" --chat-kind "$A=private" --chat-kind "$B=private"
two="$(relay --crash-at before-response)"
code=$?
if grep -q "thread: continuing" <<<"$two" && grep -q "carried by the thread" <<<"$two"; then
  pass "chain.is_real" "chained, and the book was not re-sent"
else
  printf '%s\n' "$two" | tail -8
  fail "chain.is_real" "the second turn did not chain — every check below is vacuous"
fi

# ── 3. it died after dispatch, with no response id ─────────────────────────────────────────
state="$(outcome_of "$B")"
if [ "$state" = "submitted_unknown" ]; then
  pass "chain.crashed" "upd-$B is submitted_unknown (exit $code)"
else
  fail "chain.crashed" "expected submitted_unknown, journal says ${state:-nothing}"
fi

# ── 4. the restart rebuilds it byte-identically ────────────────────────────────────────────
three="$(relay)"
if grep -q "rebuild is byte-identical" <<<"$three"; then
  if grep -q "update $B delivery=delivered" <<<"$three"; then
    pass "chain.recovers" "rebuilt byte-identically, replayed, delivered"
  else
    pass "chain.recovers" "rebuilt byte-identically (delivery not reached)"
  fi
elif grep -q "cannot be reconstructed byte-identically" <<<"$three"; then
  fail "chain.recovers" "a chained turn cannot be rebuilt — the decision it made was not journalled"
else
  printf '%s\n' "$three" | tail -8
  fail "chain.recovers" "neither a replay nor a refusal; see above"
fi

# ── 5. and either way the channel is not held ──────────────────────────────────────────────
# Independent of check 4 on purpose. A turn that cannot be resolved must SETTLE, because the safe
# cursor is the lowest open update across every tenant.
four="$(relay)"
offset="$(grep -o 'poll offset=[0-9]*' <<<"$four" | tail -1 | cut -d= -f2)"
if [ -n "$offset" ] && [ "$offset" -gt "$B" ]; then
  pass "cursor.released" "offset advanced to $offset, past upd-$B"
else
  fail "cursor.released" "offset stuck at ${offset:-unknown}; one turn is deafening the channel"
fi

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ] || exit 2
