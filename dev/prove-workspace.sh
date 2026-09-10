#!/usr/bin/env bash
# COST: real model turns against the probe. Needs a sandbox-profile probe, a model
# key, and a service declaring `runtime_tools: "sandbox"` — no shipped service does.
# Prove that IronWorks reaches IronClaw's persistent sandbox workspace.
#
# This deliberately uses three fresh IronWorks journals. There is no previous_response_id and no
# conversation replay between them, so the only product path joining the turns is the runtime's
# per-user /workspace. The first turn creates an unknown random value inside the sandbox; the next
# two independently read it. Matching 64-hex values are the positive persistence control.
#
# Preconditions, each checked before a turn is spent rather than left for the reader to infer from
# a failure:
#   - the proof-only binary supplies a synthetic definition declaring `runtime_tools: "sandbox"`.
#     It is absent from the production binary and the service catalogue.
#   - IronClaw is running with hosted-single-tenant-volume-sandboxed, so the member's catalogue
#     holds a shell capability. `dev/probe-up.sh` brings up a plain stack and does not.
#   - builtin.shell is allowed for this client; authority-changing and general network tools are not
#   - the normal IRONWORKS_BASE, IRONWORKS_TOKEN, IRONWORKS_RECORDS and
#     IRONWORKS_RECORDS_TOKEN variables are exported
#
# The preflights can only rule this proof OUT. Passing them certifies nothing about the sandbox —
# that is what the three turns below are for.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-workspace.XXXXXX")"
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() { rm -rf "$WORK"; }
on_exit cleanup
# The two defaults must agree: `Guidance::parse` refuses guidance bound to another slug, so a
# default slug the default guidance file does not name blocks every turn below before the sandbox
# is reached — a proof failing on its own fixture rather than on the property. `eval` is the slug
# `fixtures/eval.guidance.md` declares. Override both together or neither.
GUIDANCE="${IRONWORKS_WORKSPACE_GUIDANCE:-synthetic://guidance}"
SLUG="${IRONWORKS_WORKSPACE_SLUG:-synthetic-a}"
# The default is implemented only by the feature-gated qualification binary. An override can name
# a normal service, but the same sandbox-capability preflight still applies.
SERVICE="${IRONWORKS_WORKSPACE_SERVICE:-synthetic-sandbox-proof}"
ARTIFACT="/workspace/ironworks-persistence-proof"

# Fail-fast wrappers over `dev/lib.sh`'s `verdict`. Named apart from the counters it keeps:
# `blocked()` beside `$blocked` reads as one thing and is two.
# This script stops at the first failure — every check below depends on the one above it, so
# there is nothing left to measure. `dev/prove-continuation.sh` tallies instead and carries
# on, which is why that one is not spelled the same.
refuse_and_stop() { verdict "$1" FAIL "$2"; summary; }

need_env IRONWORKS_BASE IRONWORKS_TOKEN IRONWORKS_RECORDS IRONWORKS_RECORDS_TOKEN
if [ "$GUIDANCE" != "synthetic://guidance" ]; then
  [ -f "$GUIDANCE" ] || give_up "guidance" "$GUIDANCE does not exist"
fi
need_runtime
cargo build -q -p ironworks-cli --bin ironworks-deployment-proof \
  --features synthetic-qualification 2>/dev/null || give_up "build" "qualification build failed"
BIN=./target/debug/ironworks-deployment-proof

# ── preconditions, checked before any turn is spent ──────────────────────────────────────────
#
# Both of these were learned the expensive way: without them this script spent a turn, watched the
# model answer "I don't have access to builtin.shell", and reported `BLOCKED the sandboxed turn did
# not complete` — a message naming the wrong thing entirely. A BLOCKED that misnames what could not
# be measured is worse than a slow one, because the reader debugs the sandbox instead of the
# profile. Each check below can only ever rule the proof OUT; neither can certify the sandbox works,
# which is what the three turns are for.

