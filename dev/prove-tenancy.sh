#!/usr/bin/env bash
# COST: free — no runtime, no model key, nothing spent.
# Prove two tenants served by one store cannot reach each other's records.
#
#   dev/prove-tenancy.sh          # this; needs no runtime and no model key
#
# Seven checks. Like prove-lifecycle.sh this one costs nothing and touches no model: separation is
# a property of the record path, and asking a model about it would prove something else.
#
#   1. control      tenant-a reads its own record        200, found, scoped to tenant-a
#   2. control      tenant-b reads its own record        200, found, scoped to tenant-b
#   3. isolation    tenant-a cannot read tenant-b's      404, not found
#   4. isolation    tenant-b cannot read tenant-a's      404, not found
#   5. isolation    the two listings share no record     and neither listing is empty
#   6. negative     an unknown credential                401, never an empty 200
#   7. control      the same read with the store gone    BLOCKED, and never FAIL
#
# ── why checks 1 and 2 are not padding ──────────────────────────────────────────────────────
#
# Gate 2's own plan says it plainly: *an empty database passes every isolation check.* Checks 3
# and 4 assert a record is NOT reachable, and "not reachable" is exactly what a store returns for
# a record that does not exist anywhere. So each of them is only worth something because the
# check above it proved the same id IS reachable under its own credential, in the same run,
# against the same process. Read 1+4 and 2+3 as pairs; either half alone certifies nothing.
#
# The same reasoning is why `ironworks-devstore` refuses to start with an empty book, and why a
# book binds to an org: one book shared by two identities makes every check here pass while
# measuring nothing at all.
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves the record path is org-scoped end to end: a credential resolves to an org server-side
# and the answer carries only that org's records. It does NOT prove tenant separation anywhere
# else — not in the channel, the journal, the thread store, or the alloy — and it is a disposable
# fixture rather than the Account Service, so it speaks for the contract and not the deployment.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

PORT="${PROVE_TENANCY_PORT:-3994}"
A_TOKEN="dev-token-tenant-a"; A_ORG="tenant-a"; A_REC="NW-001"
B_TOKEN="dev-token-tenant-b"; B_ORG="tenant-b"; B_REC="SV-004"
STORE_PID=""
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() {
  [ -n "$STORE_PID" ] && kill "$STORE_PID" 2>/dev/null
  rm -f "${TMPDIR:-/tmp}/ironworks-tenancy.$$"
}
on_exit cleanup

need_jq

cargo build -q -p ironworks-devstore 2>/dev/null || {
  give_up "build" "cargo build failed"; }

# REFUSE TO MEASURE A STORE THIS SCRIPT DID NOT START. If anything already answers on the port,
# our own store never binds, every request goes to the stranger, and the run reports on a config
# nobody here chose. That is not a failure of separation, so it must not read as one: it is the
# textbook BLOCKED — the measurement could not be performed. (Observed: a store left behind by a
# concurrent session answered here, its tokens differed, and five checks "failed" against it.)
if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
  give_up "store.port" \
    "something already answers on $PORT; set PROVE_TENANCY_PORT to a free port"
fi

# One store, two orgs, one book each. The fixtures are the reference's ported proof pair.
STORE_PID="$(start_fixture "http://127.0.0.1:$PORT/health" \
  ./target/debug/ironworks-devstore --port "$PORT" \
  --book "$A_ORG=fixtures/tenants/tenant-a.account.json" --identity "$A_TOKEN=$A_ORG" \
  --book "$B_ORG=fixtures/tenants/tenant-b.account.json" --identity "$B_TOKEN=$B_ORG")"
[ -n "$STORE_PID" ] || give_up "store.up" "devstore did not come up on $PORT"

# `http <token> <path>` prints "<http status> <body>" so a check can assert on both — and prints
# NOTHING AT ALL when no HTTP response came back, which is the distinction every caller below
# turns into BLOCKED rather than FAIL.
#
# It has to be the absence of output, because there is no status that means "there was no status".
# curl reports a refused or dropped connection as `000`, a code the store cannot return, and it
# used to be printed as though it were one: an unconditional `printf ' '` made the result " " or
# "000 ", the empty-result branch below was unreachable, and a store that went away mid-run was
# reported as a tenancy FAILURE. Losing the subject of the measurement is the textbook BLOCKED,
# and check 7 holds this honest by taking the store away on purpose.
http() {
  local body="${TMPDIR:-/tmp}/ironworks-tenancy.$$" code
  code="$(curl -sS -o "$body" -w '%{http_code}' -H "X-Service-Token: $1" \
    "http://127.0.0.1:$PORT/$2" 2>/dev/null)"
  case "$code" in
    ""|000) rm -f "$body"; return 3 ;;
  esac
  printf '%s %s' "$code" "$(cat "$body" 2>/dev/null)"
  rm -f "$body"
}

