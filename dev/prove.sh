#!/usr/bin/env bash
# Prove the live chain, the refusals that bound it, and recovery across process death.
#
#   dev/probe-up.sh          # runtime + disposable record store
#   dev/prove.sh             # this
#
# Nine checks. The chain establishes that it works; everything after establishes that it stops,
# degrades, or recovers where it should. A chain that only ever succeeds proves nothing about a
# control.
#
#   1. own-org control           the right credential returns DATA — no model call
#   2. the chain                 credential -> server-resolved org -> verified instance
#                                -> scoped records -> ModelTurn -> IronClaw response
#   3. wrong credential          the store refuses; nothing serves
#   4. scope drift               the store changes its answer; the turn is REFUSED
#   5. post-verification outage  the store goes away; the turn DEGRADES and still serves
#   6. B2 crash before dispatch  restart dispatches the ORIGINAL logical turn
#   7. B3 crash before the id    restart REPLAYS the same key and recovers the original response
#   8. B4 crash after the id     restart FETCHES; create is never called again
#   9. B5 crash after stdout     restart prints the SAME response, then settles delivery
#
# ── why check 1 exists, and why it must not need a model ────────────────────────────────────
#
# AN EMPTY STORE PASSES EVERY ISOLATION CHECK. Checks 3 and 4 are refusals: they establish that a
# wrong credential and a drifted scope yield nothing. Neither distinguishes isolation from a store
# that would have returned nothing anyway, so both are vacuous without a check proving the own-org
# case returns DATA.
#
# That control used to be check 2, the full chain — which needs a completed model turn. So the
# control failed exactly when the provider was unavailable, which is when the refusals most needed
# scoring. Measured at the 1.4.0 qualification: the provider rate-limited the chain, the two
# refusals scored PASS, and that run distinguished nothing. See docs/UPGRADE.md.
#
# So the control is split out and made model-free. `--crash-at before-dispatch` builds the envelope
# from real records under the real credential and exits at the named point, having contacted no
# runtime at all — which is why this check is the one that still scores when everything upstream of
# IronWorks is down.
#
# ── what the call counts prove, and what they do not ────────────────────────────────────────
#
# Duplicate execution is NOT inferred from equal output: two executions of one prompt can produce
# the same words. Calls are counted at the runtime adapter boundary, in a log that outlives each
# process, and the assertions are about calls and identities.
#
# The pinned runtime exposes no model-execution counter, so what is proven is precisely this:
#
#   * B3 issues TWO create calls carrying ONE idempotency identity, and the second returns the
#     SAME response id the first produced and discarded. Per the measured contract, a repeated key
#     with a byte-identical body is served from the idempotency index rather than re-executed.
#   * B4 issues exactly ONE create call in total; the restart fetches.
#   * B5 issues exactly ONE create call in total; the restart fetches the same response id and
#     the direct terminal delivery then disappears from `status`.
#
# What is NOT proven: that the provider's model was invoked exactly once. That would need a
# counter the runtime does not expose. The response identity is the strongest evidence available,
# and a second logical turn would necessarily carry a different response id.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

STORE_PORT="${PROBE_STORE_PORT:-3996}"
NEG_PORT="${PROVE_NEG_PORT:-3995}"
STORE_TOKEN="dev-token-eval"
GUIDANCE="fixtures/eval.guidance.md"
SLUG="eval"
SERVICE="relationship-intelligence"
ASK="Name the single account with a confirmed budget. One line."
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-prove.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

pass=0; failed=0; blocked=0
verdict() {
  printf '%-9s %-30s %s\n' "$2" "$1" "${3:-}"
  case "$2" in PASS) pass=$((pass+1));; FAIL) failed=$((failed+1));; BLOCKED) blocked=$((blocked+1));; esac
}

