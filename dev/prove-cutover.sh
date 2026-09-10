#!/usr/bin/env bash
# COST: one runtime member, spent irreversibly, plus two model turns. Needs a probe
# and a model key.
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
# Both serving checks go through the same helper, so the two halves of the control cannot drift.
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
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STORE_PORT="${PROBE_STORE_PORT:-3996}"
CHAN_PORT="${PROVE_CUTOVER_PORT:-3992}"
STORE_TOKEN="dev-token-eval"
# The org that token resolves to in the probe store (dev/probe-up.sh). Only the hand-written prior
# registry needs it stated; the candidate goes through `registry publish-one`, which records the
# org it measured from the store itself.
STORE_ORG="eval"
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
on_exit cleanup

# ── preconditions ────────────────────────────────────────────────────────────────────────
need_env IRONWORKS_BASE IRONWORKS_TOKEN
need_runtime
need_store
if curl -fsS --max-time 2 "http://127.0.0.1:$CHAN_PORT/health" >/dev/null 2>&1; then
  give_up "channel.port" \
    "something already answers on $CHAN_PORT; set PROVE_CUTOVER_PORT to a free port"
fi
cargo build -q -p ironworks-cli -p ironworks-devchannel 2>/dev/null || {
  give_up "build" "cargo build failed"; }
BIN=./target/debug/ironworks
CHAN_BIN=./target/debug/ironworks-devchannel
STORE_URL="http://127.0.0.1:$STORE_PORT"
CHAN="http://127.0.0.1:$CHAN_PORT"

# ── the two tenants ──────────────────────────────────────────────────────────────────────
# Same service and the same book on purpose. If they differed in what they answer, a check could
# pass on the content and never test the route; making the ROOM the only difference is what makes
# the observation log the whole of the evidence.
mkdir -p "$LIVE_DIR" || { give_up "workspace" "cannot create $LIVE_DIR"; }
CAND_GUIDANCE="$WORK/$CANDIDATE.guidance.md"
PRIOR_GUIDANCE="$WORK/$PRIOR.guidance.md"
sed "1s|slug: eval|slug: $CANDIDATE|" fixtures/eval.guidance.md > "$CAND_GUIDANCE"
sed "1s|slug: eval|slug: $PRIOR|" fixtures/eval.guidance.md > "$PRIOR_GUIDANCE"
if ! grep -q "slug: $CANDIDATE" "$CAND_GUIDANCE" \
  || ! grep -q "slug: $PRIOR" "$PRIOR_GUIDANCE"; then
  give_up "guidance" "fixtures/eval.guidance.md no longer carries the expected marker"
fi

# Credentials reach every process as env:NAME handles, never as argv and never in a file. The
# registry records the handle; the secret is only ever in this shell's environment.
export CUTOVER_RECORDS="$STORE_TOKEN"
export CUTOVER_PRIOR_RECORDS="$STORE_TOKEN"
export CUTOVER_PRIOR_RUNTIME="$IRONWORKS_TOKEN"
export IRONWORKS_RECORDS="$STORE_URL"
export IRONCLAW_API="$IRONWORKS_BASE"

