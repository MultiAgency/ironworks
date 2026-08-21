#!/usr/bin/env bash
# Operator-controlled client deletion — the counterpart of provision.sh.
# NOT a model tool, NOT a client API: a human runs this, on the host, with operator creds.
#
#   ./deprovision.sh <slug>                          # DRY RUN: inventory only, deletes nothing
#   ./deprovision.sh <slug> --execute --confirm <slug>   # destructive: requires the slug typed twice
#
# What it covers (live systems on THIS machine):
#   1. access/routing first: client registry env + guidance file, bridge thread-state entry,
#      org token deregistered from the hot-reloaded identities file
#   2. the sealed IronClaw account (supported API: DELETE /api/webchat/v2/admin/users/<id>)
#   3. Account Store rows in ONE transaction: activities, contacts, accounts, organization
#   4. operator-curated staging material: ~/.agency/account-data/<slug>/ and <slug>.env
# What it deliberately does NOT touch:
#   - backups (Hetzner images / restic snapshots age out on their documented schedules;
#     after any restore of a backup that predates a deletion, RE-RUN this script)
#   - shared logs (journald); they carry service events, not account content
#   - the OTHER machine's staging copies — run the script there too (it prints a reminder)
# Safety: slug must match ^[a-z0-9-]+$ exactly (no wildcards/prefixes possible — every SQL
# statement uses a bound parameter), the registry's ORG_ID is the only org key used, and
# execution requires --confirm <slug> to match. Audit output is COUNTS ONLY — no content.
#
# Env (defaults fit the standard layout):
#   CLIENTS_DIR (~/.agency/clients)  IDENTITIES_FILE (~/.agency/account-identities/identities.json)
#   ACCOUNT_DB_CONTAINER (auto-detect *account-db*)  IRONCLAW_API (http://127.0.0.1:3020)
#   WEBUI_TOKEN (operator token, needed only for the sealed-account deletion step)
#   BRIDGE_STATE (~/.agency/bridge-threads.json)
set -euo pipefail
. "$(dirname "$0")/../../deploy/lib/fleet.sh"   # curl_bearer (tokens off argv) + fleet_env_get

SLUG="${1:?usage: deprovision.sh <slug> [--execute --confirm <slug>]}"
shift || true
EXECUTE=0 CONFIRM=""
while [ $# -gt 0 ]; do case "$1" in
  --execute) EXECUTE=1 ;;
  --confirm) CONFIRM="${2:-}"; shift ;;
  *) echo "!! unknown arg: $1" >&2; exit 2 ;;
esac; shift; done

case "$SLUG" in (*[!a-z0-9-]*|"") echo "!! slug must be exact lowercase [a-z0-9-]+ — no wildcards, no prefixes" >&2; exit 2 ;; esac

CLIENTS_DIR="${CLIENTS_DIR:-$HOME/.agency/clients}"
IDENTITIES_FILE="${IDENTITIES_FILE:-$HOME/.agency/account-identities/identities.json}"
BRIDGE_STATE="${BRIDGE_STATE:-$HOME/.agency/bridge-threads.json}"
IRONCLAW_API="${IRONCLAW_API:-http://127.0.0.1:3020}"
DATA_DIR="$HOME/.agency/account-data/$SLUG"
DATA_ENV="$HOME/.agency/account-data/$SLUG.env"
ENV_FILE="$CLIENTS_DIR/$SLUG.env"
GUIDE_FILE="$CLIENTS_DIR/$SLUG.guidance.md"
DB_CONT="${ACCOUNT_DB_CONTAINER:-$(docker ps --format '{{.Names}}' | grep 'account-db' | head -1 || true)}"   # no match must not abort under pipefail — the guard below is the handler
[ -n "$DB_CONT" ] || { echo "!! no account-db container found (set ACCOUNT_DB_CONTAINER)" >&2; exit 1; }

# ── Resolve the org strictly from the client's own registry record ────────────────────
ORG_ID="" IC_UID="" GROUP_ID="" ACCT_TOKEN=""
if [ -f "$ENV_FILE" ]; then
  # fleet_env_get, not a local sed: this reader was the STRICTEST of the four in the tree and
  # that was load-bearing in the wrong direction. It matched only a bare or double-quoted value
  # anchored to end-of-line, so a single-quoted or whitespace-padded IRONCLAW_USER_ID parsed
  # EMPTY here and nowhere else — step 2 then printed "SKIPPED (need IRONCLAW_USER_ID…)", never
  # set DEGRADED, and the run exited 0 with the sealed account still alive. Same class silently
  # skipped the bridge-state removal (GROUP_ID) and the 401 verification (ACCOUNT_TOKEN).
  ORG_ID=$(fleet_env_get "$ENV_FILE" ORG_ID)
  IC_UID=$(fleet_env_get "$ENV_FILE" IRONCLAW_USER_ID)
  GROUP_ID=$(fleet_env_get "$ENV_FILE" TELEGRAM_GROUP_ID)
  ACCT_TOKEN=$(fleet_env_get "$ENV_FILE" ACCOUNT_TOKEN)
