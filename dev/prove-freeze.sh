#!/usr/bin/env bash
# COST: six real model turns against a sandbox-profile probe, and a model key. Mints two
# members and **burns neither** — §L-9 spends a member when approval-off is written, and this
# writes no settings at all.
# Prove the freeze: one role writes a candidate, another cannot see it, and only a copy the seam
# takes at a recorded digest puts it in front of the second.
#
# ── what this is for ────────────────────────────────────────────────────────────────────────
#
# arXiv 2609.01481 §3.4.2-3.4.3 asks for three properties around an evolving artifact, and states
# them as one mechanism: the QA role receives a **frozen, read-only candidate**, so that "the
# implementation cannot change while its evidence is being collected" and "an assessment cannot
# combine behavior from different artifact versions".
#
#   single-writer boundary   only the Developer's member can write A_t
#   frozen candidate         QA observes a copy taken at one instant, not a live directory
#   one candidate identity   every observation is bound to one digest
#
# **Here that is closed by construction rather than by policy, which is the point.** IronClaw gives
# each member its own `sandbox-workspaces/users/<id>/`, and a worker's binds are built from its own
# workspace alone
# (`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process.rs:757`), so another
# member's bytes are not on that container's filesystem. **That last step is a source reading**:
# what this script measures is one path in each direction, not every path. `docs/INVARIANTS.md` §L-7 records that ordinary confinement is custody-backed and a
# member can undo it; nothing a member writes to its own settings moves another member's
# workspace, so this boundary is not custody-backed and does not inherit that limit.
#
# ── what it does NOT prove ──────────────────────────────────────────────────────────────────
#
# Neither member is confined. This measures workspace isolation and candidate identity, not tool
# authority — a confined role is Phase 5's other half and needs `lifecycle confine`, which spends
# the member (§L-9). Nothing here claims a role cannot reach a tool; it claims a role cannot reach
# another role's bytes.
#
# The copy is performed by this script standing in for the seam, through the probe's Docker
# volume. On a served host the same directory is at `/var/lib/ironclaw/<client>/workspace` owned by
# the service account (`deploy/README.md`), so the operation is a directory copy there too — but
# that has not been run on a host and this proves nothing about one.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-freeze.XXXXXX")"
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() { rm -rf "$WORK"; }
on_exit cleanup

SLUG="${IRONWORKS_FREEZE_SLUG:-synthetic-a}"
GUIDANCE="${IRONWORKS_FREEZE_GUIDANCE:-synthetic://guidance}"
SERVICE="${IRONWORKS_FREEZE_SERVICE:-synthetic-sandbox-proof}"
CANDIDATE="/workspace/candidate"
MARKER="/workspace/qa-was-here"
PROJECT="${IRONWORKS_FREEZE_PROJECT:-multiprobe}"
TURN_CYCLES="${IRONWORKS_FREEZE_CYCLES:-4}"
# The runtime's data volume, where per-user sandbox workspaces actually live. Measured in
# `docs/experiments/0059-the-sandbox-works-once-the-model-is-told-the-tools-other-name.md`: they
# are under the runtime's DATA directory, not under `IRONCLAW_REBORN_WORKSPACE_ROOT`.
VOLUME="${IRONWORKS_FREEZE_VOLUME:-${PROJECT}_ic-data}"
USERS_DIR=/d/ironclaw-reborn/hosted-single-tenant-volume-sandboxed/sandbox-workspaces/users

refuse_and_stop() { verdict "$1" FAIL "$2"; summary; }
last_error() { why "$(cat "$1.stderr" 2>/dev/null)"; }

need_env IRONWORKS_BASE IRONWORKS_TOKEN IRONWORKS_RECORDS IRONWORKS_RECORDS_TOKEN
need_runtime
command -v docker >/dev/null || give_up "docker" "the copy reaches the probe's volume through docker"
cargo build -q -p ironworks-cli --bin ironworks-deployment-proof \
  --features synthetic-qualification 2>/dev/null || give_up "build" "qualification build failed"
BIN=./target/debug/ironworks-deployment-proof