cat > "$LIVE" <<JSON
{"tenants":[{"slug":"$PRIOR","channel":"$PRIOR_ROOM","org":"$STORE_ORG","service":"$SERVICE",
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

# `publish-one` repeats the requested-model policy comparison and refuses unless the runtime's
# operator-visible policy is the singleton MODEL_PIN, so a fresh probe cannot be published to: it
# comes up with `selection_enabled=false` and publication refuses, correctly, on that.
#
# **Before the mint, deliberately.** This used to be absent altogether, and the run reached
# publication having already spent a member — then scored the refusal FAIL, which reads as "the
# cutover is broken" when what happened is that the probe was never configured for one. A
# precondition that cannot be met is BLOCKED and it must cost nothing (§F-14).
if ! policy="$("$BIN" model-policy apply 2>&1)"; then
  give_up "probe.model_policy" "the probe's requested-model policy could not be set: $(why "$policy")"
fi
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
  give_up "candidate.member" "could not mint a member: $(printf '%s' "$minted_json" | head -c 120)"
fi
minted=1
export CUTOVER_MEMBER IRONCLAW_MEMBER_TOKEN="$CUTOVER_MEMBER"

if ! adopt="$("$BIN" lifecycle adopt "$CANDIDATE" --store "$STORE" --service "$SERVICE" \
  --channel "$CAND_ROOM" --records-credential env:CUTOVER_RECORDS \
  --member-credential env:CUTOVER_MEMBER 2>&1)"; then
  give_up "candidate.adopted" "$(why "$adopt")"
fi
if ! confine="$("$BIN" lifecycle confine "$CANDIDATE" --store "$STORE" 2>&1)"; then
  give_up "candidate.confined" "$(why "$confine")"
fi
printf 'prove-cutover: candidate adopted and confined; prior registry is live at %s\n' \
  "$(basename "$LIVE")"

# Confinement writes one settings row per catalogued tool — comfortably more than the runtime's
# 30 requests per 60 seconds — and check 2's turn is the very next thing this script asks of it.
# Measured on 2026-09-04: without this wait check 2 reported `service_unavailable` while the same
# member served a turn cleanly a minute later, which is a rate limit reported as a cutover defect.
# The window is the runtime's, so waiting it out is the only way to measure what is downstream.
printf 'prove-cutover: waiting 60s for the runtime request window to clear after confinement\n\n'
pace_window

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
  CHAN_PID="$(start_fixture "$CHAN/health" "$CHAN_BIN" --port "$CHAN_PORT" "${args[@]}")"
  [ -n "$CHAN_PID" ] && return 0
  return 1
}

# relay <call-log>
# The serving process, one poll cycle, against whatever the live path holds right now.
relay() {
  "$BIN" relay --registry "$LIVE" --journal "$JOURNAL" --call-log "$1" --channel "$CHAN" 2>&1
}

# Chunks the channel confirmed into one room. The whole verdict rests on this line.
in_room()  { grep -c "^accepted chunk=[0-9]* channel=$2 " <<<"$1" | tr -d ' '; }
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
# serve <name> <room that must answer> <room that must stay silent> <call-log> <id=room> <id=room>
#
# Both serving checks go through here rather than through two copies, so the assertion cannot
# drift apart between them — which matters more than the duplication it removes, because the two
# checks are each other's control and a difference between them would be invisible.
serve() {
  local name="$1" answers="$2" silent="$3" L="$4"; shift 4
  if ! channel "$@"; then
    verdict "$name" BLOCKED "no channel on :$CHAN_PORT"
    return
  fi
  local out rc
  out="$(relay "$L")"; rc=$?
  if no_model "$out"; then
    verdict "$name" BLOCKED "runtime has no usable model key"
    return
  fi
  if unsettled "$out"; then
    verdict "$name" BLOCKED "$(unsettled_why "$out")"
    return
  fi
  local log good bad
  log="$(obs)"
  good="$(in_room "$log" "$answers")"
  bad="$(in_room "$log" "$silent")"
  if [ "$rc" -eq 0 ] && [ "$good" -ge 1 ] && [ "$bad" = 0 ] && [ "$(creates "$L")" = 1 ]; then
    verdict "$name" PASS \
      "$good chunk(s) into $answers, 0 into $silent, create=1"
  else
    verdict "$name" FAIL \
      "rc=$rc answered=$good silent=$bad create=$(creates "$L")"
  fi
}

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
  give_up "cutover.publication" "$(why "$out")"
elif [ $rc -eq 0 ] \
  && [ "$(routes "$LIVE" "$CANDIDATE")" = 1 ] && [ "$(routes "$LIVE" "$PRIOR")" = 0 ] \
  && grep -q -- "$CAND_ROOM" "$LIVE" \
  && [ -f "$ROLLBACK" ] && [ "$(cat "$ROLLBACK")" = "$before" ] \
  && ! has_credential; then
  verdict "cutover.publication" PASS "live is $CANDIDATE ($REGISTRY_ID); prior kept as .previous"
else
  # The publisher's own last words, as every BLOCKED arm in this file already does. The counts
  # say the cutover did not happen; only the sentence says why, and without it a FAIL here costs
  # a second member to diagnose — this one did.
  verdict "cutover.publication" FAIL \
    "rc=$rc candidate=$(routes "$LIVE" "$CANDIDATE") prior=$(routes "$LIVE" "$PRIOR") rollback=$([ -f "$ROLLBACK" ] && echo yes || echo no) | $(why "$out")"
  # `summary`, not `exit 2`. Every serving check below reads the registry this one was supposed to
  # publish, so there is nothing left to measure — but a bare `exit 2` would report "measured, and
  # it is wrong" even when an earlier check had been BLOCKED and nothing was measured at all. That
  # inversion is what `summary` exists to prevent (`docs/experiments/0050-blocked-was-being-read-as-fail.md`).
  summary 'the serving checks below read the registry this one publishes, so the run stops here.'
fi

# ── 2. the candidate serves ──────────────────────────────────────────────────────────────
# Two rooms, one batch, one live registry. The candidate's update must be answered and the
# prior's must not be — a relay still serving the old file would satisfy exactly the opposite.
serve "cutover.candidate_serves" "$CAND_ROOM" "$PRIOR_ROOM" "$WORK/2.log" \
  "$((UID_BASE+1))=$CAND_ROOM" "$((UID_BASE+2))=$PRIOR_ROOM"

pace

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
serve "cutover.prior_serves_after_restart" "$PRIOR_ROOM" "$CAND_ROOM" "$WORK/4.log" \
  "$((UID_BASE+11))=$PRIOR_ROOM" "$((UID_BASE+12))=$CAND_ROOM"

# ── verdict ──────────────────────────────────────────────────────────────────────────────
summary \
  'the room each answer landed in comes from the channel, so serving the wrong registry' \
  'cannot pass: every serving check asserts one room answered and the other did not.' \
  'wait ~60s before another proof: the runtime allows 30 requests per 60 seconds.'