# ── preconditions ────────────────────────────────────────────────────────────────────────
[ -n "${IRONWORKS_BASE:-}" ] && [ -n "${IRONWORKS_TOKEN:-}" ] || {
  echo "prove: IRONWORKS_BASE / IRONWORKS_TOKEN are unset. Run dev/probe-up.sh and export the" >&2
  echo "       lines it prints." >&2
  exit 3
}
curl -fsS --max-time 3 "$IRONWORKS_BASE/api/health" >/dev/null 2>&1 || {
  verdict "runtime.reachable" BLOCKED "$IRONWORKS_BASE did not answer /api/health"; exit 3; }
curl -fsS --max-time 3 "http://127.0.0.1:$STORE_PORT/health" >/dev/null 2>&1 || {
  verdict "store.reachable" BLOCKED "no record store on :$STORE_PORT (dev/probe-up.sh)"; exit 3; }
cargo build -q -p ironworks-cli -p ironworks-devstore 2>/dev/null || {
  verdict "build" BLOCKED "cargo build failed"; exit 3; }
BIN=./target/debug/ironworks
STORE_BIN=./target/debug/ironworks-devstore
HEALTHY="http://127.0.0.1:$STORE_PORT"

neg_store() {
  ( exec "$STORE_BIN" --port "$NEG_PORT" --book "$SLUG=fixtures/book" \
      --identity "$STORE_TOKEN=$SLUG" "$1" "$2" ) >/dev/null 2>&1 </dev/null &
  local pid=$!
  disown 2>/dev/null || true
  for _ in $(seq 1 30); do
    curl -fsS --max-time 1 "http://127.0.0.1:$NEG_PORT/health" >/dev/null 2>&1 && { echo "$pid"; return; }
    sleep 0.2
  done
  kill "$pid" 2>/dev/null; echo ""
}

# turn <journal> <call-log> <store-url> <records-token> [crash-at]
turn() {
  local j="$1" cl="$2" store="$3" tok="$4" crash="${5:-}"
  local extra=()
  [ -n "$crash" ] && extra=(--crash-at "$crash")
  IRONWORKS_RECORDS="$store" IRONWORKS_RECORDS_TOKEN="$tok" \
    "$BIN" turn --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" --ask "$ASK" \
      --journal "$j" --call-log "$cl" ${extra[@]+"${extra[@]}"} 2>&1
}

no_model() { grep -q "not a valid model name\|model provider rejected" <<<"$1"; }
# The model PROVIDER, upstream of the runtime, rate limits. It surfaces here as a send timeout or
# an unknown turn outcome — which are real and important states, handled correctly by the code
# under test, but they are not evidence about the chain. Measured: the runtime answers
# /api/health in ~2ms while its own log reports `Provider nearai_chat rate limited`. A proof that
# scored this as FAIL would be reporting a defect in IronWorks for something upstream of it.
#
# `service_unavailable` is the SAME CLASS and was missing. It is the runtime's 30-second wait
# timeout (`DEFAULT_RESPONSES_WAIT_TIMEOUT`, DESIGN.md): the provider outran the wait, the runtime
# returned 503, and the turn kept running server-side. Nothing in IronWorks did anything wrong —
# the seam recovers it by byte-identical replay, which is a different check's job to prove. Left
# unmatched it scored `chain.end_to_end` and `recover.b2_before_dispatch` as FAIL on a run where
# the provider was merely slow: measured 11 refusals in 17 asks of the shipped question
# (`docs/experiments/0036-the-demo-question-lands-on-the-thirty-second-cliff.md`), which turned a
# green suite red without a line of this repository changing.
#
# **This does not make the run a pass.** A BLOCKED `chain.end_to_end` is still the positive
# control the two isolation refusals need, so a sitting that blocks it measured nothing about
# isolation either — §J's rule, and the same reading `docs/INVARIANTS.md` §K records for the
# 1.4.0 qualification. It reports the right thing rather than the wrong one.
degraded() {
  grep -qE "timed out (writing|reading)|rate limited|provider .* unavailable|service_unavailable" <<<"$1"
}
creates()  { awk -F'\t' '$1=="create"' "$1" 2>/dev/null | wc -l | tr -d ' '; }
fetches()  { awk -F'\t' '$1=="fetch"'  "$1" 2>/dev/null | wc -l | tr -d ' '; }
keys()     { awk -F'\t' '$1=="create"{print $2}' "$1" 2>/dev/null | sort -u | wc -l | tr -d ' '; }
resp_of()  { grep -oE 'resp_[0-9a-f]+' <<<"$1" | head -n1; }
# The failing run's OWN last words. An earlier version grepped the first `ironworks:` line, which
# is the "scope verified" success message — so every FAIL reported the wrong sentence and a
# transient looked identical to a regression.
why()      { grep -v '^$' <<<"$1" | tail -n1 | cut -c1-88; }
# A recovery check needs its crash to actually happen. If the deliberate exit (75) did not occur,
# the boundary was never set up — the runtime timed out, or refused, or rate-limited — and the
# check has measured nothing. That is BLOCKED, not FAIL: scoring it as a failure reports a defect
# in the recovery path when what happened was an unavailable dependency, and an operator who
# investigates two of those learns to skim the ones that matter.
crashed()  { [ "$1" -eq 75 ]; }