# ── two members, because one workspace cannot be two roles ──────────────────────────────────
# The operator bearer is deliberately not used as either role: it is the credential that mints
# them, and a proof whose two "roles" are one caller measures nothing about isolation.
mint() {
  local who="$1" minted token
  minted="$(curl -s --max-time 20 -X POST "$IRONWORKS_BASE/api/webchat/v2/admin/users" \
    -H "authorization: Bearer $IRONWORKS_TOKEN" -H 'content-type: application/json' \
    -d "{\"email\":\"freeze-$who-$$@example.com\",\"display_name\":\"freeze $who $$\",\"role\":\"member\"}")"
  # `api_token` is returned once and never again (§I / `0043`), so a mint whose token we could not
  # read is a member we can never act as. Say so rather than falling back to the operator bearer,
  # which would quietly measure one caller twice.
  token="$(printf '%s' "$minted" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("api_token") or "")
except Exception: pass')"
  [ -n "$token" ] || give_up "member.$who" "could not mint: $(printf '%s' "$minted" | head -c 160)"
  printf '%s' "$token"
}
DEV_TOKEN="$(mint developer)"
QA_TOKEN="$(mint qa)"

# One logical turn as one member, across as many serving cycles as it needs. Same mechanism and
# same reasoning as `dev/prove-workspace.sh` — `0044`'s recovery, never a resubmit.
turn_as() {
  local token="$1" journal="$2" ask="$3" out rc errs
  rc=1
  for _ in $(seq 1 "$TURN_CYCLES"); do
    out="$(IRONWORKS_TOKEN="$token" "$BIN" turn --slug "$SLUG" --guidance "$GUIDANCE" \
      --service "$SERVICE" --journal "$journal" --ask "$ask" 2>"$journal.stderr")"
    rc=$?
    [ "$rc" -eq 0 ] && { printf '%s' "$out"; return 0; }
    errs="$(cat "$journal.stderr" 2>/dev/null)"
    unsettled "$errs" || break
  done
  printf '%s' "$out"
  return "$rc"
}
# Every ask names the tool's wire spelling. The runtime refuses its own model's output otherwise
# (`docs/UPSTREAM.md` D10, withdrawn as a defect but measured as behaviour), so this is a
# workaround for an observed incompatibility, not a preference and not a claim about whose it is.
SHELL_TOOL='Use the tool named builtin__shell — two underscores, exactly as your tool definitions spell it.'
extract_hex() { grep -Eo '[0-9a-f]{64}' | head -n1; }
in_volume() { docker run --rm -v "$VOLUME:/d" alpine:3 sh -c "$1" 2>/dev/null; }

# ── 1. the Developer writes a candidate only it can have written ────────────────────────────
dev_out="$(turn_as "$DEV_TOKEN" "$WORK/dev-write.db" \
  "$SHELL_TOOL Run exactly this command inside the sandbox: umask 077 && python3 -c \"import secrets; open('$CANDIDATE','w').write(secrets.token_hex(32)+'\\n')\". Do not read the file. Reply only SAVED if the command succeeds.")"
dev_rc=$?
[ "$dev_rc" -eq 0 ] || give_up "freeze.developer_writes" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/dev-write.db")"
[ "$dev_out" = "SAVED" ] || refuse_and_stop "freeze.developer_writes" "expected SAVED, got: $dev_out"
verdict "freeze.developer_writes" PASS "the Developer wrote a candidate into its own workspace"

# ── 2. the QA role cannot see it — the positive control ─────────────────────────────────────
# Without this the proof is worthless: if both roles shared one workspace, every later read would
# succeed and be reported as the copy working. This is the check that makes the copy mean
# something, so it runs BEFORE any copy exists (§K-8).
qa_before="$(turn_as "$QA_TOKEN" "$WORK/qa-before.db" \
  "$SHELL_TOOL Run exactly this command inside the sandbox: cat $CANDIDATE 2>/dev/null || echo NOTHING-THERE. Reply with only the command's output.")"
qa_before_rc=$?
[ "$qa_before_rc" -eq 0 ] || give_up "freeze.isolation" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/qa-before.db")"
# Three-valued on purpose, because `|| echo NOTHING-THERE` is a fallback and a fallback cannot
# tell "the file was absent" from "the command never ran". Absence of the value is not evidence;
# the sentinel is. A turn that completed without running anything measured nothing and blocks.
if [ -n "$(printf '%s' "$qa_before" | extract_hex)" ]; then
  refuse_and_stop "freeze.isolation" \
    "the QA member read the Developer's candidate before any copy — the two share one workspace"
elif ! grep -q "NOTHING-THERE" <<<"$qa_before"; then
  give_up "freeze.isolation" \
    "the QA turn returned neither the candidate nor the sentinel, so nothing was measured: $qa_before"
fi
verdict "freeze.isolation" PASS "the QA member's workspace does not contain the Developer's candidate"