ctx() { http "$1" "get_account_context?account_id=$2"; }

check_own() {   # name token org record
  local out; out="$(ctx "$2" "$4")"
  case "$out" in
    "") verdict "$1" BLOCKED "the store did not answer" ;;
    200\ *) case "$out" in
              *'"found":true'*) case "$out" in
                  *"\"org\":\"$3\""*) verdict "$1" PASS "$4 readable as $3" ;;
                  *) verdict "$1" FAIL "found, but not scoped to $3: $out" ;;
                esac ;;
              *) verdict "$1" FAIL "200 without found:true: $out" ;;
            esac ;;
    *) verdict "$1" FAIL "expected 200 for its own record, got: $out" ;;
  esac
}

check_denied() { # name token record owner
  local out; out="$(ctx "$2" "$3")"
  case "$out" in
    "") verdict "$1" BLOCKED "the store did not answer" ;;
    404\ *'"found":false'*) verdict "$1" PASS "$3 ($4's) not reachable" ;;
    200\ *) verdict "$1" FAIL "CROSS-TENANT READ: served $4's $3" ;;
    *) verdict "$1" FAIL "expected a 404 not-found, got: $out" ;;
  esac
}

check_own    "control.a_reads_own"      "$A_TOKEN" "$A_ORG" "$A_REC"
check_own    "control.b_reads_own"      "$B_TOKEN" "$B_ORG" "$B_REC"
check_denied "isolation.a_cannot_read_b" "$A_TOKEN" "$B_REC" "$B_ORG"
check_denied "isolation.b_cannot_read_a" "$B_TOKEN" "$A_REC" "$A_ORG"

# Listings must be non-empty on both sides and share nothing. Empty listings would make the
# disjointness trivially true, so emptiness is a FAIL here and not a pass.
a_out="$(http "$A_TOKEN" list_accounts)"
b_out="$(http "$B_TOKEN" list_accounts)"
# `jq`, not `grep -o '"account_id":"[^"]*"'`. That pattern cannot tell a key from a value: it
# matches an `account_id` anywhere in the document, including inside a record body, so a record
# quoting another tenant's id would have read as a shared listing — a FAIL invented by the reader
# rather than measured. `.accounts[]` says which field, at which depth.
a_ids="$(json_field '[.accounts[].account_id] | sort | .[]' "$a_out")"
b_ids="$(json_field '[.accounts[].account_id] | sort | .[]' "$b_out")"
if [ -z "$a_out" ] || [ -z "$b_out" ]; then
  verdict "isolation.listings_disjoint" BLOCKED "the store did not answer"
elif [ -z "$a_ids" ] || [ -z "$b_ids" ]; then
  verdict "isolation.listings_disjoint" FAIL "a listing was empty; disjointness would be vacuous"
else
  shared="$(comm -12 <(printf '%s\n' "$a_ids") <(printf '%s\n' "$b_ids"))"
  if [ -z "$shared" ]; then
    verdict "isolation.listings_disjoint" PASS \
      "$(printf '%s\n' "$a_ids" | wc -l | tr -d ' ') and $(printf '%s\n' "$b_ids" | wc -l | tr -d ' ') records, nothing in common"
  else
    verdict "isolation.listings_disjoint" FAIL "both tenants see: $shared"
  fi
fi

unknown="$(http "not-a-real-token" list_accounts)"
case "$unknown" in
  "")      verdict "negative.unknown_credential" BLOCKED "the store did not answer" ;;
  401\ *)  verdict "negative.unknown_credential" PASS "401, and no records" ;;
  200\ *)  verdict "negative.unknown_credential" FAIL "an unknown credential was served" ;;
  *)       verdict "negative.unknown_credential" FAIL "expected 401, got: $unknown" ;;
esac

# ── 7. the control for the verdicts themselves ──────────────────────────────────────────────
#
# Every check above distinguishes "the store answered and the answer was wrong" from "there was
# no answer", and the first five would read identically if that distinction were broken — which
# it was. So the run ends by taking the store away and repeating check 1, whose PASS is on the
# record a few lines up. The same request must now report BLOCKED. Self-contained: it needs
# nothing this script did not start, and it is the reason the BLOCKED branches are not decorative.
kill "$STORE_PID" 2>/dev/null
STORE_PID=""
for _ in $(seq 1 30); do
  curl -fsS --max-time 1 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 || break
  sleep 0.2
done
gone="$(ctx "$A_TOKEN" "$A_REC")"
case "$gone" in
  "") verdict "control.vanished_store_blocks" PASS "the same read reports no answer, not a leak" ;;
  *)  verdict "control.vanished_store_blocks" FAIL \
        "a store that is gone answered anyway; a check cannot tell loss from failure: $gone" ;;
esac

echo
summary