fi
ORG_ID="${ORG_ID:-$SLUG}"
case "$ORG_ID" in (*[!a-zA-Z0-9._-]*|"") echo "!! refusing suspicious ORG_ID" >&2; exit 2 ;; esac

sql_count() { echo "SELECT count(*) FROM $1 WHERE org_id = :'org';" | \
  docker exec -i "$DB_CONT" psql -qtA -U postgres -d accounts -v org="$ORG_ID"; }

echo "== deprovision inventory for client '$SLUG' (org '$ORG_ID') =="
A=$(sql_count accounts); C=$(sql_count contacts); T=$(sql_count activities); O=$(sql_count organizations)
TOK_REG=$( [ -f "$IDENTITIES_FILE" ] && python3 -c "
import json,sys
d=json.load(open('$IDENTITIES_FILE'))
print(sum(1 for t,o in d.items() if o=='$ORG_ID'))" || echo 0 )
BR=$( [ -f "$BRIDGE_STATE" ] && [ -n "$GROUP_ID" ] && python3 -c "
import json;d=json.load(open('$BRIDGE_STATE'));print(1 if '$GROUP_ID' in d else 0)" || echo 0 )
printf '   account-store rows: organizations=%s accounts=%s contacts=%s activities=%s\n' "$O" "$A" "$C" "$T"
printf '   identity tokens registered for org: %s\n' "$TOK_REG"
printf '   registry env: %s · guidance: %s\n' "$([ -f "$ENV_FILE" ] && echo present || echo absent)" "$([ -f "$GUIDE_FILE" ] && echo present || echo absent)"
printf '   bridge thread-state entry: %s · sealed ironclaw user id: %s\n' "$BR" "${IC_UID:-unknown}"
printf '   staging: dir %s · env %s\n' "$([ -d "$DATA_DIR" ] && echo present || echo absent)" "$([ -f "$DATA_ENV" ] && echo present || echo absent)"
echo "   note: backups are NOT rewritten (they expire on schedule); other-machine staging is NOT touched"

if [ "$EXECUTE" -ne 1 ]; then echo; echo "DRY RUN ONLY — nothing deleted. Re-run with: --execute --confirm $SLUG"; exit 0; fi
[ "$CONFIRM" = "$SLUG" ] || { echo "!! --confirm must repeat the slug exactly ('$CONFIRM' != '$SLUG') — aborting" >&2; exit 2; }

echo; echo "== EXECUTING deletion for '$SLUG' =="
DEGRADED=0     # any step that could not fully complete flips this; summarized at the end
echo "-- 1/4 revoke access & routing first"
RM_FILES=0
for f in "$ENV_FILE" "$GUIDE_FILE"; do
  [ -f "$f" ] || continue                          # absent = already gone (fine)
  # A FAILED rm here is not benign: the registry env file staying means the bridge would still load
  # and route this client on its next restart. Surface it loudly rather than swallowing it in an &&
  # chain (which set -e ignores) and reporting a clean deprovision.
  if rm -f "$f"; then RM_FILES=$((RM_FILES+1)); else
    echo "  !! could NOT remove $f — this client may remain routable; remove it by hand" >&2; DEGRADED=1
  fi
done
if [ -f "$BRIDGE_STATE" ] && [ -n "$GROUP_ID" ]; then
  python3 - "$BRIDGE_STATE" "$GROUP_ID" <<'PY'
import json, os, sys, tempfile
path, gid = sys.argv[1], sys.argv[2]
d = json.load(open(path))
if gid in d:
    del d[gid]
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path)); os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w") as f: json.dump(d, f)
    os.replace(tmp, path)
PY
fi
if [ -f "$IDENTITIES_FILE" ]; then
  python3 - "$IDENTITIES_FILE" "$ORG_ID" <<'PY'
import json, os, sys, tempfile
path, org = sys.argv[1], sys.argv[2]
d = json.load(open(path))
kept = {t: o for t, o in d.items() if o != org}
fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path)); os.fchmod(fd, 0o600)
with os.fdopen(fd, "w") as f: json.dump(kept, f)
os.replace(tmp, path)
print(f"   deregistered {len(d)-len(kept)} org token(s) (hot-reloaded, effective now)")
PY
fi

