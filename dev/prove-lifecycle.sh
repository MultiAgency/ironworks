#!/usr/bin/env bash
# Prove the Account Service reconciler against a real one.
#
#   dev/prove-lifecycle.sh        # this; needs no runtime and no model key
#
# Five checks. Unlike the other two proofs this one costs nothing and touches no model, because
# reconciliation is a read: it asks an authoritative system what exists and reports the answer.
#
#   1. matching org            a credential resolving to this org, and the id we recorded -> MANAGED
#   2. another org's credential the credential opens someone else -> UNMANAGED, not "unexpected"
#   3. service unreachable     nothing was learned -> BLOCKED, never a clean report
#   4. empty journal, live cred authority exists that we never recorded -> UNMANAGED, never absent
#   5. read-only               the store's bytes are identical afterwards
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves the adapter translates a REAL Account Service's answers into the three-valued model
# without inventing a fourth. It does not prove the instance is clean: this reconciler speaks for
# org registration and stays silent about members, confinement, tenant data and the registry
# entry, so every run below is BLOCKED overall on the four it never asked about. That is the
# intended shape — a single reconciler must not be able to certify an instance.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

PORT="${PROVE_LIFECYCLE_PORT:-3993}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-lifecycle.XXXXXX")"
STORE_PID=""
cleanup() { [ -n "$STORE_PID" ] && kill "$STORE_PID" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

pass=0; failed=0; blocked=0
verdict() {
  printf '%-9s %-34s %s\n' "$2" "$1" "${3:-}"
  case "$2" in PASS) pass=$((pass+1));; FAIL) failed=$((failed+1));; BLOCKED) blocked=$((blocked+1));; esac
}

cargo build -q -p ironworks-cli -p ironworks-devstore 2>/dev/null || {
  verdict "build" BLOCKED "cargo build failed"; exit 3; }
cargo run -q -p ironworks-lifecycle --example seed -- "$WORK/lc.db" acme activated || {
  verdict "seed" BLOCKED "could not seed a lifecycle store"; exit 3; }
BIN=./target/debug/ironworks

# A real Account Service. `--identity TOKEN=ORG` is exactly the identity map the reference's
# `identities.py` owns, which is what makes this a reconciliation target rather than a mock.
account_service() {
  [ -n "$STORE_PID" ] && { kill "$STORE_PID" 2>/dev/null; wait "$STORE_PID" 2>/dev/null; STORE_PID=""; }
  # The org half of TOKEN=ORG is what the book binds to, so the two cannot drift apart here.
  ( exec ./target/debug/ironworks-devstore --port "$PORT" --book "${1#*=}=fixtures/book" \
      --identity "$1" ) >/dev/null 2>&1 </dev/null &
  STORE_PID=$!
  disown 2>/dev/null || true
  for _ in $(seq 1 30); do
    curl -fsS --max-time 1 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  STORE_PID=""
  return 1
}

inspect() { # inspect <store> [extra args...]
  local s="$1"; shift
  IRONWORKS_RECORDS="http://127.0.0.1:$PORT" IRONWORKS_RECORDS_TOKEN=dev-token-acme \
    "$BIN" lifecycle inspect acme --store "$s" --account-service ${@+"$@"} 2>&1
}
org_line() { grep -E '^  org_registration' <<<"$1" | sed 's/^ *//'; }

# The seed records the org registration under the id `org_registration-1`. The reconciler reports
# whatever org the credential actually opens, so the two agree only when the service says so.
cargo run -q -p ironworks-lifecycle --example seed -- "$WORK/match.db" acme activated
python3 - "$WORK/match.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("UPDATE stage_history SET created_id='acme-sales' WHERE stage='org_registered'")
c.commit()
PY

# ── 1. matching org registration ─────────────────────────────────────────────────────────
if ! account_service "dev-token-acme=acme-sales"; then
  verdict "reconcile.matching_org" BLOCKED "no account service on :$PORT"
else
  out="$(inspect "$WORK/match.db")"
  if grep -q "MANAGED \[acme-sales\]" <<<"$out"; then
    verdict "reconcile.matching_org" PASS "$(org_line "$out" | cut -c1-58)"
  else
    verdict "reconcile.matching_org" FAIL "$(org_line "$out" | cut -c1-58)"
  fi
fi

# ── 2. the credential opens somebody else ────────────────────────────────────────────────
if ! account_service "dev-token-acme=rival-co"; then
  verdict "reconcile.foreign_credential" BLOCKED "no account service on :$PORT"
else
  out="$(inspect "$WORK/match.db")"
  if grep -q "UNMANAGED \[rival-co\]" <<<"$out"; then
    verdict "reconcile.foreign_credential" PASS "reported as rival-co, not as 'unexpected'"
  else
    verdict "reconcile.foreign_credential" FAIL "$(org_line "$out" | cut -c1-58)"
  fi
fi

# ── 3. the service is not there ──────────────────────────────────────────────────────────
kill "$STORE_PID" 2>/dev/null; wait "$STORE_PID" 2>/dev/null; STORE_PID=""
out="$(inspect "$WORK/match.db")"; rc=$?
if [ $rc -eq 3 ] && grep -q "UNVERIFIABLE" <<<"$(org_line "$out")" \
   && grep -q "verdict    BLOCKED" <<<"$out"; then
  verdict "reconcile.unreachable_blocks" PASS "nothing learned; nothing claimed"
else
  verdict "reconcile.unreachable_blocks" FAIL "rc=$rc $(org_line "$out" | cut -c1-50)"
fi

# ── 4. nothing recorded, authority live ──────────────────────────────────────────────────
# The governing invariant, through a real service: an empty local journal is not evidence that
# nothing exists out there.
cargo run -q -p ironworks-lifecycle --example seed -- "$WORK/empty.db" acme preflight_passed
if ! account_service "dev-token-acme=acme-sales"; then
  verdict "reconcile.empty_journal_live_authority" BLOCKED "no account service on :$PORT"
else
  out="$(inspect "$WORK/empty.db")"
  if grep -q "UNMANAGED \[acme-sales\]" <<<"$out" \
     && ! grep -qE '^  org_registration.*absent' <<<"$out" \
     && grep -q "no record of creating it" <<<"$out"; then
    verdict "reconcile.empty_journal_live_authority" PASS "reported, never absent, never adopted"
  else
    verdict "reconcile.empty_journal_live_authority" FAIL "$(org_line "$out" | cut -c1-58)"
  fi
fi

# ── 5. inspection changed nothing ────────────────────────────────────────────────────────
before="$(md5 -q "$WORK/empty.db" 2>/dev/null || md5sum "$WORK/empty.db" | cut -d' ' -f1)"
inspect "$WORK/empty.db" >/dev/null
after="$(md5 -q "$WORK/empty.db" 2>/dev/null || md5sum "$WORK/empty.db" | cut -d' ' -f1)"
if [ "$before" = "$after" ]; then
  verdict "reconcile.read_only" PASS "store byte-identical after inspection"
else
  verdict "reconcile.read_only" FAIL "the store changed: $before -> $after"
fi

printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
printf 'every run above is BLOCKED overall: this reconciler speaks for org registration only.\n'
[ "$failed" -gt 0 ] && exit 2
[ "$blocked" -gt 0 ] && exit 3
exit 0
