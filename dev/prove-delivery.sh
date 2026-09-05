#!/usr/bin/env bash
# Prove delivery across real process restarts: what a client does and does not read twice.
#
#   dev/probe-up.sh              # runtime + disposable record store
#   dev/prove.sh                 # identity, refusals, and turn recovery (B1-B4)
#   dev/prove-delivery.sh        # this: delivery evidence and channel recovery (B5, B7)
#
# Four checks. Every one of them runs the process to death and starts a new one, because the
# question is not "does the state machine agree with itself" — that is answered by
# `cargo test -p ironworks-channel` — but "does a real client end up reading the same answer
# twice, or none at all".
#
#   1. known-unsent          the channel refuses before accepting anything; one attempt, no
#                            automatic retry, and the evidence says a resend is free
#   2. B5 residual window    the channel took the answer; IronWorks died before recording it.
#                            The restart sends the SAME answer — proven by digest, not by count
#   3. partial delivery      N chunks land, then contact is lost. The count survives, and a later
#                            attempt that knows LESS cannot lower it
#   4. B7 acknowledgement    delivered, then death before the offset went out. The channel
#                            redelivers; zero reruns, zero resends, and the offset advances
#
# ── what the observation log proves, and what it does not ───────────────────────────────────
#
# Assertions read `GET /observations` from the channel — an append-only record of what the
# channel externally did. Nothing here inspects the fixture's internals, so a passing check is
# evidence about behaviour a real channel could also exhibit rather than about this fixture's
# shape. Chunk contents appear as digests, which is how a duplicate is proven byte-identical
# without a client's answer ever coming back out of the channel.
#
# What is NOT proven: that the provider's model executed exactly once. The runtime exposes no
# such counter. Create calls are counted at the adapter boundary and response identity is the
# strongest available evidence — a second logical turn would carry a different response id.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

STORE_PORT="${PROBE_STORE_PORT:-3996}"
CHAN_PORT="${PROVE_CHANNEL_PORT:-3994}"
STORE_TOKEN="dev-token-eval"
GUIDANCE="fixtures/eval.guidance.md"
SLUG="eval"
SERVICE="relationship-intelligence"
GROUP="g-eval"
ASK="Name the single account with a confirmed budget. One line."
# Update ids must be fresh on every run of this script.
#
# The relay derives an idempotency key from the update id, so re-running with the same id would
# collide with a turn the RUNTIME still remembers from the previous run, under a body built at a
# different second. That is correctly refused — and the refusal is now an operator alarm rather
# than a retry loop — but it would make this proof unrunnable twice. A real channel numbers
# updates monotonically for the life of a bot; a fixture that restarts does not, so the ids come
# from the clock instead.
UID_BASE=$(( $(date +%s) % 900000 + 1000 ))
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-delivery.XXXXXX")"
CHAN_PID=""
cleanup() { [ -n "$CHAN_PID" ] && kill "$CHAN_PID" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

pass=0; failed=0; blocked=0
verdict() {
  printf '%-9s %-30s %s\n' "$2" "$1" "${3:-}"
  case "$2" in PASS) pass=$((pass+1));; FAIL) failed=$((failed+1));; BLOCKED) blocked=$((blocked+1));; esac
}

# ── preconditions ────────────────────────────────────────────────────────────────────────
[ -n "${IRONWORKS_BASE:-}" ] && [ -n "${IRONWORKS_TOKEN:-}" ] || {
  echo "prove-delivery: IRONWORKS_BASE / IRONWORKS_TOKEN are unset. Run dev/probe-up.sh and" >&2
  echo "                export the lines it prints." >&2
  exit 3
}
curl -fsS --max-time 3 "$IRONWORKS_BASE/api/health" >/dev/null 2>&1 || {
  verdict "runtime.reachable" BLOCKED "$IRONWORKS_BASE did not answer /api/health"; exit 3; }
