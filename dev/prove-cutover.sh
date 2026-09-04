#!/usr/bin/env bash
# Prove a registry cutover through the SERVING process, not through the loader.
#
#   dev/probe-up.sh              # runtime + disposable record store
#   dev/prove-cutover.sh         # this: publish -> serve -> roll back -> serve again
#
# Four checks. `crates/cli/tests/registry_publish.rs` already proves the file half — that
# publish-one writes durably, refuses a wrong identity, and that what comes back parses. It cannot
# prove the half an operator actually cares about at a cutover, because parsing a registry is not
# serving from one. Every check below therefore runs the real binaries as separate processes and
# reads the answer out of the channel.
#
#   1. publication              a valid prior registry is replaced by a certified candidate, and
#                               the prior survives as the durable rollback artifact
#   2. candidate serves         a real relay against the PUBLISHED live path answers into the
#                               candidate's room, and does not answer into the prior's
#   3. rollback                 the prior registry is restored, the artifact is not consumed
#   4. prior serves after       a NEW channel process and a NEW relay process against the SAME
#      restart                  live path answer into the prior's room, and not the candidate's
#
# ── why each serving check asserts a negative in the same run ────────────────────────────────
#
# "An answer was delivered" is not evidence of a cutover: a stale registry delivers answers too.
# So both updates are offered to the relay in one batch — one addressed to the candidate's room,
# one to the prior's — and the check asserts which room received chunks AND that the other did
# not. Check 4 is check 2 with the rooms exchanged, against the same channel fixture and the same
# journal. Read 2+4 as a pair: either half alone would pass against a relay that always served
# whichever tenant it happened to hold.
#
# The serving assertions come from the channel's own observation log
# (`accepted chunk=N channel=...`), which is why `ironworks-devchannel` records the room. The
# publication and rollback assertions are file-side: they inspect the live registry and its
# durable rollback artifact. Nothing here parses `TenantRegistry` as a Rust type, inspects the
# fixture's memory, or believes the relay's account of itself: the relay's stderr is printed as
# context and scored on nothing.
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves that publishing changes who a real serving process answers for, that rolling back
# changes it back, and that a restart across the boundary reads the file rather than a cached
# routing table. It bills two model turns, one per serving check, against the disposable probe.
#
# It does NOT prove anything about a production host, a real channel, or two tenants at once:
# every publication state here holds exactly one tenant, which is what publish-one enforces.
#
# ── the residual ─────────────────────────────────────────────────────────────────────────────
#
# The candidate is a member minted for this run and confined through the real command, so the
# run ends with one spent member on the probe — approval-off is one-way (§L-9) and this script
# cannot put it back. It is used by nothing else and goes away with `dev/probe-up.sh --down`;
# consecutive runs mint their own, so the probe stays reusable.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT" || exit 3

STORE_PORT="${PROBE_STORE_PORT:-3996}"
CHAN_PORT="${PROVE_CUTOVER_PORT:-3992}"
STORE_TOKEN="dev-token-eval"
SERVICE="relationship-intelligence"
CANDIDATE="cutover-candidate"
PRIOR="cutover-prior"
# Rooms, not names, are what a registry routes on, so they are what the two tenants differ in.
CAND_ROOM="-1002000001"
PRIOR_ROOM="-1002000002"
# Deliberately the cheapest question that still costs a real, verified, booted turn. This proof
# is about which tenant answers, not about how well: a heavier ask spends longer inside the
# runtime's 30s response window and turns a routing proof into an availability one.
ASK="Reply with the single word: ready."
# Fresh per run, for the reason `dev/prove-delivery.sh` sets out at length: the relay derives an
# idempotency key from the update id, and a repeated id would collide with a turn the runtime
# still remembers under a body built at a different second.
UID_BASE=$(( $(date +%s) % 900000 + 1000 ))

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-cutover.XXXXXX")"
LIVE_DIR="$WORK/live"
LIVE="$LIVE_DIR/registry.json"
ROLLBACK="$LIVE.previous"
STORE="$WORK/lifecycle.store"
JOURNAL="$WORK/relay.db"
CHAN_PID=""
minted=0
# Invoked indirectly by the EXIT, INT and TERM traps below.
# shellcheck disable=SC2329
cleanup() {
  [ -n "$CHAN_PID" ] && kill "$CHAN_PID" 2>/dev/null
  rm -rf "$WORK"
  if [ "$minted" -eq 1 ]; then
    printf '\nprove-cutover: the candidate member was minted for this run and is left confined.\n'
    printf 'prove-cutover: nothing else uses it; dev/probe-up.sh --down discards it.\n'
  fi
}
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM
trap cleanup EXIT

