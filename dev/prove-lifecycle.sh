#!/usr/bin/env bash
# COST: free — no runtime, no model key, nothing spent.
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
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

PORT="${PROVE_LIFECYCLE_PORT:-3993}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ironworks-lifecycle.XXXXXX")"
STORE_PID=""
# shellcheck disable=SC2329  # invoked indirectly, through `on_exit`.
cleanup() { [ -n "$STORE_PID" ] && kill "$STORE_PID" 2>/dev/null; rm -rf "$WORK"; }
on_exit cleanup

cargo build -q -p ironworks-cli -p ironworks-devstore 2>/dev/null || {
  give_up "build" "cargo build failed"; }
BIN=./target/debug/ironworks

# A real Account Service. `--identity TOKEN=ORG` is exactly the identity map the reference's
# `identities.py` owns, which is what makes this a reconciliation target rather than a mock.
account_service() {
  [ -n "$STORE_PID" ] && { kill "$STORE_PID" 2>/dev/null; wait "$STORE_PID" 2>/dev/null; STORE_PID=""; }
  # The org half of TOKEN=ORG is what the book binds to, so the two cannot drift apart here.
  STORE_PID="$(start_fixture "http://127.0.0.1:$PORT/health" \
    ./target/debug/ironworks-devstore --port "$PORT" --book "${1#*=}=fixtures/book" \
    --identity "$1")"
  [ -n "$STORE_PID" ]
}

inspect() { # inspect <store> [extra args...]
  local s="$1"; shift
  IRONWORKS_RECORDS="http://127.0.0.1:$PORT" IRONWORKS_RECORDS_TOKEN=dev-token-acme \
    "$BIN" lifecycle inspect acme --store "$s" --account-service ${@+"$@"} 2>&1
}
org_line() { grep -E '^  org_registration' <<<"$1" | sed 's/^ *//'; }

# The seed records the org registration under the id `org_registration-1`. The reconciler reports
# whatever org the credential actually opens, so the two agree only when the service says so.
#
# `org_registered` is the whole requirement: this proof speaks for org registration and nothing
# else, as the header says. Seeding further would claim stages this measures nothing about, and
# past `member_minted` a seed cannot go at all — see `crates/lifecycle/examples/seed.rs`.
cargo run -q -p ironworks-lifecycle --example seed -- "$WORK/match.db" acme org_registered || {
  give_up "seed" "could not seed a lifecycle store"; }
# BOTH tables, and that is not belt-and-braces. `reach` writes the created id to `stage_history`
# and to `authority_identities`, and reconciliation reads the latter — so patching only the former
# left this fixture recording `org_registration-1`, the reconciler reporting UNMANAGED, and check 1
# passing anyway on the substring below. Measured: with both patched the same command reports
# `MANAGED [acme-sales]`.
python3 - "$WORK/match.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("UPDATE stage_history SET created_id='acme-sales' WHERE stage='org_registered'")
c.execute("UPDATE authority_identities SET opaque_id='acme-sales' WHERE authority='org_registration'")
c.commit()
PY

# ── 1. matching org registration ─────────────────────────────────────────────────────────
if ! account_service "dev-token-acme=acme-sales"; then
  verdict "reconcile.matching_org" BLOCKED "no account service on :$PORT"
else
  out="$(inspect "$WORK/match.db")"
  # Anchored on a word boundary: `MANAGED [acme-sales]` is a substring of
  # `UNMANAGED [acme-sales]`, so the unanchored pattern passed this check on the exact result it
  # exists to rule out — and this is the positive control the three refusals below depend on.
  if grep -qE '(^|[[:space:]])MANAGED \[acme-sales\]' <<<"$out"; then
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
cargo run -q -p ironworks-lifecycle --example seed -- "$WORK/empty.db" acme preflight_passed || {
  give_up "seed" "could not seed a lifecycle store"; }
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

summary 'every run above is BLOCKED overall: this reconciler speaks for org registration only.'