curl -fsS --max-time 3 "http://127.0.0.1:$STORE_PORT/health" >/dev/null 2>&1 || {
  verdict "store.reachable" BLOCKED "no record store on :$STORE_PORT (dev/probe-up.sh)"; exit 3; }
cargo build -q -p ironworks-cli -p ironworks-devchannel 2>/dev/null || {
  verdict "build" BLOCKED "cargo build failed"; exit 3; }
BIN=./target/debug/ironworks

# One tenant is a registry with one entry — relay has no single-tenant path, deliberately, so a
# channel cannot acquire a second and looser route into the serving code. The credential handles
# name the same environment variables this script already exports.
REGISTRY="$WORK/registry.json"
cat > "$REGISTRY" <<JSON
{"tenants":[{"slug":"$SLUG","channel":"$GROUP","service":"relationship-intelligence",
  "guidance":"$GUIDANCE",
  "records_credential":"env:IRONWORKS_RECORDS_TOKEN",
  "runtime_credential":"env:IRONWORKS_TOKEN"}]}
JSON
CHAN_BIN=./target/debug/ironworks-devchannel
STORE="http://127.0.0.1:$STORE_PORT"
CHAN="http://127.0.0.1:$CHAN_PORT"

# channel <update-id> [fault flags...]
# Starts a fresh disposable channel. The journal on the IronWorks side outlives it, which is what
# lets a check restart the channel under a DIFFERENT fault and ask what the evidence does.
channel() {
  local id="$1"; shift
  [ -n "$CHAN_PID" ] && { kill "$CHAN_PID" 2>/dev/null; wait "$CHAN_PID" 2>/dev/null; CHAN_PID=""; }
  ( exec "$CHAN_BIN" --port "$CHAN_PORT" --update "$id=$GROUP:$ASK" "$@" ) \
      >/dev/null 2>&1 </dev/null &
  CHAN_PID=$!
  disown 2>/dev/null || true
  for _ in $(seq 1 30); do
    curl -fsS --max-time 1 "$CHAN/health" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  CHAN_PID=""
  return 1
}

# relay <journal> <call-log> [extra args...]
relay() {
  local j="$1" cl="$2"; shift 2
  IRONWORKS_RECORDS="$STORE" IRONWORKS_RECORDS_TOKEN="$STORE_TOKEN" \
    "$BIN" relay --registry "$REGISTRY" --journal "$j" --call-log "$cl" \
      --channel "$CHAN" ${@+"$@"} 2>&1
}

# redeliver <journal> <call-log> <update-id>
redeliver() {
  local j="$1" cl="$2" id="$3"; shift 3
  IRONWORKS_RECORDS="$STORE" IRONWORKS_RECORDS_TOKEN="$STORE_TOKEN" \
    "$BIN" redeliver --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" \
      --journal "$j" --call-log "$cl" \
      --channel "$CHAN" --update "$id" ${@+"$@"} 2>&1
}