# ── 1. own-org control: the right credential returns DATA, with no model call ────────────
# No `no_model`/`degraded` arm on purpose. Nothing upstream of IronWorks is reachable on this
# path, so there is no unavailable dependency to excuse a miss: if this does not reach the crash
# point with 40 records, that is a defect in what the check measures.
out="$(turn "$WORK/0.db" "$WORK/0.log" "$HEALTHY" "$STORE_TOKEN" before-dispatch)"; rc=$?
if crashed "$rc" && grep -q "scope $SLUG verified by" <<<"$out" \
   && grep -q "40 record(s) \[complete\]" <<<"$out" \
   && [ "$(creates "$WORK/0.log")" = 0 ]; then
  verdict "isolation.own_org_control" PASS "40 records under the right credential; 0 create calls"
else
  verdict "isolation.own_org_control" FAIL \
    "rc=$rc creates=$(creates "$WORK/0.log") | $(why "$out")"
fi

# ── 2. the chain ─────────────────────────────────────────────────────────────────────────
out="$(turn "$WORK/1.db" "$WORK/1.log" "$HEALTHY" "$STORE_TOKEN")"; rc=$?
if no_model "$out"; then verdict "chain.end_to_end" BLOCKED "runtime has no usable model key"
elif degraded "$out"; then verdict "chain.end_to_end" BLOCKED "the model provider is degraded: $(why "$out")"
elif [ $rc -eq 0 ] && grep -q "scope $SLUG verified by" <<<"$out" \
  && grep -q "40 record(s) \[complete\]" <<<"$out" && grep -q "response resp_" <<<"$out"; then
  verdict "chain.end_to_end" PASS "$(tail -n1 <<<"$out" | cut -c1-52)"
else verdict "chain.end_to_end" FAIL "rc=$rc $(why "$out")"; fi

# ── 3. wrong credential ──────────────────────────────────────────────────────────────────
out="$(turn "$WORK/2.db" "$WORK/2.log" "$HEALTHY" "not-a-real-token")"; rc=$?
if [ $rc -ne 0 ] && grep -q "refused this credential" <<<"$out" && [ "$(creates "$WORK/2.log")" = 0 ]; then
  verdict "negative.wrong_credential" PASS "refused; 0 create calls"
else verdict "negative.wrong_credential" FAIL "expected a refusal, rc=$rc | $(why "$out")"; fi

# ── 4. scope drift ───────────────────────────────────────────────────────────────────────
pid="$(neg_store --drift-after 1)"
if [ -z "$pid" ]; then verdict "negative.scope_drift" BLOCKED "no drifting store"
else
  out="$(turn "$WORK/3.db" "$WORK/3.log" "http://127.0.0.1:$NEG_PORT" "$STORE_TOKEN")"; rc=$?
  kill "$pid" 2>/dev/null
  if [ $rc -ne 0 ] && grep -q "SCOPE DRIFT" <<<"$out" && grep -q "REFUSING TO SERVE" <<<"$out" \
    && [ "$(creates "$WORK/3.log")" = 0 ]; then
    verdict "negative.scope_drift" PASS "refused; 0 create calls"
  else verdict "negative.scope_drift" FAIL "expected a refusal, rc=$rc | $(why "$out")"; fi
