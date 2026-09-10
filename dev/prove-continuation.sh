#!/usr/bin/env bash
# COST: real model turns against the probe. Needs a probe and a model key.
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
# Four more checks cover the other way a turn stops being answerable — not "it cannot be rebuilt"
# but "it ran, and its response holds nothing anyone may be shown":
#
#   6. settles.cancelled     a run cancelled OUT OF BAND settles for an operator. The response is
#                            terminal, so the only automatic move left — fetching it again —
#                            returns the same thing on every cycle, forever. `--crash-at
#                            after-accepted` leaves the id durable and the run still going, so the
#                            cancel has no window to miss and this does not race the provider.
#   7. cancelled.released    and that settle advanced the offset, same obligation as check 5.
#   8. accepted.recovers     THE POSITIVE CONTROL for 6 and 7: the same crash point with nothing
#                            cancelled must RECOVER and deliver. Without it, 6 and 7 would pass on
#                            a build that settles every recovered turn — the opposite defect, and
#                            indistinguishable from the fix (§K-8).
#   9. batch.contained       one update that cannot be served does not take the rest of its batch
#                            with it. Two tenants in one cycle: the first cannot verify, and the
#                            second must still be answered. `batch of 2 journalled` is the control
#                            for the ordering — without it, "the second was answered" could be a
#                            later cycle rather than the one that failed.
#
# Checks 6 and 8 each cost a model turn, and 9 costs one more. Cancelling needs the runtime's
# cancel route (`POST /api/v1/responses/{id}/cancel`), which this seam never calls itself.
#
# `Failed` and `Incomplete` have no arm, deliberately: nothing here can make the runtime accept a
# turn and then fail it, and a nondeterministic arm manufactures false BLOCKEDs and FAILs — which
# `docs/experiments/0044-a-refused-turn-recovers-on-the-next-serving-cycle.md` closes by naming as
# the expensive kind of wrong.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STORE_PORT="${PROBE_STORE_PORT:-3996}"
CHAN_PORT="${PROVE_CONTINUATION_PORT:-3993}"
GUIDANCE="fixtures/account-coordination/guidance.md"
SLUG="coordination-eval"
# The org IRONWORKS_RECORDS_TOKEN must resolve to — `dev-token-coordination` in the probe store
# (dev/probe-up.sh). The unverifiable tenant declares the same org on purpose: what makes it
# unverifiable is a credential the store rejects, not a mismatched expectation.
STORE_ORG="coordination-eval"
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
C=$(( UID_BASE + 2 ))   # cancelled out of band
D=$(( UID_BASE + 3 ))   # the control for C: same crash point, nothing cancelled
E=$(( UID_BASE + 4 ))   # the tenant that cannot verify
F=$(( UID_BASE + 5 ))   # the tenant behind it, which must still be answered