obs()        { curl -fsS --max-time 3 "$CHAN/observations" 2>/dev/null; }
no_model()   { grep -q "not a valid model name\|model provider rejected" <<<"$1"; }
# States in which the boundary was never set up, so the check measured nothing. BLOCKED, not FAIL.
#
# This script had `no_model` and nothing else, and it cost two investigations on 2026-08-31 alone:
# `deliver.b7_redelivery_after_ack_gap` reported `rc=2/0 accepted=1 create=2 acks=2` twice, and
# both times the cause was the runtime's **30 requests / 60 seconds** limit — tripped by a
# `dev/prove.sh` or eval run in the preceding minute, not by anything this script tests. Run alone
# after a full window it scores 4/4 with `create=1`. A rate limit is an unavailable dependency;
# scoring it as FAIL reports a delivery defect that does not exist, and an operator who chases two
# of those learns to skim the ones that matter.
#
# `service_unavailable` is here because the runtime refuses a turn it cannot finish inside
# `DEFAULT_RESPONSES_WAIT_TIMEOUT` (30s) with a 503 — see `crates/ironclaw/src/client.rs`. That is the
# runtime declining, never this script's boundary failing. Deliberately NOT called "the provider
# is degraded": the provider and the runtime are different parties, and only one of them is
# upstream of us.
#
# `HTTP 429` and not a bare `429`, because a bare one matches inside a hex digest and turns a
# clean run into a false BLOCKED. Measured 2026-09-04 in `dev/prove-cutover.sh`, which had copied
# this line: two of eight sittings blocked on runs that had delivered, both on a token like
# `request 4297bd26b3b174a1`. **This script is exposed more, not less** — every check passes the
# output of TWO relay runs to `degraded`, and each prints an alloy digest, a request digest and a
# `resp_` id, so there are six chances per check rather than three.
#
# Narrowing loses no real coverage only if the number is ANCHORED rather than PREFIXED, and the
# first attempt prefixed it. `crates/ironclaw/src/client.rs::interpret_error` carries the error
# envelope's `code`, so a rate limit normally arrives as `rate_limit...` and `rate.?limit` already
# holds it — but its no-envelope branch puts `raw.body` into the text verbatim, and a proxy's own
# `429 Too Many Requests` page carries the number and nothing else on this line. Requiring the
# literal `HTTP 429` dropped exactly that case, and dropping it scores an environment failure as
# FAIL: reporting a defect that is not there, which `dev/prove.sh` names as the worse error.
#
# So the number matches only where it is not inside a hex run. That excludes
# `request 4297bd26b3b174a1` — the digest that produced the false BLOCKEDs — while keeping both
# `HTTP 429: ` (`crates/records/src/append.rs`) and a bare proxy page.
degraded()   { grep -qE "timed out (writing|reading)|rate limited|rate.?limit|(^|[^0-9a-f])429([^0-9a-f]|$)|service_unavailable|provider .* unavailable" <<<"$1"; }
# The failing run's OWN last words, so a BLOCKED verdict names what stopped it. Same shape as
# `dev/prove.sh`'s: the first `ironworks:` line is a success message, so taking the last non-empty
# line is what makes a transient distinguishable from a regression.
why()        { grep -v '^$' <<<"$1" | tail -n1 | cut -c1-88; }
creates()    { awk -F'\t' '$1=="create"' "$1" 2>/dev/null | wc -l | tr -d ' '; }
fetches()    { awk -F'\t' '$1=="fetch"'  "$1" 2>/dev/null | wc -l | tr -d ' '; }
accepted()   { grep -c '^accepted chunk=' <<<"$1" | tr -d ' '; }
digests()    { grep -oE 'sha256=[0-9a-f]+' <<<"$1"; }

# ── 1. known-unsent ──────────────────────────────────────────────────────────────────────
# The channel refuses before accepting any part, so a resend is provably free of duplication.
# What must NOT happen is an automatic resend: the reference corpus asserts against exactly that
# ("a permanent rejection was retried without operator action"), because the same code path also
# handles the ambiguous case where a resend duplicates.
#
# The channel redelivers, so the second run genuinely sees the update again. Without that there
# would be nothing to resend and the check would pass by having nothing to do — the vacuous kind
# of green that makes a gate worse than no gate.
J="$WORK/1.db"; L="$WORK/1.log"
if ! channel "$((UID_BASE+1))" --reject-before-accept --redeliver-unacked; then
  verdict "deliver.known_unsent" BLOCKED "no channel on :$CHAN_PORT"
