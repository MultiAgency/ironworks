#!/usr/bin/env bash
# Prove that IronWorks reaches IronClaw's persistent sandbox workspace.
#
# This deliberately uses three fresh IronWorks journals. There is no previous_response_id and no
# conversation replay between them, so the only product path joining the turns is the runtime's
# per-user /workspace. The first turn creates an unknown random value inside the sandbox; the next
# two independently read it. Matching 64-hex values are the positive persistence control.
#
# Preconditions, each checked before a turn is spent rather than left for the reader to infer from
# a failure:
#   - the service names a definition declaring `runtime_tools: "sandbox"`. **No shipped service
#     does**, so this proof currently needs one written for it; `relationship-intelligence` is
#     read-only records and declares `none`.
#   - IronClaw is running with hosted-single-tenant-volume-sandboxed, so the member's catalogue
#     holds a shell capability. `dev/probe-up.sh` brings up a plain stack and does not.
#   - builtin.shell is allowed for this client; authority-changing and general network tools are not
#   - the normal IRONWORKS_BASE, IRONWORKS_TOKEN, IRONWORKS_RECORDS and
#     IRONWORKS_RECORDS_TOKEN variables are exported
#
# The preflights can only rule this proof OUT. Passing them certifies nothing about the sandbox —
# that is what the three turns below are for.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT" || exit 3

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-workspace.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
# The two defaults must agree: `Guidance::parse` refuses guidance bound to another slug, so a
# default slug the default guidance file does not name blocks every turn below before the sandbox
# is reached — a proof failing on its own fixture rather than on the property. `eval` is the slug
# `fixtures/eval.guidance.md` declares. Override both together or neither.
GUIDANCE="${IRONWORKS_WORKSPACE_GUIDANCE:-fixtures/eval.guidance.md}"
SLUG="${IRONWORKS_WORKSPACE_SLUG:-eval}"
SERVICE="relationship-intelligence"
ARTIFACT="/workspace/ironworks-persistence-proof"

verdict() { printf '%-9s %-28s %s\n' "$2" "$1" "${3:-}"; }
blocked() { verdict "$1" BLOCKED "$2"; exit 3; }
failed() { verdict "$1" FAIL "$2"; exit 2; }

for name in IRONWORKS_BASE IRONWORKS_TOKEN IRONWORKS_RECORDS IRONWORKS_RECORDS_TOKEN; do
  [ -n "${!name:-}" ] || blocked "environment" "$name is unset"
done
[ -f "$GUIDANCE" ] || blocked "guidance" "$GUIDANCE does not exist"
curl -fsS --max-time 3 "$IRONWORKS_BASE/api/health" >/dev/null 2>&1 \
  || blocked "runtime.reachable" "$IRONWORKS_BASE did not answer /api/health"
cargo build -q -p ironworks-cli 2>/dev/null || blocked "build" "cargo build failed"
BIN=./target/debug/ironworks

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
[ -f "$DEFINITION" ] || blocked "service.definition" "$DEFINITION does not exist"
RUNTIME_TOOLS="$(python3 -c '
import json,sys
try: print(json.load(open(sys.argv[1]))["capabilities"]["runtime_tools"])
except Exception: pass' "$DEFINITION")"
[ "$RUNTIME_TOOLS" = "sandbox" ] || blocked "service.runtime_tools" \
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
[ "$SHELL_ROWS" = "?" ] || [ "$SHELL_ROWS" != "0" ] || blocked "runtime.sandbox_profile" \
  "the member catalogue holds no shell capability — IronClaw is not running \
hosted-single-tenant-volume-sandboxed. dev/probe-up.sh brings up a plain stack."

turn() {
  local journal="$1" ask="$2"
  "$BIN" turn --slug "$SLUG" --guidance "$GUIDANCE" --service "$SERVICE" \
    --journal "$journal" --ask "$ask" 2>/dev/null
}
extract_hex() { grep -Eo '[0-9a-f]{64}' | head -n1; }

# A turn that never completed and a turn that completed with the wrong answer are different facts,
# and only the second is evidence about the sandbox. Splitting the exit code from the body is what
# lets the two carry different verdicts — BLOCKED for "no measurement", FAIL for "measured, wrong".
create="$(turn "$WORK/create.db" \
  "Use builtin.shell. Run exactly this command inside the sandbox: test ! -e /var/run/docker.sock && umask 077 && python3 -c \"import secrets; open('$ARTIFACT','w').write(secrets.token_hex(32)+'\\n')\". Do not read the file. Reply only SAVED if the command succeeds.")"
create_rc=$?
[ "$create_rc" -eq 0 ] || blocked "workspace.create" \
  "the sandboxed turn did not complete (exit $create_rc); no answer to judge"
[ "$create" = "SAVED" ] || failed "workspace.create" "expected exactly SAVED, got: $create"
verdict "workspace.create" PASS "worker wrote /workspace and had no Docker socket"

read_one="$(turn "$WORK/read-one.db" \
  "This is a fresh thread. Use builtin.shell to read $ARTIFACT and reply with only its contents.")"
read_one_rc=$?
[ "$read_one_rc" -eq 0 ] || blocked "workspace.read.first" \
  "the fresh read turn did not complete (exit $read_one_rc); no answer to judge"
value_one="$(printf '%s' "$read_one" | extract_hex)"
[ -n "$value_one" ] || failed "workspace.read.first" "no 64-hex sandbox value was returned"
verdict "workspace.read.first" PASS "$value_one"

read_two="$(turn "$WORK/read-two.db" \
  "This is another fresh thread. Use builtin.shell to read $ARTIFACT and reply with only its contents.")"
read_two_rc=$?
[ "$read_two_rc" -eq 0 ] || blocked "workspace.read.second" \
  "the second fresh read turn did not complete (exit $read_two_rc); no answer to judge"
value_two="$(printf '%s' "$read_two" | extract_hex)"
[ -n "$value_two" ] || failed "workspace.read.second" "no 64-hex sandbox value was returned"
[ "$value_one" = "$value_two" ] \
  || failed "workspace.persistence" "fresh threads returned different values"
verdict "workspace.persistence" PASS "three fresh Responses threads shared one client workspace"

printf '\nPASS — IronWorks exercised IronClaw\x27s persistent sandbox workspace.\n'
