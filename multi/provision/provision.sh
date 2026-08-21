#!/usr/bin/env bash
# MODEL_PIN: the model of record (repo root) — keep the smoke turn on the same model the
# product runs, or the smoke proves a path production never takes.
# Composed client provisioning: one command turns "<Name>" into a servable client.
#
#   client = (Account-Service org + org token)              — their private data scope
#          + (sealed IronClaw member account + token)       — their private agent scope
#          + (Telegram group id)                            — their channel
#          + (~/.agency/clients/<slug>.env)                — the seam/bridge registration
#
# Usage:
#   IRONCLAW_API=http://127.0.0.1:3020 \
#   IRONCLAW_OPERATOR_TOKEN=<operator token> \
#   ./provision.sh <slug> "<Display Name>" <telegram_group_id>
#
# Optional data: put candidate-shaped *.json under ~/.agency/account-data/<slug>/ BEFORE
# running and it is seeded into the new org (via seed-real.sh); with no data dir the org
# starts empty (valid — org existence is row scoping).
#
# The Telegram group itself stays manual (create private group, add the bot as admin, get the
# id — procedure in multi/seam/README.md), as does restarting the bridge.
#
# Org tokens are HOT-RELOADED by the Account Service from ~/.agency/account-identities/
# identities.json (ACCOUNT_IDENTITIES_FILE) — provisioning never restarts the data layer,
# so current clients see zero interruption.
set -euo pipefail
cd "$(dirname "$0")"
. ../../deploy/lib/fleet.sh   # curl_header/curl_bearer (tokens off argv) + fleet_* helpers
MODEL_PIN="$(fleet_model_pin)"   # MODEL env still wins; no fallback literal (see fleet.sh)
SLUG="${1:?usage: provision.sh <slug> \"<Display Name>\" <telegram_group_id>}"
NAME="${2:?client display name, e.g. \"Acme Corp\"}"
GROUP_ID="${3:?telegram group id (negative for groups; see multi/seam/README.md to obtain)}"
API="${IRONCLAW_API:?set IRONCLAW_API (the multi-tenant instance base URL)}"
: "${IRONCLAW_OPERATOR_TOKEN:?set IRONCLAW_OPERATOR_TOKEN (a current operator/admin token)}"
ACCOUNT_BASE="${ACCOUNT_BASE:-http://127.0.0.1:8443}"
CONT="${ACCOUNT_SERVICE_CONTAINER:-multiagency-data-account-service-1}"
DATA_DIR_ROOT="$HOME/.agency/account-data"
CLIENTS_DIR="${CLIENTS_DIR:-$HOME/.agency/clients}"
DATA_COMPOSE_DIR="$(cd ../../deploy/account-intel/data && pwd)"