else
  one="$(relay "$J" "$L")"; rc1=$?
  two="$(relay "$J" "$L")"; rc2=$?
  log="$(obs)"
  if no_model "$one$two"; then verdict "deliver.known_unsent" BLOCKED "runtime has no usable model key"
  elif degraded "$one$two"; then verdict "deliver.known_unsent" BLOCKED "dependency unavailable: $(why "$one$two")"
  elif [ $rc1 -eq 0 ] && [ $rc2 -eq 0 ] \
    && grep -q "delivery=known_unsent" <<<"$one" \
    && grep -q "a resend is safe; nothing reached the client" <<<"$one" \
    && grep -q "already settled (delivery=known_unsent)" <<<"$two" \
    && [ "$(accepted "$log")" = 0 ] \
    && [ "$(grep -c '^rejected chunk=' <<<"$log")" = 1 ] \
    && [ "$(creates "$L")" = 1 ]; then
    verdict "deliver.known_unsent" PASS "1 attempt, 0 chunks accepted, no automatic retry"
  else
    verdict "deliver.known_unsent" FAIL \
      "rc=$rc1/$rc2 accepted=$(accepted "$log") rejected=$(grep -c '^rejected chunk=' <<<"$log") create=$(creates "$L")"
  fi
fi

sleep 5

# ── 2. B5: the residual window ───────────────────────────────────────────────────────────
# The channel has the answer; the process died before writing that down. A duplicate is
# unavoidable — and it must be the SAME answer, which is asserted by comparing the two chunk
# digests the channel recorded, not by counting sends.
J="$WORK/2.db"; L="$WORK/2.log"
if ! channel "$((UID_BASE+2))"; then
  verdict "deliver.b5_residual_window" BLOCKED "no channel on :$CHAN_PORT"
else
  one="$(relay "$J" "$L" --crash-at after-send)"; rc1=$?
  two="$(relay "$J" "$L")"; rc2=$?
  log="$(obs)"
  first="$(digests "$log" | head -n1)"
  second="$(digests "$log" | tail -n1)"
  if no_model "$one$two"; then verdict "deliver.b5_residual_window" BLOCKED "runtime has no usable model key"
  elif degraded "$one$two"; then verdict "deliver.b5_residual_window" BLOCKED "dependency unavailable: $(why "$one$two")"
  elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] \
    && grep -q "CRASHING at after-send" <<<"$one" \
    && [ "$(accepted "$log")" = 2 ] \
    && [ -n "$first" ] && [ "$first" = "$second" ] \
    && [ "$(creates "$L")" = 1 ] && [ "$(fetches "$L")" -ge 1 ] \
    && grep -q "fetching known response resp_" <<<"$two"; then
    verdict "deliver.b5_residual_window" PASS \
      "2 sends, identical (${first#sha256=}), create=1 fetch=$(fetches "$L")"
  else
    verdict "deliver.b5_residual_window" FAIL \
      "rc=$rc1/$rc2 accepted=$(accepted "$log") first=$first second=$second create=$(creates "$L")"
  fi
fi

sleep 5

# ── 3. partial delivery, and evidence that cannot be lowered ─────────────────────────────
# One chunk lands and is confirmed; the next is accepted and the reply is lost. The count is the
# only record of whether a resend duplicates content a client can already read — so the second
# half of this check restarts the channel under a DIFFERENT fault, one that reports "nothing was
# sent", and asserts the stored evidence does not fall back to it. `bridge redeliver` is designed
# to be retried, so a failed retry is the normal path, and it is where the reference
# implementation lost `delivery_partial`.
J="$WORK/3.db"; L="$WORK/3.log"
if ! channel "$((UID_BASE+3))" --fail-after-chunk 1; then
  verdict "deliver.partial_evidence_holds" BLOCKED "no channel on :$CHAN_PORT"