pass=0; failed=0; blocked=0
verdict() {
  printf '%-9s %-34s %s\n' "$2" "$1" "${3:-}"
  case "$2" in PASS) pass=$((pass+1));; FAIL) failed=$((failed+1));; BLOCKED) blocked=$((blocked+1));; esac
}

# ── preconditions ────────────────────────────────────────────────────────────────────────
[ -n "${IRONWORKS_BASE:-}" ] && [ -n "${IRONWORKS_TOKEN:-}" ] || {
  echo "prove-cutover: IRONWORKS_BASE / IRONWORKS_TOKEN are unset. Run dev/probe-up.sh and" >&2
  echo "               export the lines it prints." >&2
  exit 3
}
curl -fsS --max-time 3 "$IRONWORKS_BASE/api/health" >/dev/null 2>&1 || {
  verdict "runtime.reachable" BLOCKED "$IRONWORKS_BASE did not answer /api/health"; exit 3; }
curl -fsS --max-time 3 "http://127.0.0.1:$STORE_PORT/health" >/dev/null 2>&1 || {
  verdict "store.reachable" BLOCKED "no record store on :$STORE_PORT (dev/probe-up.sh)"; exit 3; }
if curl -fsS --max-time 2 "http://127.0.0.1:$CHAN_PORT/health" >/dev/null 2>&1; then
  verdict "channel.port" BLOCKED \
    "something already answers on $CHAN_PORT; set PROVE_CUTOVER_PORT to a free port"
  exit 3
fi
cargo build -q -p ironworks-cli -p ironworks-devchannel 2>/dev/null || {
  verdict "build" BLOCKED "cargo build failed"; exit 3; }
BIN=./target/debug/ironworks
CHAN_BIN=./target/debug/ironworks-devchannel
STORE_URL="http://127.0.0.1:$STORE_PORT"
CHAN="http://127.0.0.1:$CHAN_PORT"

# ── the two tenants ──────────────────────────────────────────────────────────────────────
# Same service and the same book on purpose. If they differed in what they answer, a check could
# pass on the content and never test the route; making the ROOM the only difference is what makes
# the observation log the whole of the evidence.
mkdir -p "$LIVE_DIR" || { verdict "workspace" BLOCKED "cannot create $LIVE_DIR"; exit 3; }
CAND_GUIDANCE="$WORK/$CANDIDATE.guidance.md"
PRIOR_GUIDANCE="$WORK/$PRIOR.guidance.md"
sed "1s|slug: eval|slug: $CANDIDATE|" fixtures/eval.guidance.md > "$CAND_GUIDANCE"
sed "1s|slug: eval|slug: $PRIOR|" fixtures/eval.guidance.md > "$PRIOR_GUIDANCE"
if ! grep -q "slug: $CANDIDATE" "$CAND_GUIDANCE" \
  || ! grep -q "slug: $PRIOR" "$PRIOR_GUIDANCE"; then
  verdict "guidance" BLOCKED "fixtures/eval.guidance.md no longer carries the expected marker"
  exit 3
fi

# Credentials reach every process as env:NAME handles, never as argv and never in a file. The
# registry records the handle; the secret is only ever in this shell's environment.
export CUTOVER_RECORDS="$STORE_TOKEN"
export CUTOVER_PRIOR_RECORDS="$STORE_TOKEN"
export CUTOVER_PRIOR_RUNTIME="$IRONWORKS_TOKEN"
export IRONWORKS_RECORDS="$STORE_URL"
export IRONCLAW_API="$IRONWORKS_BASE"

cat > "$LIVE" <<JSON
{"tenants":[{"slug":"$PRIOR","channel":"$PRIOR_ROOM","service":"$SERVICE",
  "guidance":"$PRIOR_GUIDANCE",
  "records_credential":"env:CUTOVER_PRIOR_RECORDS",
  "runtime_credential":"env:CUTOVER_PRIOR_RUNTIME"}]}