case "$SLUG" in *[!a-z0-9-]*) echo "!! slug must be lowercase [a-z0-9-]: $SLUG" >&2; exit 1;; esac
[ -f "$CLIENTS_DIR/$SLUG.env" ] && { echo "!! client already provisioned: $CLIENTS_DIR/$SLUG.env" >&2; exit 1; }
# refuse a group id already mapped to another client — a duplicate would misroute that whole
# group to the wrong client's tokens/data (load_clients also fails closed on this at runtime).
if [ -d "$CLIENTS_DIR" ]; then
  for cf in "$CLIENTS_DIR"/*.env; do
    [ -e "$cf" ] || continue
    # a registry env without TELEGRAM_GROUP_ID is legal: fleet_env_get returns empty rather than
    # non-zero, so `exist` simply never equals $GROUP_ID below
    exist=$(fleet_env_get "$cf" TELEGRAM_GROUP_ID)
    [ "$exist" = "$GROUP_ID" ] && { echo "!! TELEGRAM_GROUP_ID $GROUP_ID already maps to client $(basename "$cf" .env) — one group = one client" >&2; exit 1; }
  done
fi
fleet_require_container "$CONT" || { echo "   (the account service must be up before provisioning)" >&2; exit 1; }

ORG_ID="$SLUG"
ACCOUNT_TOKEN="$(openssl rand -hex 24)"

echo "== 1/4 Account-Service org '$ORG_ID' + token (hot-reloaded — zero interruption) =="
mkdir -p "$DATA_DIR_ROOT"
umask 077
cat > "$DATA_DIR_ROOT/$SLUG.env" <<EOF
REAL_ORG_ID="$ORG_ID"
REAL_ORG_NAME="$NAME"
REAL_ACCOUNT_TOKEN="$ACCOUNT_TOKEN"
EOF
if ls "$DATA_DIR_ROOT/$SLUG"/*.json >/dev/null 2>&1; then
  "$DATA_COMPOSE_DIR/seed-real.sh" "$SLUG"        # register identity + seed data + isolation smoke
else
  # no data yet: register the identity only (empty org is valid — org existence is row scoping)
  ORG_TOKEN="$ACCOUNT_TOKEN" "$DATA_COMPOSE_DIR/register-identity.sh" "$ORG_ID"
fi

echo "== 2/4 sealed IronClaw member account on $API =="
# No eval: run the child, check its exit status, then parse only the two expected
# KEY=VALUE lines — child stdout is data, never shell code, and a child failure stops us
# BEFORE the step-3/4 side effects instead of dying later at set -u.
if ! sealed_out="$(./provision-client.sh --env "$NAME")"; then
  echo "!! sealed-account provisioning failed. Already created by step 1: org '$ORG_ID' is registered" >&2
  echo "   with a token in ~/.agency/account-identities/identities.json and $DATA_DIR_ROOT/$SLUG.env." >&2
  echo "   Re-running mints a NEW org token and leaves the old entry registered — deregister it" >&2
  echo "   (deprovision.sh, or edit identities.json) if you re-run." >&2
  exit 1
fi
IRONCLAW_USER_ID="$(printf '%s' "$sealed_out" | sed -n 's/^IRONCLAW_USER_ID=//p')"
IRONCLAW_TOKEN="$(printf '%s' "$sealed_out" | sed -n 's/^IRONCLAW_TOKEN=//p')"
[ -n "$IRONCLAW_USER_ID" ] && [ -n "$IRONCLAW_TOKEN" ] || { echo "!! provision-client.sh --env returned no usable IRONCLAW_USER_ID/IRONCLAW_TOKEN" >&2; exit 1; }

# Confine the member to a read-only, NO-EGRESS surface BEFORE it is registered/servable. A fresh
# member ships builtin.http (wildcard egress) always_allow; a prompt-injected turn holding this
# client's private pipeline could otherwise POST it to an attacker host. ironclaw exposes no config
# egress allowlist (compiled-in), so we disable the egress/write/escalate tools per-bearer with the
# member's OWN token and PROBE fail-closed. FATAL if confinement can't be certified — a client that
# cannot be confined must not be provisioned (fail closed, no half-open client).
echo "== 2b/4 confine the sealed member (no-egress, read-only) =="
IRONCLAW_API="$API" IRONCLAW_MEMBER_TOKEN="$IRONCLAW_TOKEN" ./confine-member.sh \
  || { echo "!! member confinement FAILED — refusing to provision an un-confined client" >&2; exit 1; }

echo "== 3/4 register with the seam: $CLIENTS_DIR/$SLUG.env =="
mkdir -p "$CLIENTS_DIR"
# Client business guidance is MANDATORY and fail-closed (the seam refuses a guidance-less
# registry). The operator writes it with the client BEFORE provisioning, from
# multi/clients/GUIDANCE.template.md, slug-bound by its first-line marker.
GUIDANCE_FILE="${GUIDANCE_FILE:-$CLIENTS_DIR/$SLUG.guidance.md}"
[ -f "$GUIDANCE_FILE" ] || { echo "!! no client guidance: $GUIDANCE_FILE" >&2
  echo "   copy multi/clients/GUIDANCE.template.md there, fill it with the client, chmod 600" >&2; exit 1; }
# CWD is already the script dir (cd at the top), so derive the seam path from ".." — NOT from
# $(dirname "$0") again, which re-applies to $0 AFTER the cd and breaks under a relative invocation
# like `bash multi/provision/provision.sh` (found via the end-to-end run).
PYTHONPATH="$(cd ../seam && pwd)" GF="$GUIDANCE_FILE" SLUG="$SLUG" python3 - <<'PY' || exit 1
import os
from persona import load_guidance, GuidanceError
try:
    load_guidance(os.environ["GF"], os.environ["SLUG"])
    print("   guidance validated (slug-bound, non-trivial)")
except GuidanceError as e:
    raise SystemExit(f"!! guidance invalid: {e}")
PY
cat > "$CLIENTS_DIR/$SLUG.env" <<EOF
CLIENT_SLUG="$SLUG"
CLIENT_NAME="$NAME"
ORG_ID="$ORG_ID"
ACCOUNT_TOKEN="$ACCOUNT_TOKEN"
IRONCLAW_TOKEN="$IRONCLAW_TOKEN"
IRONCLAW_USER_ID="$IRONCLAW_USER_ID"
TELEGRAM_GROUP_ID="$GROUP_ID"
EOF

echo "== 4/4 smoke =="
# EVERY LEG ASSERTS, AND A FAILURE STOPS THE RUN.
# This block once only PRINTED. The cross-org leg emitted the literal string
# "LEAK: <org>" when another client's token resolved to THIS org — a real isolation failure —
# and the script went straight on to print "✅ client provisioned". The unknown-token leg
# printed an HTTP code nobody compared. A smoke test whose output nothing reads is a status
# report, not a gate, and this is the last thing between provisioning and a client being
# declared live.
SMOKE_FAIL=0
smoke_fail() { echo "      ^^ FAILED: $1" >&2; SMOKE_FAIL=1; }

echo -n "   org token lists its (own) accounts: "
own=$(curl_header "X-Service-Token: $ACCOUNT_TOKEN" -sf "$ACCOUNT_BASE/list_accounts" 2>/dev/null || true)
if [ -z "$own" ]; then
  echo "(no response)"
  smoke_fail "the org token could not list its own accounts — the store rejected it or is down"
else
  printf '%s' "$own" | ORG="$ORG_ID" python3 -c "
import sys, json, os
d = json.load(sys.stdin)
print(f\"org={d['org']} accounts={len(d['accounts'])}\")
sys.exit(0 if d.get('org') == os.environ['ORG'] else 1)" \
    || smoke_fail "the org token resolved to a DIFFERENT org than the one just created"
fi

echo -n "   unknown token -> 401 (fail closed): "
code=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Service-Token: not-a-real-token-$(openssl rand -hex 8)" "$ACCOUNT_BASE/list_accounts" || true)
echo "$code"
[ "$code" = 401 ] || smoke_fail "an unknown token got HTTP $code, expected 401 — the store is not failing closed"

# isolation: every OTHER registered client token must resolve to its own org, never this one
OTHER=$(ORG="$ORG_ID" python3 -c "
import json, os
try: d = json.load(open(os.path.expanduser('~/.agency/account-identities/identities.json')))
except Exception: d = {}
print(next((t for t, o in d.items() if o != os.environ['ORG']), ''))")
if [ -n "$OTHER" ]; then
  echo -n "   another client's token cannot act as this org (isolation): "
  other=$(curl_header "X-Service-Token: $OTHER" -s "$ACCOUNT_BASE/list_accounts" 2>/dev/null || true)
  printf '%s' "$other" | ORG="$ORG_ID" python3 -c "
import sys, json, os
try:
    d = json.load(sys.stdin)
except Exception:
    print('(unreadable response)'); sys.exit(1)
o = d.get('org')
print('OK' if o != os.environ['ORG'] else 'LEAK: ' + str(o))
sys.exit(1 if o == os.environ['ORG'] else 0)" \
    || smoke_fail "CROSS-ORG LEAK — another client's token resolved to THIS org, or the check could not be read"
else
  echo "   (first client — no other org token to cross-check yet)"
  echo "      NOTE: cross-org isolation is UNPROVEN for this client. There is nothing to"
  echo "      cross-check against until a second client exists — re-run this leg then."
fi

echo -n "   sealed IronClaw token answers: "
ans=$(curl_bearer "$IRONCLAW_TOKEN" -sf -m 120 -X POST -H 'content-type: application/json' \
  -H 'User-Agent: Mozilla/5.0' \
  -d '{"model":"'"$MODEL_PIN"'","input":"Reply with the single word: ready"}' \
  "$API/v1/responses" 2>/dev/null || true)
if [ -z "$ans" ]; then
  echo "(no response)"
  smoke_fail "the sealed member token got no answer from IronClaw — the client cannot be served"
else
  printf '%s' "$ans" | python3 -c "
import sys, json
d = json.load(sys.stdin)
t = ''.join(c.get('text','') for it in d.get('output', []) if it.get('type') == 'message'
            for c in it.get('content', []))
print((t.strip() or d.get('status', '?'))[:40])
sys.exit(0 if t.strip() else 1)" \
    || smoke_fail "the sealed member token produced no message text — the turn did not complete"
fi

echo
if [ "$SMOKE_FAIL" -ne 0 ]; then
  echo "❌ smoke FAILED — NOT declaring '$SLUG' provisioned." >&2
  echo "   The org, sealed member and confinement above may already exist. Fix the cause and" >&2
  echo "   re-run, or tear it down first:" >&2
  echo "     multi/provision/deprovision.sh --execute --confirm $SLUG" >&2
  exit 1
fi
echo "✅ client '$NAME' provisioned as '$SLUG' (org=$ORG_ID, group=$GROUP_ID)"
echo "   -> restart telegram_bridge.py to pick up the new client (it reads $CLIENTS_DIR at startup)"