# 1. The service must be able to carry a sandbox tool at all. `ServiceInstance::new` attenuates the
#    turn to the definition's ceiling, so `runtime_tools: none` removes the capability before the
#    runtime is ever asked — no catalogue state can restore it. Local, deterministic, free.
DEFINITION="services/$SERVICE.json"
if [ "$SERVICE" = "synthetic-sandbox-proof" ]; then
  RUNTIME_TOOLS=sandbox
else
  [ -f "$DEFINITION" ] || give_up "service.definition" "$DEFINITION does not exist"
  RUNTIME_TOOLS="$(python3 -c '
import json,sys
try: print(json.load(open(sys.argv[1]))["capabilities"]["runtime_tools"])
except Exception: pass' "$DEFINITION")"
fi
[ "$RUNTIME_TOOLS" = "sandbox" ] || give_up "service.runtime_tools" \
  "$SERVICE declares runtime_tools=${RUNTIME_TOOLS:-unreadable}; a sandbox turn needs \"sandbox\". \
No shipped service declares it, so this proof needs one that does."

# 2. The member's catalogue must actually hold a shell capability. Best-effort by design: a token
#    that cannot read `/settings/tools` establishes nothing either way, so an unreadable catalogue
#    proceeds rather than blocking. Only a catalogue that is readable AND holds no shell row is
#    grounds to stop.
CATALOGUE="$(curl -s --max-time 10 "$IRONWORKS_BASE/api/webchat/v2/settings/tools" \
  -H "authorization: Bearer $IRONWORKS_TOKEN" 2>/dev/null)"
SHELL_ROWS="$(printf '%s' "$CATALOGUE" | python3 -c '
import json,sys
try: d = json.load(sys.stdin)
except Exception: print("?"); raise SystemExit
ids = [e["key"][5:] for e in (d.get("entries") or []) if e["key"].startswith("tool.")]
if not ids: print("?"); raise SystemExit
print(sum(1 for i in ids if "shell" in i or "exec" in i) or 0)')"
[ "$SHELL_ROWS" = "?" ] || [ "$SHELL_ROWS" != "0" ] || give_up "runtime.sandbox_profile" \
  "the member catalogue holds no shell capability — IronClaw is not running \
hosted-single-tenant-volume-sandboxed. dev/probe-up.sh brings up a plain stack."

# How many cycles one logical turn gets. Each cycle costs no extra turn: a recovering invocation
# replays a recorded key rather than submitting a new one.
TURN_CYCLES="${IRONWORKS_WORKSPACE_CYCLES:-4}"
# The last refusal's own words, so a BLOCKED verdict names what stopped it instead of quoting an
# exit code. It has to be a file rather than a variable: `turn` is called in a command
# substitution, so anything it assigns dies with the subshell.
last_error() { why "$(cat "$1.stderr" 2>/dev/null)"; }