echo "-- 2/4 delete the sealed IronClaw account (supported admin API)"
if [ -n "$IC_UID" ] && [ -n "${WEBUI_TOKEN:-}" ]; then
  code=$(curl_bearer "$WEBUI_TOKEN" -s -o /dev/null -w '%{http_code}' -X DELETE \
    "$IRONCLAW_API/api/webchat/v2/admin/users/$IC_UID")
  echo "   DELETE admin/users -> HTTP $code"
  case "$code" in
    2??|404) ;;   # 404 = already gone
    *) echo "   !! sealed account NOT deleted (HTTP $code — stale/ambient WEBUI_TOKEN?); continuing, but this run is DEGRADED" >&2
       DEGRADED=1 ;;
  esac
  # LIMITATION (verified against ironclaw): deleting the user record does NOT revoke an already
  # issued member token. The token is a signed session (HMAC over sid/tenant/user/expiry); the
  # server's revoked-set is in-memory only, and admin_delete_user never adds to it — so the token
  # KEEPS AUTHENTICATING /v1/responses until it expires. There is no admin "revoke another user's
  # session" API (only /auth/logout revokes the caller's own). So this does NOT immediately cut the
  # client off. Real cut-off requires the token to expire, or rotating IRONCLAW_REBORN_WEBUI/session
  # signing key (invalidates ALL sessions). Treat retrieval of the client's token file + short
  # session TTLs as the mitigation; do NOT assume this DELETE ended their access.
  echo "   NOTE: user record deleted, but the client's member TOKEN stays valid until expiry" >&2
  echo "         (no admin session-revoke API upstream) — see the comment in this script." >&2
else
  echo "   SKIPPED (need IRONCLAW_USER_ID in registry + WEBUI_TOKEN env). NOTE: the org (Account"
  echo "   Service) token IS revoked by step 1, but the IronClaw MEMBER token is not — it stays valid"
  echo "   until expiry regardless (no admin session-revoke API). Delete the user record manually."
fi

echo "-- 3/4 delete Account-Store rows in one transaction"
docker exec -i "$DB_CONT" psql -q -U postgres -d accounts -v org="$ORG_ID" <<'SQL'
BEGIN;
DELETE FROM activities    WHERE org_id = :'org';
DELETE FROM contacts      WHERE org_id = :'org';
DELETE FROM accounts      WHERE org_id = :'org';
DELETE FROM organizations WHERE org_id = :'org';
COMMIT;
SQL

echo "-- 4/4 remove operator-curated staging material on THIS machine"
RM_STAGE=0
[ -d "$DATA_DIR" ] && rm -rf "$DATA_DIR" && RM_STAGE=$((RM_STAGE+1))
[ -f "$DATA_ENV" ] && rm "$DATA_ENV" && RM_STAGE=$((RM_STAGE+1))

echo; echo "== post-delete verification =="
A2=$(sql_count accounts); C2=$(sql_count contacts); T2=$(sql_count activities); O2=$(sql_count organizations)
printf '   rows now: organizations=%s accounts=%s contacts=%s activities=%s (want all 0)\n' "$O2" "$A2" "$C2" "$T2"
if [ -n "$ACCT_TOKEN" ]; then
  hc=$(curl_header "X-Service-Token: $ACCT_TOKEN" -s -o /dev/null -w '%{http_code}' \
    "${ACCOUNT_BASE:-http://127.0.0.1:8443}/list_accounts")
  echo "   revoked org token against the Account Service -> HTTP $hc (want 401)"
fi
echo
echo "AUDIT '$SLUG': deleted rows org=$O acct=$A contact=$C activity=$T · files removed: registry+guidance=$RM_FILES staging=$RM_STAGE · tokens deregistered: $TOK_REG · bridge entry removed: $BR"
echo "REMAINING BY DESIGN: backup copies until scheduled expiry; journald service logs; the other machine's staging (run this script there); restart the bridge to drop in-memory routing."
if [ "$DEGRADED" = 1 ]; then
  echo "!! DEGRADED: at least one revocation step did not fully complete (see the '!!' line(s) above)." >&2
  echo "   - registry file not removed -> the client may still be routed (remove it by hand)." >&2
  echo "   - sealed account not deleted -> re-run step 2 with a current operator token:" >&2
  echo "     curl -X DELETE -H \"Authorization: Bearer \$WEBUI_TOKEN\" $IRONCLAW_API/api/webchat/v2/admin/users/$IC_UID" >&2
  exit 1
fi