fi

# ── 5. post-verification record outage ───────────────────────────────────────────────────
pid="$(neg_store --fail-after 1)"
if [ -z "$pid" ]; then verdict "degrade.record_outage" BLOCKED "no failing store"
else
  out="$(turn "$WORK/4.db" "$WORK/4.log" "http://127.0.0.1:$NEG_PORT" "$STORE_TOKEN")"; rc=$?
  kill "$pid" 2>/dev/null
  if no_model "$out"; then verdict "degrade.record_outage" BLOCKED "runtime has no usable model key"
  elif degraded "$out"; then verdict "degrade.record_outage" BLOCKED "upstream is degraded or outran the 30s wait"
  elif [ $rc -eq 0 ] && grep -q "0 record(s) \[DEGRADED\]" <<<"$out" \
    && grep -q "response resp_" <<<"$out"; then
    verdict "degrade.record_outage" PASS "verified scope kept; served with no records"
  else verdict "degrade.record_outage" FAIL "expected a degraded turn, rc=$rc | $(why "$out")"; fi
fi

# The runtime allows 30 requests per 60 seconds per user, and each check above is a create plus
# polls. Pace the recovery checks so a rate limit cannot masquerade as a recovery failure.
sleep 5

# ── 6. B2: crash before dispatch ─────────────────────────────────────────────────────────
# The key and the body digest are durable; nothing was sent. The restart dispatches the ORIGINAL
# logical turn — same key, same timestamp, therefore the same bytes.
J="$WORK/b2.db"; L="$WORK/b2.log"
one="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN" before-dispatch)"; rc1=$?
two="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN")"; rc2=$?
if no_model "$two"; then verdict "recover.b2_before_dispatch" BLOCKED "runtime has no usable model key"
elif degraded "$one$two"; then
  verdict "recover.b2_before_dispatch" BLOCKED "upstream is degraded or outran the 30s wait"
elif ! crashed "$rc1"; then
  verdict "recover.b2_before_dispatch" BLOCKED "the crash point was never reached: $(why "$one")"
elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] \
  && grep -q "CRASHING at before-dispatch" <<<"$one" \
  && [ "$(creates "$L")" = 1 ] && [ "$(keys "$L")" = 1 ] \
  && grep -q "recovering .*NotSubmitted" <<<"$two"; then
  verdict "recover.b2_before_dispatch" PASS "create=1 keys=1 (dispatched once)"
else
  verdict "recover.b2_before_dispatch" FAIL \
    "rc=$rc1/$rc2 create=$(creates "$L") keys=$(keys "$L") | $(why "$one$two")"
fi

sleep 5

# ── 7. B3: crash after acceptance, before the id is durable ──────────────────────────────
# The architectural improvement. The runtime accepted a turn we never recorded. The restart
# reconstructs the body byte-identically — reusing the journalled retrieved_at rather than reading
# a fresh clock — and replays the SAME key, recovering the SAME response id.
J="$WORK/b3.db"; L="$WORK/b3.log"
one="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN" before-response)"; rc1=$?
discarded="$(grep -oE 'DISCARDING response resp_[0-9a-f]+' <<<"$one" | grep -oE 'resp_[0-9a-f]+')"
two="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN")"; rc2=$?
recovered="$(resp_of "$(grep -m1 'response resp_' <<<"$two")")"
if no_model "$one$two"; then verdict "recover.b3_replay_identical" BLOCKED "runtime has no usable model key"
elif degraded "$one$two"; then
  verdict "recover.b3_replay_identical" BLOCKED "upstream is degraded or outran the 30s wait"
elif ! crashed "$rc1"; then
  verdict "recover.b3_replay_identical" BLOCKED "the crash point was never reached: $(why "$one")"
elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] \
  && [ -n "$discarded" ] && [ "$discarded" = "$recovered" ] \
  && [ "$(creates "$L")" = 2 ] && [ "$(keys "$L")" = 1 ] \
  && grep -q "byte-identical" <<<"$two"; then
  verdict "recover.b3_replay_identical" PASS \
    "create=2 keys=1 -> $recovered (one logical turn)"