JSON
chmod 600 "$LIVE"

# ── the candidate's authority, established the way an operator establishes it ─────────────
# Minted, adopted and confined through the real commands, because publish-one refuses anything
# less: it requires a certified `member_confined` stage whose recorded org and runtime subject
# both re-measure. None of this is scored here — a failure to set up a cutover is a measurement
# that did not happen, and `dev/prove-confinement.sh` is the script that scores confinement.
OPERATOR="${IRONCLAW_OPERATOR_TOKEN:-$IRONWORKS_TOKEN}"
export IRONCLAW_OPERATOR_TOKEN="$OPERATOR"
minted_json="$(curl -s --max-time 20 -X POST "$IRONWORKS_BASE/api/webchat/v2/admin/users" \
  --config - -H 'content-type: application/json' \
  -d "{\"email\":\"cutover-$$@example.com\",\"display_name\":\"cutover probe $$\",\"role\":\"member\"}" <<CURL
header = "authorization: Bearer $OPERATOR"
CURL
)"
# `api_token` is returned once and never again. A mint whose token we could not read is a member
# we can no longer act as — BLOCKED, and never a fallback to the operator bearer, which would
# measure a caller no tenant will ever have.
CUTOVER_MEMBER="$(printf '%s' "$minted_json" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("api_token") or "")
except Exception: pass')"
if [ -z "$CUTOVER_MEMBER" ]; then
  verdict "candidate.member" BLOCKED "could not mint a member: $(printf '%s' "$minted_json" | head -c 120)"
  exit 3
fi
minted=1
export CUTOVER_MEMBER IRONCLAW_MEMBER_TOKEN="$CUTOVER_MEMBER"

if ! adopt="$("$BIN" lifecycle adopt "$CANDIDATE" --store "$STORE" --service "$SERVICE" \
  --channel "$CAND_ROOM" --records-credential env:CUTOVER_RECORDS \
  --member-credential env:CUTOVER_MEMBER 2>&1)"; then
  verdict "candidate.adopted" BLOCKED "$(grep -v '^$' <<<"$adopt" | tail -n1 | cut -c1-84)"; exit 3
fi
if ! confine="$("$BIN" lifecycle confine "$CANDIDATE" --store "$STORE" 2>&1)"; then
  verdict "candidate.confined" BLOCKED "$(grep -v '^$' <<<"$confine" | tail -n1 | cut -c1-84)"
  exit 3
fi
printf 'prove-cutover: candidate adopted and confined; prior registry is live at %s\n' \
  "$(basename "$LIVE")"

# Confinement writes one settings row per catalogued tool — comfortably more than the runtime's
# 30 requests per 60 seconds — and check 2's turn is the very next thing this script asks of it.
# Measured on 2026-09-04: without this wait check 2 reported `service_unavailable` while the same
# member served a turn cleanly a minute later, which is a rate limit reported as a cutover defect.
# The window is the runtime's, so waiting it out is the only way to measure what is downstream.
printf 'prove-cutover: waiting 60s for the runtime request window to clear after confinement\n\n'
sleep 60

# ── helpers ──────────────────────────────────────────────────────────────────────────────

# channel <id=room> <id=room>
# A fresh disposable channel per serving check. Restarting it is what keeps each check's
# observation log its own, so "the prior's room received nothing" is a statement about THIS
# cutover state and not about a log that accumulated across both.
channel() {
  [ -n "$CHAN_PID" ] && { kill "$CHAN_PID" 2>/dev/null; wait "$CHAN_PID" 2>/dev/null; CHAN_PID=""; }
  local args=()
  local pair
  for pair in "$@"; do args+=(--update "${pair%%=*}=${pair#*=}:$ASK"); done
  ( exec "$CHAN_BIN" --port "$CHAN_PORT" "${args[@]}" ) >/dev/null 2>&1 </dev/null &
  CHAN_PID=$!
  disown 2>/dev/null || true
  for _ in $(seq 1 30); do
    curl -fsS --max-time 1 "$CHAN/health" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  CHAN_PID=""
  return 1
}

# relay <call-log>
# The serving process, one poll cycle, against whatever the live path holds right now.
relay() {
  "$BIN" relay --registry "$LIVE" --journal "$JOURNAL" --call-log "$1" --channel "$CHAN" 2>&1
}