# The second tenant exists only to fail. Its records credential names a variable holding a token
# the store refuses, so it cannot verify and `boot_tenant` fails before any model is reached —
# deterministic, free, and the exact shape of a tenant whose store is briefly misconfigured.
BAD_SLUG="coordination-eval-unverifiable"
BAD_GROUP="g-chain-unverifiable"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-continuation.XXXXXX")"
CHAN_PID=""
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() {
  if [ -n "$CHAN_PID" ]; then
    disown "$CHAN_PID" 2>/dev/null || true
    kill "$CHAN_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
on_exit cleanup

# A blocked precondition ends the run through `give_up` rather than being tallied: every check
# below spends a real model turn, so continuing past a boundary that was never set up would bill
# for nothing.

need_env IRONWORKS_BASE IRONWORKS_TOKEN IRONWORKS_RECORDS IRONWORKS_RECORDS_TOKEN
need_runtime
need_store
cargo build -q -p ironworks-cli -p ironworks-devchannel 2>/dev/null \
  || give_up "build" "cargo build failed"
BIN=./target/debug/ironworks
CHAN=./target/debug/ironworks-devchannel
JOURNAL="$WORK/serve.journal"

cat > "$WORK/registry.json" <<JSON
{"tenants":[{
  "slug": "$SLUG",
  "channel": "$GROUP",
  "org": "$STORE_ORG",
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
  wait_for_http "http://127.0.0.1:$CHAN_PORT/poll?offset=0" && return 0
  give_up "channel.up" "the disposable channel did not start";
}

relay() {
  "$BIN" relay --registry "$WORK/registry.json" --journal "$JOURNAL" \
    --channel "http://127.0.0.1:$CHAN_PORT" "$@" 2>&1
}
# Sets `state` to the journal's outcome for one update, or to nothing when the journal records none.
# `status`'s own exit is kept apart from the grep that reads its report: piped together, a `status`
# that could not read the journal printed nothing, the grep found nothing, and three checks scored
# FAIL — "journal says nothing" — about a journal that was never read. That is BLOCKED, and it ends
# the run for the reason `give_up` states above: every check that reads the journal after it would
# block the same way, and most of them spend a turn first.
outcome_of() { # outcome_of <update id> <check>
  local report
  if ! report="$("$BIN" status --journal "$JOURNAL" 2>"$WORK/status.err")"; then
    give_up "$2" "ironworks status could not read the journal: $(why "$(cat "$WORK/status.err")")"
  fi
  state="$(grep -o "key=upd-$1 .*outcome=[a-z_]*" <<<"$report" | grep -o 'outcome=[a-z_]*' | cut -d= -f2)"
}

# ── 1. the positive control: a first turn answers and leaves a thread pointer ──────────────
start_channel --update "$A=$GROUP:$ASK_A" --chat-kind "$A=private"
one="$(relay)"
if grep -q "update $A delivery=delivered" <<<"$one"; then
  verdict "chain.established" PASS "update $A answered and delivered"
else
  printf '%s\n' "$one" | tail -5
  give_up "chain.established" "the first turn did not deliver; nothing below would mean anything"
fi

# ── 2. the second turn really chains ───────────────────────────────────────────────────────
# Asserted separately from recovery. If continuation regressed to always-Fresh, check 4 would
# pass while proving nothing, because a fresh turn's rebuild matches trivially.
start_channel --update "$A=$GROUP:$ASK_A" --update "$B=$GROUP:$ASK_B" --chat-kind "$A=private" --chat-kind "$B=private"
two="$(relay --crash-at before-response)"
code=$?
if grep -q "thread: continuing" <<<"$two" && grep -q "carried by the thread" <<<"$two"; then
  verdict "chain.is_real" PASS "chained, and the book was not re-sent"
else
  printf '%s\n' "$two" | tail -8
  verdict "chain.is_real" FAIL "the second turn did not chain — every check below is vacuous"
fi

# ── 3. it died after dispatch, with no response id ─────────────────────────────────────────
outcome_of "$B" chain.crashed
if [ "$state" = "submitted_unknown" ]; then
  verdict "chain.crashed" PASS "upd-$B is submitted_unknown (exit $code)"
else
  verdict "chain.crashed" FAIL "expected submitted_unknown, journal says ${state:-nothing}"
fi

# ── 4. the restart rebuilds it byte-identically ────────────────────────────────────────────
three="$(relay)"
if grep -q "rebuild is byte-identical" <<<"$three"; then
  if grep -q "update $B delivery=delivered" <<<"$three"; then
    verdict "chain.recovers" PASS "rebuilt byte-identically, replayed, delivered"
  else
    verdict "chain.recovers" PASS "rebuilt byte-identically (delivery not reached)"
  fi
elif grep -q "cannot be reconstructed byte-identically" <<<"$three"; then
  verdict "chain.recovers" FAIL "a chained turn cannot be rebuilt — the decision it made was not journalled"
else
  printf '%s\n' "$three" | tail -8
  verdict "chain.recovers" FAIL "neither a replay nor a refusal; see above"
fi

# ── 5. and either way the channel is not held ──────────────────────────────────────────────
# Independent of check 4 on purpose. A turn that cannot be resolved must SETTLE, because the safe
# cursor is the lowest open update across every tenant.
four="$(relay)"
offset="$(grep -o 'poll offset=[0-9]*' <<<"$four" | tail -1 | cut -d= -f2)"
if [ -n "$offset" ] && [ "$offset" -gt "$B" ]; then
  verdict "cursor.released" PASS "offset advanced to $offset, past upd-$B"
else
  verdict "cursor.released" FAIL "offset stuck at ${offset:-unknown}; one turn is deafening the channel"
fi

# ── 6. a terminal response holding no answer settles, rather than being re-fetched forever ──
# The only one of `Failed`/`Cancelled`/`Incomplete` a script can induce deterministically. The
# others need the runtime to accept a turn and then fail it, which nothing here can force, and a
# nondeterministic arm manufactures false FAILs — `0044`'s own closing section names that the
# expensive kind of wrong.
#
# Forced with `--crash-at`, not raced: `after-accepted` leaves B4's exact precondition on disk —
# accepted, response id durable, not yet polled — so the run is still going and the cancel below
# has no window to miss. The next cycle then takes `Recovery::Fetch`, reads `cancelled`, and must
# settle rather than re-read it every cycle forever.
start_channel --update "$C=$GROUP:$ASK_A" --chat-kind "$C=private"
six="$(relay --crash-at after-accepted)"
# Anchored on the crash point's own wording, not on the id's alphabet: `ResponseId::parse` accepts
# any non-blank string, so a pattern that assumed lowercase hex would BLOCK on an id shape we do
# not own — the false-BLOCKED failure `0044` closes by naming.
RESP="$(grep -o 'response [^ ]* is durable (accepted, not polled)' <<<"$six" | head -1 | cut -d' ' -f2)"
if [ -z "$RESP" ]; then
  printf '%s\n' "$six" | tail -8
  give_up "settles.cancelled" "no response id was journalled; there is nothing to cancel"
fi
if ! curl -fsS --max-time 10 -X POST \
     -H "Authorization: Bearer $IRONWORKS_TOKEN" \
     "$IRONWORKS_BASE/api/v1/responses/$RESP/cancel" >"$WORK/cancel.json" 2>&1; then
  printf '%s\n' "$(tail -3 "$WORK/cancel.json")"
  give_up "settles.cancelled" "the runtime refused to cancel $RESP"
fi
seven="$(relay)"
outcome_of "$C" settles.cancelled
if [ "$state" = "operator_required" ]; then
  verdict "settles.cancelled" PASS "upd-$C settled; $RESP was cancelled out of band"
elif grep -q "still running" <<<"$seven"; then
  # The cancel did not take before the fetch — an environment race, not a defect in the code.
  give_up "settles.cancelled" "$RESP was still running when it was re-fetched"
else
  printf '%s\n' "$seven" | tail -8
  verdict "settles.cancelled" FAIL "expected operator_required, journal says ${state:-nothing}"
fi

# ── 7. and that settle released the channel ─────────────────────────────────────────────────
eight="$(relay)"
offset="$(grep -o 'poll offset=[0-9]*' <<<"$eight" | tail -1 | cut -d= -f2)"
if [ -n "$offset" ] && [ "$offset" -gt "$C" ]; then
  verdict "cancelled.released" PASS "offset advanced to $offset, past upd-$C"
else
  verdict "cancelled.released" FAIL "offset stuck at ${offset:-unknown}; a cancelled turn holds the channel"
fi

# ── 8. the positive control for 6 and 7 ─────────────────────────────────────────────────────
# The SAME crash point, with nothing cancelled. It must recover and deliver — otherwise checks 6
# and 7 would pass on a build that settles every recovered turn, which is the opposite defect and
# indistinguishable from the fix without this arm (§K-8).
start_channel --update "$D=$GROUP:$ASK_A" --chat-kind "$D=private"
relay --crash-at after-accepted >/dev/null 2>&1
nine="$(relay)"
# The journal is read only when the relay's own report has not already settled the check, so a
# delivered turn passes whether or not `status` can read the journal afterwards.
if grep -q "update $D delivery=delivered" <<<"$nine"; then
  verdict "accepted.recovers" PASS "upd-$D fetched its known response and delivered"
else
  outcome_of "$D" accepted.recovers
  if [ "$state" = "operator_required" ]; then
    verdict "accepted.recovers" FAIL "a recoverable turn was SETTLED — check 6 proves nothing"
  else
    printf '%s\n' "$nine" | tail -8
    verdict "accepted.recovers" FAIL "neither delivered nor settled; journal says ${state:-nothing}"
  fi
fi

# ── 9. one bad update does not take its batch with it ───────────────────────────────────────
# Two tenants, two updates, one cycle. The first tenant's records credential is refused, so it
# cannot verify and `boot_tenant` fails — deterministically, with no network flakiness. The second
# tenant must still be answered IN THE SAME CYCLE.
#
# The batch line is the control for the ordering: without it, "E was delivered" could be a later
# cycle rather than the one that failed.
sed "s/slug: $SLUG /slug: $BAD_SLUG /" "$GUIDANCE" > "$WORK/bad.guidance.md"
cat > "$WORK/two.registry.json" <<JSON
{"tenants":[{
  "slug": "$BAD_SLUG",
  "channel": "$BAD_GROUP",
  "org": "$STORE_ORG",
  "service": "relationship-intelligence",
  "guidance": "$WORK/bad.guidance.md",
  "records_credential": "env:PROVE_CONTINUATION_BAD_TOKEN",
  "runtime_credential": "env:IRONWORKS_TOKEN"
},{
  "slug": "$SLUG",
  "channel": "$GROUP",
  "org": "$STORE_ORG",
  "service": "relationship-intelligence",
  "guidance": "$GUIDANCE",
  "records_credential": "env:IRONWORKS_RECORDS_TOKEN",
  "runtime_credential": "env:IRONWORKS_TOKEN"
}]}
JSON
export PROVE_CONTINUATION_BAD_TOKEN="not-a-real-record-store-token"
start_channel --update "$E=$BAD_GROUP:$ASK_A" --update "$F=$GROUP:$ASK_B" \
  --chat-kind "$E=private" --chat-kind "$F=private"
ten="$("$BIN" relay --registry "$WORK/two.registry.json" --journal "$WORK/two.journal" \
  --channel "http://127.0.0.1:$CHAN_PORT" 2>&1)"
if ! grep -q "batch of 2 journalled" <<<"$ten"; then
  printf '%s\n' "$ten" | tail -8
  give_up "batch.contained" "the two updates were not in one batch; ordering proves nothing"
fi
if grep -q "update $E did not settle this cycle" <<<"$ten" \
   && grep -q "update $F delivery=delivered" <<<"$ten"; then
  verdict "batch.contained" PASS "upd-$E failed and upd-$F was still answered in the same cycle"
elif grep -q "update $F delivery=delivered" <<<"$ten"; then
  printf '%s\n' "$ten" | tail -8
  verdict "batch.contained" FAIL "upd-$E did not fail; the containment is untested"
else
  printf '%s\n' "$ten" | tail -8
  verdict "batch.contained" FAIL "one tenant's failure took the whole batch with it"
fi

summary