# ── 3. the seam freezes the candidate ───────────────────────────────────────────────────────
# Identify the two workspaces by content and by exclusion. Both members have now run a turn, so
# both directories exist; anything other than exactly two is a stack this script cannot reason
# about, and guessing which is which would be the defect the whole proof exists to avoid.
count="$(in_volume "ls -1 $USERS_DIR 2>/dev/null | wc -l" | tr -d '[:space:]')"
[ "$count" = "2" ] || give_up "freeze.copy" \
  "expected exactly two sandbox workspaces, found ${count:-none}; cannot identify the roles"
DEV_WS="$(in_volume "grep -rl . $USERS_DIR/*/candidate 2>/dev/null | head -1 | xargs -r dirname")"
[ -n "$DEV_WS" ] || give_up "freeze.copy" "no workspace holds the candidate"
QA_WS="$(in_volume "for d in $USERS_DIR/*; do [ \"\$d\" != '$DEV_WS' ] && echo \"\$d\"; done | head -1")"
[ -n "$QA_WS" ] && [ "$QA_WS" != "$DEV_WS" ] || give_up "freeze.copy" "could not identify the QA workspace"
FROZEN="$(in_volume "sha256sum $DEV_WS/candidate | cut -d' ' -f1")"
[ -n "$FROZEN" ] || give_up "freeze.copy" "could not digest the candidate"
in_volume "cp -a $DEV_WS/candidate $QA_WS/candidate" >/dev/null
copied="$(in_volume "sha256sum $QA_WS/candidate | cut -d' ' -f1")"
[ "$copied" = "$FROZEN" ] || refuse_and_stop "freeze.copy" \
  "the copy digests differently: $FROZEN vs ${copied:-nothing}"
verdict "freeze.copy" PASS "candidate frozen at ${FROZEN:0:16}"

# ── 4. QA observes that candidate, and the digest says which one ────────────────────────────
qa_after="$(turn_as "$QA_TOKEN" "$WORK/qa-after.db" \
  "$SHELL_TOOL This is a fresh thread. Run exactly this command inside the sandbox: cat $CANDIDATE. Reply with only its contents.")"
qa_after_rc=$?
[ "$qa_after_rc" -eq 0 ] || give_up "freeze.candidate_identity" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/qa-after.db")"
observed="$(printf '%s' "$qa_after" | extract_hex)"
[ -n "$observed" ] || refuse_and_stop "freeze.candidate_identity" "QA returned no 64-hex value"
observed_digest="$(printf '%s\n' "$observed" | shasum -a 256 | cut -d' ' -f1)"
[ "$observed_digest" = "$FROZEN" ] || refuse_and_stop "freeze.candidate_identity" \
  "QA observed a value whose digest is not the frozen one: $observed_digest vs $FROZEN"
verdict "freeze.candidate_identity" PASS "QA read the frozen candidate, digest matches"

# ── 5. the copy is a copy: QA cannot reach back ─────────────────────────────────────────────
qa_write="$(turn_as "$QA_TOKEN" "$WORK/qa-write.db" \
  "$SHELL_TOOL Run exactly this command inside the sandbox: echo qa >$MARKER. Reply only DONE if the command succeeds.")"
qa_write_rc=$?
[ "$qa_write_rc" -eq 0 ] || give_up "freeze.single_writer" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/qa-write.db")"
[ "$qa_write" = "DONE" ] || refuse_and_stop "freeze.single_writer" "expected DONE, got: $qa_write"
dev_check="$(turn_as "$DEV_TOKEN" "$WORK/dev-check.db" \
  "$SHELL_TOOL This is a fresh thread. Run exactly this command inside the sandbox: test -e $MARKER && echo MARKER-PRESENT || echo MARKER-ABSENT. Reply with only the command's output.")"
dev_check_rc=$?
[ "$dev_check_rc" -eq 0 ] || give_up "freeze.single_writer" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/dev-check.db")"
case "$dev_check" in
  *MARKER-ABSENT*) verdict "freeze.single_writer" PASS \
    "a QA write did not reach the Developer's workspace" ;;
  *MARKER-PRESENT*) refuse_and_stop "freeze.single_writer" \
    "the QA member's write appeared in the Developer's workspace — the copy shares storage" ;;
  # Neither sentinel: the turn completed and told us nothing, which is BLOCKED rather than a
  # product failure — the same distinction §F-14 draws and `0050` found being collapsed.
  *) give_up "freeze.single_writer" \
       "the Developer reported neither sentinel, so nothing was measured: $dev_check" ;;
esac

summary "The seam froze a candidate: one writer, one copy, one digest, and no path back."