else
  verdict "recover.b3_replay_identical" FAIL \
    "rc=$rc1/$rc2 create=$(creates "$L") keys=$(keys "$L") | $(why "$one$two")"
fi

sleep 5

# ── 8. B4: crash after the id is durable ─────────────────────────────────────────────────
# The response is known, so the restart fetches it. `create` is never called again.
J="$WORK/b4.db"; L="$WORK/b4.log"
one="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN" before-commit)"; rc1=$?
durable="$(grep -oE 'response resp_[0-9a-f]+ is durable' <<<"$one" | grep -oE 'resp_[0-9a-f]+')"
before="$(creates "$L")"
two="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN")"; rc2=$?
if no_model "$one$two"; then verdict "recover.b4_fetch_not_create" BLOCKED "runtime has no usable model key"
elif degraded "$one$two"; then
  verdict "recover.b4_fetch_not_create" BLOCKED "upstream is degraded or outran the 30s wait"
elif ! crashed "$rc1"; then
  verdict "recover.b4_fetch_not_create" BLOCKED "the crash point was never reached: $(why "$one")"
elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] \
  && [ "$before" = 1 ] && [ "$(creates "$L")" = 1 ] && [ "$(fetches "$L")" -ge 1 ] \
  && grep -q "fetching known response $durable (no create)" <<<"$two"; then
  verdict "recover.b4_fetch_not_create" PASS \
    "create=1 total, fetch=$(fetches "$L") -> $durable"
else
  verdict "recover.b4_fetch_not_create" FAIL \
    "rc=$rc1/$rc2 create=$(creates "$L") fetch=$(fetches "$L") | $(why "$one$two")"
fi

sleep 5

# ── 9. B5: stdout accepted the answer before delivery became durable ────────────────────
# The unavoidable direct-terminal window. The restart may print an identical duplicate, but it
# fetches the known response rather than executing another model turn. A successful flush on the
# restart records delivery, so the persistent journal can serve a later question.
J="$WORK/b5.db"; L="$WORK/b5.log"
one="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN" after-output)"; rc1=$?
durable="$(resp_of "$(grep -m1 'response resp_' <<<"$one")")"
two="$(turn "$J" "$L" "$HEALTHY" "$STORE_TOKEN")"; rc2=$?
status="$($BIN status --journal "$J" 2>&1)"; rc3=$?
if no_model "$one$two"; then verdict "recover.b5_terminal_delivery" BLOCKED "runtime has no usable model key"
elif degraded "$one$two"; then
  verdict "recover.b5_terminal_delivery" BLOCKED "upstream is degraded or outran the 30s wait"
elif ! crashed "$rc1"; then
  verdict "recover.b5_terminal_delivery" BLOCKED "the crash point was never reached: $(why "$one")"
elif [ $rc1 -eq 75 ] && [ $rc2 -eq 0 ] && [ $rc3 -eq 0 ] \
  && [ "$(creates "$L")" = 1 ] && [ "$(fetches "$L")" -ge 1 ] \
  && grep -q "fetching known response $durable (no create)" <<<"$two" \
  && grep -q "terminal delivery=delivered" <<<"$two" \
  && grep -q "0 unresolved turn(s)" <<<"$status"; then
  verdict "recover.b5_terminal_delivery" PASS \
    "create=1, fetched $durable, terminal delivery settled"
else
  verdict "recover.b5_terminal_delivery" FAIL \
    "rc=$rc1/$rc2/$rc3 create=$(creates "$L") fetch=$(fetches "$L") | $(why "$one$two$status")"
fi

# ── verdict ──────────────────────────────────────────────────────────────────────────────
printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
printf 'call counts are taken at the runtime adapter boundary and survive each restart.\n'
printf 'chain requests are counted from the first non-health store request; /health is excluded.\n'
[ "$failed" -gt 0 ] && exit 2
[ "$blocked" -gt 0 ] && exit 3
exit 0