obs()      { curl -fsS --max-time 3 "$CHAN/observations" 2>/dev/null; }
# Chunks the channel confirmed into one room. The whole verdict rests on this line.
in_room()  { grep -c "^accepted chunk=[0-9]* channel=$2 " <<<"$1" | tr -d ' '; }
creates()  { awk -F'\t' '$1=="create"' "$1" 2>/dev/null | wc -l | tr -d ' '; }
# The prior registry is written compactly and the published one pretty-printed, so the space
# after the colon is optional rather than a fact about either file.
routes()   { grep -c "\"slug\": *\"$2\"" "$1" 2>/dev/null | tr -d ' '; }
# Patterns arrive on stdin so no resolved credential appears in grep's argv or in either file.
has_credential() {
  # An empty grep pattern matches every line. Refuse the publication explicitly if a future path
  # reaches this assertion without all three resolved credentials.
  [ -n "$STORE_TOKEN" ] && [ -n "$CUTOVER_MEMBER" ] && [ -n "$IRONWORKS_TOKEN" ] || return 0
  printf '%s\n' "$STORE_TOKEN" "$CUTOVER_MEMBER" "$IRONWORKS_TOKEN" |
    grep -F -f - "$LIVE" "$ROLLBACK" >/dev/null 2>&1
}
no_model() { grep -q "not a valid model name\|model provider rejected" <<<"$1"; }
# States in which the boundary was never set up, so the check measured nothing. The runtime's
# 30-requests-per-60-seconds limit and its 30s response timeout are upstream of IronWorks;
# scoring either as FAIL reports a cutover defect that does not exist. Identical to
# `dev/prove-delivery.sh`'s, which carries the reasoning — including why the 429 alternative is
# anchored to `HTTP 429`, a narrowing this script's sittings are what measured.
degraded() { grep -qE "timed out (writing|reading)|rate limited|rate.?limit|HTTP 429|service_unavailable|provider .* unavailable" <<<"$1"; }
# The failing run's own last words. Continuation lines are skipped: the runtime's refusals end
# with an indented `→ ...` instruction, so taking the last non-empty line would name the advice
# and never the cause — and a BLOCKED verdict is only worth reading if it says what stopped it.
why()      { grep -vE '^[[:space:]]*(→|$)' <<<"$1" | tail -n1 | cut -c1-88; }

# ── 1. publication ───────────────────────────────────────────────────────────────────────
# The real publisher, over a valid prior registry. What must come out of it: the candidate live,
# the prior durable and reachable by name, and neither credential anywhere in either file.
before="$(cat "$LIVE")"
out="$("$BIN" registry publish-one "$CANDIDATE" --store "$STORE" --registry "$LIVE" \
  --guidance "$CAND_GUIDANCE" --records-credential env:CUTOVER_RECORDS \
  --member-credential env:CUTOVER_MEMBER 2>&1)"
rc=$?
REGISTRY_ID="$(grep -oE "registry [0-9a-f]+" <<<"$out" | tail -n1 | cut -d' ' -f2)"
if [ $rc -eq 3 ]; then
  verdict "cutover.publication" BLOCKED "$(why "$out")"
  exit 3
elif [ $rc -eq 0 ] \
  && [ "$(routes "$LIVE" "$CANDIDATE")" = 1 ] && [ "$(routes "$LIVE" "$PRIOR")" = 0 ] \
  && grep -q -- "$CAND_ROOM" "$LIVE" \
  && [ -f "$ROLLBACK" ] && [ "$(cat "$ROLLBACK")" = "$before" ] \
  && ! has_credential; then
  verdict "cutover.publication" PASS "live is $CANDIDATE ($REGISTRY_ID); prior kept as .previous"
else
  verdict "cutover.publication" FAIL \
    "rc=$rc candidate=$(routes "$LIVE" "$CANDIDATE") prior=$(routes "$LIVE" "$PRIOR") rollback=$([ -f "$ROLLBACK" ] && echo yes || echo no)"
  exit 2
fi

# ── 2. the candidate serves ──────────────────────────────────────────────────────────────
# Two rooms, one batch, one live registry. The candidate's update must be answered and the
# prior's must not be — a relay still serving the old file would satisfy exactly the opposite.
L="$WORK/2.log"
if ! channel "$((UID_BASE+1))=$CAND_ROOM" "$((UID_BASE+2))=$PRIOR_ROOM"; then
  verdict "cutover.candidate_serves" BLOCKED "no channel on :$CHAN_PORT"