# One logical turn, driven across as many serving cycles as it needs.
#
# **A turn that outlives the runtime's synchronous wait is refused while it keeps running, and this
# proof used to score that as a failed measurement.** `DEFAULT_RESPONSES_WAIT_TIMEOUT` is 30s at
# `ironclaw@4cb47cfaf3/crates/product/ironclaw_openai_compat/src/responses_workflow.rs:55`, and the
# builder that would change it has no caller outside that crate's own tests — so it cannot be
# raised from outside the binary, and raising it inside would be a fork. `0036` measured the shipped
# service's own flagship question sitting on that boundary, so a turn doing more work than that one
# should be expected to cross it.
#
# **Do not read this as "a sandboxed turn cannot fit".** That is not measured and this comment used
# to claim it. Every `workspace.create` refusal seen on 2026-09-07 overran for a reason `0057`
# found underneath: the agent loop spent four model calls on tool names the runtime then rejected.
# `0032` timed an ordinary tool round trip at 12.63s and 23.04s, both inside the wait.
#
# So the refusal is not an error to retry; it is the boundary `0044` measured. The turn keeps
# running server-side, and a later cycle over the SAME journal recovers it as one logical turn
# under one idempotency key, because `TurnDriver::drive` consults durable evidence before it admits
# anything new. This re-invokes; it never resubmits. `--ask` is passed unchanged because
# `ReplayIdentical` proves the rebuilt body against the journalled digest before replaying.
#
# **This does not weaken what the proof isolates.** The three turns still get three separate
# journals; a cycle re-uses only the journal of the turn it is recovering, so the sole product path
# joining the three remains the runtime's per-user workspace.
turn() {
  # `rc` is seeded rather than declared bare: a caller that set the cycle count to zero would skip
  # the loop entirely and `return "$rc"` on an unset variable, which under `set -u` is an error in
  # the helper rather than a verdict about the turn.
  local journal="$1" ask="$2" out="" rc=1 errs
  for _ in $(seq 1 "$TURN_CYCLES"); do
    out="$("$BIN" turn --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" \
      --journal "$journal" --ask "$ask" 2>"$journal.stderr")"
    rc=$?
    [ "$rc" -eq 0 ] && { printf '%s' "$out"; return 0; }
    errs="$(cat "$journal.stderr" 2>/dev/null)"
    # Only a refusal that leaves the turn recoverable earns another cycle. Anything else — a
    # rejected model, a refused scope, an operator-required journal — is a result, and spending
    # more cycles on it would turn a legible failure into a slow one.
    unsettled "$errs" || break
  done
  printf '%s' "$out"
  return "$rc"
}
extract_hex() { grep -Eo '[0-9a-f]{64}' | head -n1; }

# A turn that never completed and a turn that completed with the wrong answer are different facts,
# and only the second is evidence about the sandbox. Splitting the exit code from the body is what
# lets the two carry different verdicts — BLOCKED for "no measurement", FAIL for "measured, wrong".
create="$(turn "$WORK/create.db" \
  "Use the tool named builtin__shell — two underscores, exactly as your tool definitions spell it. Run exactly this command inside the sandbox: test ! -e /var/run/docker.sock && umask 077 && python3 -c \"import secrets; open('$ARTIFACT','w').write(secrets.token_hex(32)+'\\n')\". Do not read the file. Reply only SAVED if the command succeeds.")"
create_rc=$?
[ "$create_rc" -eq 0 ] || give_up "workspace.create" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/create.db")"
[ "$create" = "SAVED" ] || refuse_and_stop "workspace.create" "expected exactly SAVED, got: $create"
verdict "workspace.create" PASS "worker wrote /workspace and had no Docker socket"

read_one="$(turn "$WORK/read-one.db" \
  "This is a fresh thread. Use the tool named builtin__shell — two underscores, as your tool definitions spell it — to read $ARTIFACT and reply with only its contents.")"
read_one_rc=$?
[ "$read_one_rc" -eq 0 ] || give_up "workspace.read.first" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/read-one.db")"
value_one="$(printf '%s' "$read_one" | extract_hex)"
[ -n "$value_one" ] || refuse_and_stop "workspace.read.first" "no 64-hex sandbox value was returned"
verdict "workspace.read.first" PASS "$value_one"

read_two="$(turn "$WORK/read-two.db" \
  "This is another fresh thread. Use the tool named builtin__shell — two underscores, as your tool definitions spell it — to read $ARTIFACT and reply with only its contents.")"
read_two_rc=$?
[ "$read_two_rc" -eq 0 ] || give_up "workspace.read.second" \
  "unrecovered after $TURN_CYCLES cycles: $(last_error "$WORK/read-two.db")"
value_two="$(printf '%s' "$read_two" | extract_hex)"
[ -n "$value_two" ] || refuse_and_stop "workspace.read.second" "no 64-hex sandbox value was returned"
[ "$value_one" = "$value_two" ] \
  || refuse_and_stop "workspace.persistence" "fresh threads returned different values"
verdict "workspace.persistence" PASS "three fresh Responses threads shared one client workspace"

summary "IronWorks exercised IronClaw's persistent sandbox workspace."