else
  # A small chunk limit so a short answer still spans several messages.
  one="$(relay "$J" "$L" --chunk-bytes 40)"; rc1=$?
  log="$(obs)"
  partial=0
  grep -q "delivery=uncertain (1 chunk(s) confirmed in the client's room)" <<<"$one" && partial=1

  # A new channel process that refuses everything. Same journal, so the same evidence.
  channel "$((UID_BASE+3))" --reject-before-accept
  two="$(redeliver "$J" "$L" "$((UID_BASE+3))")"; rc2=$?
  after="$(grep -oE "update $((UID_BASE+3)) delivery=[a-z]+" <<<"$two" | tail -n1)"

  if no_model "$one$two"; then verdict "deliver.partial_evidence_holds" BLOCKED "runtime has no usable model key"
  elif degraded "$one$two"; then verdict "deliver.partial_evidence_holds" BLOCKED "dependency unavailable: $(why "$one$two")"
  elif [ $rc1 -eq 0 ] && [ "$partial" = 1 ] \
    && [ "$(accepted "$log")" -ge 2 ] \
    && grep -q "NEEDS AN OPERATOR — a resend may put a second copy" <<<"$one" \
    && [ "$after" = "update $((UID_BASE+3)) delivery=uncertain" ] \
    && [ "$(creates "$L")" = 1 ]; then
    verdict "deliver.partial_evidence_holds" PASS \
      "1 chunk confirmed; a later 'nothing sent' did not lower it"
  else
    verdict "deliver.partial_evidence_holds" FAIL \
      "rc=$rc1/$rc2 partial=$partial accepted=$(accepted "$log") after='$after' create=$(creates "$L")"
  fi
fi

sleep 5

# ── 4. B7: delivered, then death before the offset went out ──────────────────────────────
# The answer is in the client's room and the channel has not been told to stop sending the
# question. It redelivers. There must be no rerun and no resend — only an acknowledgement, which
# the next poll's offset carries. That the acknowledgement arrives on a LATER poll than the
# delivery is the whole of DELIVERED != ACKED.
J="$WORK/4.db"; L="$WORK/4.log"
if ! channel "$((UID_BASE+4))" --redeliver-unacked; then
  verdict "deliver.b7_redelivery_after_ack_gap" BLOCKED "no channel on :$CHAN_PORT"
else
  one="$(relay "$J" "$L" --crash-at after-delivered)"; rc1=$?
  mid="$(obs)"
  two="$(relay "$J" "$L")"; rc2=$?
  log="$(obs)"
  if no_model "$one$two"; then verdict "deliver.b7_redelivery_after_ack_gap" BLOCKED "runtime has no usable model key"
  elif degraded "$one$two"; then verdict "deliver.b7_redelivery_after_ack_gap" BLOCKED "dependency unavailable: $(why "$one$two")"
  elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] \
    && grep -q "CRASHING at after-delivered" <<<"$one" \
    && [ "$(grep -c 'acknowledged through=' <<<"$mid")" = 1 ] \
    && grep -q "^redelivered update=$((UID_BASE+4))" <<<"$log" \
    && grep -q "already settled (delivery=delivered) — no rerun, no resend" <<<"$two" \
    && [ "$(accepted "$log")" = 1 ] \
    && [ "$(creates "$L")" = 1 ] \
    && grep -q "^acknowledged through=$((UID_BASE+5))" <<<"$log"; then
    verdict "deliver.b7_redelivery_after_ack_gap" PASS \
      "redelivered; create=1 sends=1, offset advanced past the update"
  else
    verdict "deliver.b7_redelivery_after_ack_gap" FAIL \
      "rc=$rc1/$rc2 accepted=$(accepted "$log") create=$(creates "$L") acks=$(grep -c 'acknowledged through=' <<<"$log")"
  fi
fi

# ── verdict ──────────────────────────────────────────────────────────────────────────────
printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
printf 'chunk digests come from the channel, so an identical duplicate is proven, not inferred.\n'
printf 'create counts are taken at the runtime adapter boundary and survive each restart.\n'
# Measured: running this and dev/prove.sh back to back trips the runtime's 30-requests-per-60-
# seconds limit, and a rate-limited turn fails in a way that reads exactly like a recovery defect.
# It cost one false FAIL on B2 before it was diagnosed.
printf 'wait ~60s before running dev/prove.sh: the runtime allows 30 requests per 60 seconds.\n'
[ "$failed" -gt 0 ] && exit 2
[ "$blocked" -gt 0 ] && exit 3
exit 0