else
  out="$(relay "$L")"; rc=$?
  log="$(obs)"
  cand="$(in_room "$log" "$CAND_ROOM")"; prior="$(in_room "$log" "$PRIOR_ROOM")"
  if no_model "$out"; then
    verdict "cutover.candidate_serves" BLOCKED "runtime has no usable model key"
  elif degraded "$out"; then
    verdict "cutover.candidate_serves" BLOCKED "dependency unavailable: $(why "$out")"
  elif [ $rc -eq 0 ] && [ "$cand" -ge 1 ] && [ "$prior" = 0 ] && [ "$(creates "$L")" = 1 ]; then
    verdict "cutover.candidate_serves" PASS \
      "$cand chunk(s) into $CAND_ROOM, 0 into $PRIOR_ROOM, create=1"
  else
    verdict "cutover.candidate_serves" FAIL \
      "rc=$rc candidate_room=$cand prior_room=$prior create=$(creates "$L")"
  fi
fi

sleep 5

# ── 3. rollback ──────────────────────────────────────────────────────────────────────────
# The artifact is copied rather than consumed, so a second rollback after a second publication is
# still available. Asserting it survives is what distinguishes a restore from a move.
out="$("$BIN" registry rollback-one --registry "$LIVE" 2>&1)"; rc=$?
if [ $rc -eq 3 ]; then
  verdict "cutover.rollback" BLOCKED "$(why "$out")"
elif [ $rc -eq 0 ] \
  && grep -q "^ROLLED BACK" <<<"$out" \
  && [ "$(routes "$LIVE" "$PRIOR")" = 1 ] && [ "$(routes "$LIVE" "$CANDIDATE")" = 0 ] \
  && [ -f "$ROLLBACK" ] && [ "$(cat "$LIVE")" = "$before" ]; then
  verdict "cutover.rollback" PASS "live is $PRIOR again; the artifact was not consumed"
else
  verdict "cutover.rollback" FAIL \
    "rc=$rc prior=$(routes "$LIVE" "$PRIOR") candidate=$(routes "$LIVE" "$CANDIDATE") artifact=$([ -f "$ROLLBACK" ] && echo kept || echo gone)"
fi
# ── 4. the prior serves after a restart ──────────────────────────────────────────────────
# A new channel process and a new relay process, the same live path and the same journal. Fresh
# update ids, so the batch is genuinely new work rather than a settled one being reread.
L="$WORK/4.log"
if ! channel "$((UID_BASE+11))=$PRIOR_ROOM" "$((UID_BASE+12))=$CAND_ROOM"; then
  verdict "cutover.prior_serves_after_restart" BLOCKED "no channel on :$CHAN_PORT"
else
  out="$(relay "$L")"; rc=$?
  log="$(obs)"
  cand="$(in_room "$log" "$CAND_ROOM")"; prior="$(in_room "$log" "$PRIOR_ROOM")"
  if no_model "$out"; then
    verdict "cutover.prior_serves_after_restart" BLOCKED "runtime has no usable model key"
  elif degraded "$out"; then
    verdict "cutover.prior_serves_after_restart" BLOCKED "dependency unavailable: $(why "$out")"
  elif [ $rc -eq 0 ] && [ "$prior" -ge 1 ] && [ "$cand" = 0 ] && [ "$(creates "$L")" = 1 ]; then
    verdict "cutover.prior_serves_after_restart" PASS \
      "$prior chunk(s) into $PRIOR_ROOM, 0 into $CAND_ROOM, create=1"
  else
    verdict "cutover.prior_serves_after_restart" FAIL \
      "rc=$rc prior_room=$prior candidate_room=$cand create=$(creates "$L")"
  fi
fi

# ── verdict ──────────────────────────────────────────────────────────────────────────────
printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
printf 'the room each answer landed in comes from the channel, so serving the wrong registry\n'
printf 'cannot pass: every serving check asserts one room answered and the other did not.\n'
printf 'wait ~60s before another proof: the runtime allows 30 requests per 60 seconds.\n'
[ "$failed" -gt 0 ] && exit 2
[ "$blocked" -gt 0 ] && exit 3
exit 0
