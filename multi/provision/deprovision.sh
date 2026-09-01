#!/usr/bin/env bash
# Operator-controlled client deletion — the counterpart of provision.sh.
# NOT a model tool, NOT a client API: a human runs this, on the host, with operator creds.
#
#   ./deprovision.sh <slug>                          # DRY RUN: inventory only, deletes nothing
#   ./deprovision.sh <slug> --execute --confirm <slug>   # destructive: requires the slug typed twice
#
# EXIT CODES (stable — the console and any wrapper read them):
#   0  deprovisioned, and the member session is VERIFIED REVOKED
#   1  DEGRADED — a revocation step did not complete (see the '!!' lines)
#   2  usage error
#   3  deprovisioned, but the member session STILL AUTHENTICATES (residual authority). This is
#      the expected code on the pinned runtime: deleting a user does not revoke its signed
#      session and no revoke route is mounted (measured — multi/verify/test_session_revocation.py,
#      explained in SECURITY.md). It is recorded in the residual-authority ledger
#      with its expiry, and `ironworks doctor` fails while any entry is outstanding.
#   4  BLOCKED — the instance could not be reached, so revocation could not be established
#      either way. Never treated as success.
#
# What it covers (live systems on THIS machine):
#   1. org token deregistered and its old credential PROVED refused; authenticated scope is
#      retained without credential material so an interrupted run can resume
#   2. the sealed IronClaw account (supported API: DELETE /api/webchat/v2/admin/users/<id>)
#   3. Account Store rows in ONE transaction: activities, contacts, accounts, organization
#   4. operator-curated staging material: ~/.agency/account-data/<slug>/ and <slug>.env
#   5. registry, guidance, and bridge state LAST, after retry evidence is no longer needed
#   6. proof that the RUNNING bridge no longer routes the group — in memory, not just on
#      disk (it runs after 5 precisely because the point is a reload without this tenant)
# What it deliberately does NOT touch:
#   - backups (Hetzner images / restic snapshots age out on their documented schedules;
#     after any restore of a backup that predates a deletion, RE-RUN this script)
#   - shared logs (journald); they carry service events, not account content
#   - the OTHER machine's staging copies — run the script there too (it prints a reminder)
# Safety: slug must match ^[a-z0-9-]+$ exactly (no wildcards/prefixes possible — every SQL
# statement uses a bound parameter), the deletion scope is resolved by authenticating the
# registry's ACCOUNT_TOKEN against the Account Service, and execution requires --confirm <slug>
# to match. Registry ORG_ID is metadata and can never redirect deletion. Audit output is COUNTS
# ONLY — no content.
#
# Env (defaults fit the standard layout):
#   CLIENTS_DIR (~/.agency/clients)  IDENTITIES_FILE (~/.agency/account-identities/identities.json)
#   ACCOUNT_DB_CONTAINER (auto-detect *account-db*)  IRONCLAW_API (http://127.0.0.1:3020)
#   WEBUI_TOKEN (operator token, needed only for the sealed-account deletion step)
#   BRIDGE_STATE (~/.agency/bridge-threads.json — the migration SOURCE)
#   BRIDGE_STATE_DB (derived: ~/.agency/bridge-threads.db — the live store)
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

fleet_slug_valid "$SLUG" \
  || { echo "!! slug must be exact lowercase [a-z0-9-]+ — no wildcards, no prefixes" >&2; exit 2; }

CLIENTS_DIR="${CLIENTS_DIR:-$FLEET_AGENCY_DIR/clients}"
IDENTITIES_FILE="${IDENTITIES_FILE:-$FLEET_AGENCY_DIR/account-identities/identities.json}"
BRIDGE_STATE="${BRIDGE_STATE:-$FLEET_AGENCY_DIR/bridge-threads.json}"
BRIDGE_STATE_DB="${BRIDGE_STATE_DB:-${BRIDGE_STATE%.json}.db}"
IRONCLAW_API="${IRONCLAW_API:-http://127.0.0.1:3020}"
ACCOUNT_BASE_DEFAULT="${ACCOUNT_BASE:-http://127.0.0.1:8443}"
DATA_DIR="$FLEET_AGENCY_DIR/account-data/$SLUG"
DATA_ENV="$FLEET_AGENCY_DIR/account-data/$SLUG.env"
ENV_FILE="$CLIENTS_DIR/$SLUG.env"
STAGED_FILE="$CLIENTS_DIR/.staging/$SLUG.env"
GUIDE_FILE="$CLIENTS_DIR/$SLUG.guidance.md"
LIFECYCLE="$FLEET_REPO_ROOT/deploy/lib/lifecycle.py"
ROUTE_AUTHORITY="$FLEET_REPO_ROOT/deploy/lib/route_revocation.py"
IDENTITIES="$FLEET_REPO_ROOT/deploy/lib/identities.py"   # the one reader AND writer of the map
# ── Read credentials/routing, then AUTHENTICATE the destructive org scope ─────────────
REGISTRY_ORG_ID="" ORG_ID="" IC_UID="" GROUP_ID="" ACCT_TOKEN="" IC_TOKEN=""
RETRY_REGISTRY_FILE=""
ACCOUNT_BASE="$ACCOUNT_BASE_DEFAULT"
# One interpreter start, three values: the teardown scope is a single document and re-parsing
# it once per field starts a fresh python3 for each.
SCOPE_DOC=$(python3 "$LIFECYCLE" teardown get "$SLUG" --json)
# The trailing `echo .` is load-bearing: `$(...)` strips trailing newlines, so a scope with no
# fields yet — the ordinary case before a teardown is planned — would collapse to fewer lines
# than there are `read`s, and the failed read would abort the run under `set -e`.
{ read -r SCOPE_ORG; read -r SCOPE_BASE; read -r SCOPE_STATE
  read -r SCOPE_GROUP_ID; read -r SCOPE_REGISTRY_REMOVED_AT; } <<EOF
$(printf '%s' "$SCOPE_DOC" \
  | fleet_json "'\n'.join([d.get('org_id') or '', d.get('account_base') or '', d.get('state') or '', d.get('group_id') or '', d.get('registry_removed_at') or ''])"; echo .)
EOF
# THE GROUP ID OUTLIVES THE REGISTRY, deliberately. Step 6 proves the running bridge no longer
# routes this group, and it runs AFTER the registry — the only other record of
# TELEGRAM_GROUP_ID — has been removed, because the point is that a replacement process loaded a
# registry without this tenant. If that check fails the rerun still needs the id, so the receipt
# carries it. It is an opaque routing identifier, not a credential: `lifecycle.check_fields`
# refuses token/secret/password/key/credential/bearer, and this is none of them.
_receipt_fields() {
  printf '%s\n' "org_id=$ORG_ID" "account_base=$ACCOUNT_BASE"
  [ -z "${GROUP_ID:-}" ] || printf '%s\n' "group_id=$GROUP_ID"
  [ -z "${IC_UID:-}" ] || printf '%s\n' "ironclaw_user_id=$IC_UID"
  [ -z "${REGISTRY_REMOVED_AT:-}" ] || printf '%s\n' "registry_removed_at=$REGISTRY_REMOVED_AT"
}
receipt_set() {   # receipt_set <state>
  local state="$1" args=()
  while IFS= read -r line; do args+=("$line"); done < <(_receipt_fields)
  python3 "$LIFECYCLE" teardown set "$SLUG" "$state" "${args[@]}" >/dev/null
}
# The LIVE entry wins over the STAGED one, but they are read identically: a provisioning run
# that never reached activation left an entry that was never servable, yet the org, the member
# and the confinement it created are all real, so it is torn down through exactly the same path.
# Choosing the file first and reading once means a key added to this list cannot be added to one
# branch and forgotten in the other.
if [ -f "$ENV_FILE" ]; then
  RETRY_REGISTRY_FILE="$ENV_FILE"
elif [ -f "$STAGED_FILE" ]; then
  RETRY_REGISTRY_FILE="$STAGED_FILE"
  echo "   (reading a STAGED, never-activated entry: $STAGED_FILE)"
fi
if [ -n "$RETRY_REGISTRY_FILE" ]; then
  # fleet_env_get, not a local sed: this reader was the STRICTEST of the four in the tree and
  # that was load-bearing in the wrong direction. It matched only a bare or double-quoted value
  # anchored to end-of-line, so a single-quoted or whitespace-padded IRONCLAW_USER_ID parsed
  # EMPTY here and nowhere else — step 2 then printed "SKIPPED (need IRONCLAW_USER_ID…)", never
  # set DEGRADED, and the run exited 0 with the sealed account still alive. Same class silently
  # skipped the bridge-state removal (GROUP_ID) and the 401 verification (ACCOUNT_TOKEN).
  REGISTRY_ORG_ID=$(fleet_env_get "$RETRY_REGISTRY_FILE" ORG_ID)
  IC_UID=$(fleet_env_get "$RETRY_REGISTRY_FILE" IRONCLAW_USER_ID)
  GROUP_ID=$(fleet_env_get "$RETRY_REGISTRY_FILE" TELEGRAM_GROUP_ID)
  ACCT_TOKEN=$(fleet_env_get "$RETRY_REGISTRY_FILE" ACCOUNT_TOKEN)
  ACCOUNT_BASE=$(fleet_env_get "$RETRY_REGISTRY_FILE" ACCOUNT_BASE)
  ACCOUNT_BASE="${ACCOUNT_BASE:-$ACCOUNT_BASE_DEFAULT}"
  # Read BEFORE the file is removed in step 1: the post-delete probe needs the member token to
  # ask the only question that settles revocation — does this bearer still authenticate?
  IC_TOKEN=$(fleet_env_get "$RETRY_REGISTRY_FILE" IRONCLAW_TOKEN)
fi
# THE RERUN'S AUTHORITY, when the registry is legitimately gone. Step 6 can fail after step 5 has
# removed the last registry copy — that is not a defect, it is the ordering the check requires —
# and the next run must still know which group to prove absent. The receipt is the only surviving
# record, so it is the fallback and never the override: a present registry always wins, because a
# stale receipt must not be able to redirect anything.
GROUP_ID="${GROUP_ID:-$SCOPE_GROUP_ID}"
REGISTRY_REMOVED_AT="${SCOPE_REGISTRY_REMOVED_AT:-}"
# `ironclaw_user_id` is RECORDED in the receipt as teardown evidence but deliberately NOT read
# back into `IC_UID`. It is only ever needed for the sealed-account deletion, which happens long
# before the registry is removed, so no rerun can need it from here — and `ALREADY_ABSENT` tests
# `-z "$IC_UID"` to decide whether a tenant is fully gone. Restoring it from the receipt made a
# completely deprovisioned tenant look half-present on every rerun, which turned an idempotent
# second run into a BLOCKED one. Only `group_id` has a demonstrated need to outlive the registry.
if [ -n "$ACCT_TOKEN" ]; then
  # This file receives the tenant's AUTHENTICATED account list, so every exit path removes it.
  # INT/TERM must EXIT as well as clean up: a cleanup-only signal trap returns to the script and
  # would let the destructive teardown continue after the operator pressed Ctrl-C or stopped it.
  AUTH_BODY=$(mktemp "${TMPDIR:-/tmp}/ironworks-deprovision-auth.XXXXXX")
  cleanup_auth_body() { rm -f "${AUTH_BODY:-}"; }
  trap cleanup_auth_body EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  AUTH_CODE=$(fleet_http_code curl_header "X-Service-Token: $ACCT_TOKEN" -s --max-time 15 \
    -o "$AUTH_BODY" -w '%{http_code}' "$ACCOUNT_BASE/list_accounts" 2>/dev/null)
  AUTH_DOC=$(cat "$AUTH_BODY")
  cleanup_auth_body
  AUTH_BODY=""
  # The sensitive temporary file is gone. Restore the shell's default signal behaviour rather
  # than leaving handlers installed across the later destructive steps.
  trap - EXIT INT TERM
  if [ "$AUTH_CODE" = "200" ]; then
    ORG_ID=$(printf '%s' "$AUTH_DOC" | fleet_json "d.get('org') or ''") || {
      echo "!! authenticated Account Service response was unreadable; refusing deletion." >&2
      exit 1
    }
    # Authentication is evidence for this invocation. Persist it only once the operator has
    # explicitly authorized execution; a dry-run may observe remote scope but must not create
    # lifecycle state merely by looking.
    [ "$EXECUTE" -ne 1 ] || receipt_set authenticated
    SCOPE_STATE="authenticated"; SCOPE_ORG="$ORG_ID"; SCOPE_BASE="$ACCOUNT_BASE"
  elif [ "$AUTH_CODE" = "401" ] && [ -n "$SCOPE_ORG" ] \
       && { [ "$SCOPE_STATE" = "authenticated" ] || [ "$SCOPE_STATE" = "account_revoked" ] \
            || [ "$SCOPE_STATE" = "complete" ]; }; then
    # Expected after an interrupted run removed the remote identity but deliberately retained
    # the registry. The prior authenticated receipt supplies scope; this 401 re-proves revocation.
    ORG_ID="$SCOPE_ORG"
    ACCOUNT_BASE="${SCOPE_BASE:-$ACCOUNT_BASE}"
  else
    echo "!! cannot authenticate deletion scope for '$SLUG' at its configured Account Service (HTTP $AUTH_CODE)." >&2
    echo "   No inventory, identity removal, row deletion, or file removal was attempted." >&2
    exit 1
  fi
elif [ -n "$SCOPE_ORG" ] && { [ "$SCOPE_STATE" = "account_revoked" ] || [ "$SCOPE_STATE" = "complete" ]; }; then
  # The registry is removed only at the final checkpoint. Reaching this path means that final
  # removal completed and the non-secret receipt is now the supported idempotent-rerun evidence.
  ORG_ID="$SCOPE_ORG"
  ACCOUNT_BASE="${SCOPE_BASE:-$ACCOUNT_BASE}"
else
  echo "!! cannot establish deletion scope for '$SLUG': no authenticating ACCOUNT_TOKEN and no" >&2
  echo "   prior authenticated deprovision receipt. Registry ORG_ID is never a fallback." >&2
  echo "   Restore the tenant registry env from its protected backup and rerun; without that" >&2
  echo "   credential, remaining identity/data authority must be reconciled manually." >&2
  exit 1
fi
case "$ORG_ID" in (*[!a-zA-Z0-9._-]*|"")
  echo "!! authenticated Account Service returned no usable organization; refusing deletion." >&2
  exit 1 ;;
esac
if [ -n "$REGISTRY_ORG_ID" ] && [ "$REGISTRY_ORG_ID" != "$ORG_ID" ]; then
  echo "   WARNING: registry ORG_ID '$REGISTRY_ORG_ID' is stale; authenticated scope is '$ORG_ID'." >&2
  echo "   Deletion is bound only to the authenticated organization." >&2
fi

# Resolve infrastructure only AFTER authentication, so an unverified identity cannot reach even
# inventory SQL, much less a destructive statement.
DB_CONT="$(fleet_account_db_container)"   # env > compose default > live lookup; errors, never ""

# ONE psql for all four tables, not one per table: each `docker exec … psql` is a container
# exec plus a Postgres connect and teardown, and this ran eight of them per deprovision (four
# for the inventory, four again to verify convergence) for four integers from one database.
# Column order here is the order the callers `read` them in.
sql_counts() { # -> four lines: organizations, accounts, contacts, activities
  echo "SELECT (SELECT count(*) FROM organizations WHERE org_id = :'org'),
               (SELECT count(*) FROM accounts      WHERE org_id = :'org'),
               (SELECT count(*) FROM contacts      WHERE org_id = :'org'),
               (SELECT count(*) FROM activities    WHERE org_id = :'org');" \
  | docker exec -i "$DB_CONT" psql -qtA -F $'\n' -U postgres -d accounts -v org="$ORG_ID"
}

echo "== deprovision inventory for client '$SLUG' (org '$ORG_ID') =="
# `|| true` on the reads: a psql that exits 0 but returns short output must leave the counts
# EMPTY, not abort. Empty is the fail-closed answer here — it fails the `!= 0` convergence check
# below, which sets DEGRADED and retains the registry for retry. Aborting mid-teardown would
# skip that.
#
# BUT THE STATUS OF PSQL ITSELF HAS TO BE SEEN. This was `< <(sql_counts) || true`, a process
# substitution: the `|| true` applied to `read`, and sql_counts' exit status was never observed
# at all. So "the database is unreachable" and "the counts are genuinely zero" were the same
# event to everything downstream, and the run proceeded into the irreversible steps on an
# inventory that had measured nothing. Captured first, status checked, THEN split into lines.
COUNTS_RC=0
COUNTS_OUT="$(sql_counts)" || COUNTS_RC=$?
{ read -r O; read -r A; read -r C; read -r T; } <<COUNTS || true
$COUNTS_OUT
COUNTS
if [ "$COUNTS_RC" -ne 0 ]; then
  echo "!! could not read the Account Store inventory (psql exit $COUNTS_RC)." >&2
  echo "   Nothing about this tenant's rows was measured, so this run cannot claim to have" >&2
  echo "   removed them. Fix the database connection and re-run." >&2
  exit 1
fi
# Through the one identity module, like every other reader and writer of this map. It also
# stops interpolating $IDENTITIES_FILE and $ORG_ID into Python source, which the shape above did.
TOK_REG=$(ACCOUNT_IDENTITIES_FILE="$IDENTITIES_FILE" python3 "$IDENTITIES" count "$ORG_ID")
# The bridge's state moved from a JSON file to one transactional store
# (multi/seam/bridge_state.py), so the conversation pointer and the update journal cannot
# disagree after a crash. Read it through that module rather than re-implementing the shape.
BR=0 BR_READ_RC=0
if [ -n "$GROUP_ID" ] && [ -f "$BRIDGE_STATE_DB" ]; then
  BR=$(SEAM="$FLEET_REPO_ROOT/multi/seam" DB="$BRIDGE_STATE_DB" GID="$GROUP_ID" python3 -c "
import os, sys, pathlib
sys.path.insert(0, os.environ['SEAM'])
import bridge_state as bs
p = pathlib.Path(os.environ['DB'])
print(1 if bs.inspect_thread_exists(p, os.environ['GID']) else 0)" 2>/dev/null) || BR_READ_RC=$?
  if [ "$BR_READ_RC" -ne 0 ]; then BR="unmeasured"; fi
fi
printf '   account-store rows: organizations=%s accounts=%s contacts=%s activities=%s\n' "$O" "$A" "$C" "$T"
printf '   identity tokens registered for org: %s\n' "$TOK_REG"
printf '   registry env: %s · guidance: %s\n' "$([ -f "$ENV_FILE" ] && echo present || echo absent)" "$([ -f "$GUIDE_FILE" ] && echo present || echo absent)"
printf '   bridge thread-state entry: %s · sealed ironclaw user id: %s\n' "$BR" "${IC_UID:-unknown}"
printf '   staging: dir %s · env %s\n' "$([ -d "$DATA_DIR" ] && echo present || echo absent)" "$([ -f "$DATA_ENV" ] && echo present || echo absent)"
echo "   note: backups are NOT rewritten (they expire on schedule); other-machine staging is NOT touched"

# IDEMPOTENCE. Re-running a deprovision must CONVERGE on the removed state, not report a
# failure for work that is already done. "Nothing was deleted because nothing was there" and
# "nothing was deleted because the delete failed" are different events, and only the second is
# degraded — so the distinction is drawn HERE, from the inventory, before anything runs.
ALREADY_ABSENT=0
if [ ! -f "$ENV_FILE" ] && [ ! -f "$STAGED_FILE" ] && [ ! -f "$GUIDE_FILE" ] \
   && [ "$TOK_REG" = "0" ] && [ "$O" = "0" ] && [ "$A" = "0" ] && [ "$C" = "0" ] && [ "$T" = "0" ] \
   && [ -z "$IC_UID" ]; then
  ALREADY_ABSENT=1
fi

if [ "$EXECUTE" -ne 1 ]; then
  echo
  if [ "$BR_READ_RC" -ne 0 ]; then
    echo "!! DRY RUN BLOCKED — bridge state could not be inspected without mutation." >&2
    echo "   Nothing was changed, but route inventory is unmeasured." >&2
    exit 1
  fi
  echo "DRY RUN ONLY — nothing deleted. Re-run with: --execute --confirm $SLUG"
  exit 0
fi
[ "$CONFIRM" = "$SLUG" ] || { echo "!! --confirm must repeat the slug exactly ('$CONFIRM' != '$SLUG') — aborting" >&2; exit 2; }

echo; echo "== EXECUTING deletion for '$SLUG' =="
DEGRADED=0     # any step that could not fully complete flips this; summarized at the end
echo "-- 1/6 revoke the Account-Service identity, then PROVE the old token is refused"
RM_FILES=0
# The `-f` guard is gone: an absent map is zero to deregister, which the module already treats
# as the desired end state. A CORRUPT map is now refused here too — this path used to
# `json.load(open(path))` bare, so it was the copy that would traceback where its twin in
# provision.sh stopped cleanly, and neither wrote the same bytes as register-identity.sh.
_dereg=$(ACCOUNT_IDENTITIES_FILE="$IDENTITIES_FILE" python3 "$IDENTITIES" remove "$ORG_ID")
echo "   deregistered $_dereg org token(s) (hot-reloaded, effective now)"

if [ -n "$ACCT_TOKEN" ]; then
  hc=$(fleet_http_code curl_header "X-Service-Token: $ACCT_TOKEN" \
    -s -o /dev/null -w '%{http_code}' "$ACCOUNT_BASE/list_accounts")
  if [ "$hc" != "401" ]; then
    echo "!! Account-token revocation UNVERIFIED: old token returned HTTP $hc, expected 401." >&2
    echo "   Registry and credential retained for a safe retry; no success will be reported." >&2
    exit 1
  fi
  echo "   VERIFIED REVOKED: old Account token is refused (HTTP 401)."
elif [ "$SCOPE_STATE" != "account_revoked" ] && [ "$SCOPE_STATE" != "complete" ]; then
  echo "!! Account-token revocation has no retained proof; refusing to continue." >&2
  exit 1
fi
receipt_set account_revoked
SCOPE_STATE="account_revoked"

echo "-- 2/6 delete the sealed IronClaw account, then PROBE whether its token still works"
REVOCATION="BLOCKED"          # VERIFIED_REVOKED | RESIDUAL | BLOCKED
if [ -n "$IC_UID" ] && [ -n "${WEBUI_TOKEN:-}" ]; then
  # Through the fleet helper, which provision.sh's compensator also uses — the two teardown
  # paths must agree on what "deleted" means. It also carries the `|| echo 000` this call was
  # missing: without `-f`, curl exits non-zero when the request never LEFT, and under
  # `set -euo pipefail` that aborted deprovisioning right here — after step 1 had already
  # deregistered the org token, leaving a half-torn-down tenant and no audit line saying so.
  code=$(fleet_delete_member "$WEBUI_TOKEN" "$IRONCLAW_API" "$IC_UID")
  echo "   DELETE admin/users -> HTTP $code"
  if ! fleet_member_is_gone "$code"; then
    if [ "$code" = 000 ]; then why="the request never reached the instance"
    else why="stale/ambient WEBUI_TOKEN?"; fi
    echo "   !! sealed account NOT deleted (HTTP $code — $why); continuing, but DEGRADED" >&2
    DEGRADED=1
  fi
elif [ "$ALREADY_ABSENT" = 1 ]; then
  echo "   nothing to delete — no registry record, no rows, no identity token: already removed."
else
  # To stderr, like every other degradation in this script: this branch sets DEGRADED, and a
  # `deprovision.sh >log` that hid the one line saying a member token is still live would be
  # reporting a clean teardown it did not perform.
  echo "   SKIPPED (need IRONCLAW_USER_ID in registry + WEBUI_TOKEN env). NOTE: the org (Account" >&2
  echo "   Service) token IS revoked by step 1, but the IronClaw MEMBER token is not — it stays valid" >&2
  echo "   until expiry regardless. Delete the user record manually." >&2
  DEGRADED=1
fi

# THE PROBE. Deleting the record is not the question; whether the token still opens the door
# is. `GET /v1/responses/<id-that-does-not-exist>` costs no model call and separates the two
# answers cleanly: 401/403 means the bearer was refused, anything else means it was accepted
# and the id merely does not exist. A garbage bearer is probed first as the negative control,
# so "ACCEPTED" cannot be an artifact of a route that answers 404 for everyone.
# Full limitation and current controls: SECURITY.md.
if [ -n "$IC_TOKEN" ]; then
  PROBE="/v1/responses/resp_00000000000000000000000000000000"
  ctl=$(fleet_http_code curl_bearer "not-a-real-token-000000000000" \
    -s -o /dev/null -w '%{http_code}' "$IRONCLAW_API$PROBE")
  live=$(fleet_http_code curl_bearer "$IC_TOKEN" \
    -s -o /dev/null -w '%{http_code}' "$IRONCLAW_API$PROBE")
  if [ "$ctl" = "000" ] || [ "$live" = "000" ]; then
    REVOCATION="BLOCKED"
    echo "   !! REVOCATION UNVERIFIED — the instance did not answer the probe (control=$ctl, member=$live)." >&2
  elif [ "$ctl" != "401" ] && [ "$ctl" != "403" ]; then
    REVOCATION="BLOCKED"
    echo "   !! REVOCATION UNVERIFIED — the negative control was ACCEPTED (HTTP $ctl), so this" >&2
    echo "      probe cannot tell an accepted bearer from a rejected one. Do not read the result." >&2
  elif [ "$live" = "401" ] || [ "$live" = "403" ]; then
    REVOCATION="VERIFIED_REVOKED"
    echo "   VERIFIED REVOKED: the former member token is refused (HTTP $live)."
  else
    REVOCATION="RESIDUAL"
    echo "   RESIDUAL AUTHORITY: the former member token STILL AUTHENTICATES (HTTP $live)."
  fi
elif [ "$ALREADY_ABSENT" = 1 ]; then
  # A converged re-run has no token to probe, and that is not a blocked probe — the answer is
  # whatever the earlier run recorded. An outstanding ledger entry keeps this non-zero until
  # the session actually expires; an absent one means there is nothing left to report.
  if python3 "$LIFECYCLE" residual has "$SLUG"; then
    REVOCATION="RESIDUAL"
    echo "   already removed; the ledger still shows an UNEXPIRED member session for this slug."
  else
    REVOCATION="VERIFIED_REVOKED"
    echo "   already removed, and no unexpired member session is on record."
  fi
else
  echo "   !! no member token on record — cannot probe revocation (registry entry missing or incomplete)." >&2
fi

case "$REVOCATION" in
  VERIFIED_REVOKED)
    python3 "$LIFECYCLE" residual drop "$SLUG" >/dev/null || true ;;
  RESIDUAL)
    # Recorded, with its expiry and NO token material, so "are any credentials awaiting
    # revocation?" has an answer between now and then. `ironworks doctor` fails while any
    # entry is outstanding. A converged RE-RUN must not re-stamp the entry: re-adding would
    # push the recorded expiry a further year out every time someone ran the script, which
    # turns an audit record into a moving target.
    if [ "$ALREADY_ABSENT" != 1 ]; then
      # A FAILED LEDGER WRITE MUST NOT EXIT 3. `|| true` swallowed it, and exit 3 is defined at
      # the top of this file as "recorded in the residual-authority ledger with its expiry" —
      # so a run whose write failed still claimed the record existed, for the one token whose
      # whole point is that nothing else remembers it. `ironworks doctor` fails while an entry
      # is outstanding; no entry means it passes, and the session goes on authenticating with
      # nothing tracking it. That is the exact silence the ledger was built to end.
      #
      # DEGRADED=1, not an abort: the teardown itself succeeded and the remaining steps must
      # still run. The `if [ "$DEGRADED" = 1 ]` block below already exits 1 BEFORE the
      # REVOCATION case is reached, and 1 is this file's own code for "a revocation step did
      # not complete" — so the honest exit falls out of the vocabulary already here.
      if ! python3 "$LIFECYCLE" residual add "$SLUG" "uid=${IC_UID:-unknown}" \
           "lifetime_days=${SESSION_LIFETIME_DAYS:-365}" "org_id=$ORG_ID" >&2; then
        echo "   !! the residual-authority ledger entry could NOT be written for '$SLUG'." >&2
        echo "      The member session STILL AUTHENTICATES and nothing is now tracking it." >&2
        echo "      Record it by hand before this terminal is gone:" >&2
        echo "        python3 $LIFECYCLE residual add $SLUG uid=${IC_UID:-unknown} \\" >&2
        echo "          lifetime_days=${SESSION_LIFETIME_DAYS:-365} org_id=$ORG_ID" >&2
        DEGRADED=1
      fi
    fi
    echo "   The containment is CUSTODY (the token never left the seam) plus the global" >&2
    echo "   rotation in deploy/README.md. This DELETE did not end their access." >&2 ;;
esac

echo "-- 3/6 delete Account-Store rows in one transaction"
# THE ONLY IRREVERSIBLE STEP THAT WAS NOT HARDENED. Bare, this psql aborts the script under
# `set -e` — after step 1 has deregistered the org token and step 2 has deleted the sealed
# member. No AUDIT line, no `!! DEGRADED` banner, no residual-authority entry for a member
# token that still authenticates, and an exit status (psql's, commonly 2) that this script's
# own header documents as "usage error". The transaction is atomic, so a failure here leaves
# the rows intact and a retry is safe; what must not happen is the run ending silently.
# Same rationale as `fleet_delete_member` (deploy/lib/fleet.sh:91-99).
if ! docker exec -i "$DB_CONT" psql -q -U postgres -d accounts -v org="$ORG_ID" <<'SQL'
BEGIN;
DELETE FROM activities    WHERE org_id = :'org';
DELETE FROM contacts      WHERE org_id = :'org';
DELETE FROM accounts      WHERE org_id = :'org';
DELETE FROM organizations WHERE org_id = :'org';
COMMIT;
SQL
then
  echo "  !! the row-deletion transaction FAILED — this tenant's account data is still in the" >&2
  echo "     Account Store. It is atomic, so nothing was half-deleted; re-run this script." >&2
  DEGRADED=1
fi

echo "-- 4/6 remove operator-curated staging material on THIS machine"
# Same rule as step 1 above, for the same reason: an `&&` chain makes a FAILED rm invisible —
# set -e ignores it, RM_STAGE does not increment, and the audit line below then reports
# `staging=0` as though there had been nothing to remove. The staged tree holds this tenant's
# account data; "could not delete it" and "there was none" must not print identically.
RM_STAGE=0
for p in "$DATA_DIR" "$DATA_ENV"; do
  [ -e "$p" ] || continue                          # absent = already gone (fine)
  if rm -rf "$p"; then RM_STAGE=$((RM_STAGE+1)); else
    echo "  !! could NOT remove $p — this tenant's staged account data is still on this machine; remove it by hand" >&2
    DEGRADED=1
  fi
done

echo; echo "== post-delete verification =="
# Same shape as the inventory above, same reason: an unreadable database must not be able to
# produce four empty strings that then read as "did not converge" for the wrong cause, nor as
# convergence for any cause at all.
VERIFY_RC=0
VERIFY_OUT="$(sql_counts)" || VERIFY_RC=$?
{ read -r O2; read -r A2; read -r C2; read -r T2; } <<VERIFY || true
$VERIFY_OUT
VERIFY
if [ "$VERIFY_RC" -ne 0 ]; then
  echo "!! could not re-read the Account Store to verify the deletion (psql exit $VERIFY_RC)." >&2
  DEGRADED=1
fi
printf '   rows now: organizations=%s accounts=%s contacts=%s activities=%s (want all 0)\n' "$O2" "$A2" "$C2" "$T2"
if [ "$O2" != 0 ] || [ "$A2" != 0 ] || [ "$C2" != 0 ] || [ "$T2" != 0 ]; then
  echo "!! row deletion did not converge to zero; retaining registry/guidance for retry." >&2
  DEGRADED=1
fi

echo "-- 5/6 remove routing and local credentials LAST"
BR_REMOVED=0            # what step 5 actually removed, for the AUDIT line — never the inventory
if [ "$DEGRADED" = 0 ] && [ "$REVOCATION" != "BLOCKED" ]; then
  if [ -n "$GROUP_ID" ] && [ -f "$BRIDGE_STATE_DB" ]; then
    # `$( … || echo 0 )` MADE A FAILED DELETE INDISTINGUISHABLE FROM AN EMPTY ONE. A store that
    # could not be opened — a lock, a corrupt page, a partially-applied migration — printed
    # "removed 0", set nothing, and teardown carried on to delete the registry: the only record
    # of TELEGRAM_GROUP_ID, and therefore the only way to retry. The run then exited 0 with the
    # group's conversation pointer and its journal still on disk. Capture the status instead,
    # and let the failure reach DEGRADED like every other one in this script.
    BR_RC=0
    BR_OUT=$(SEAM="$FLEET_REPO_ROOT/multi/seam" DB="$BRIDGE_STATE_DB" GID="$GROUP_ID" python3 -c "
import os, sys
sys.path.insert(0, os.environ['SEAM'])
import bridge_state as bs
gid = os.environ['GID']
st = bs.BridgeState(os.environ['DB'])
n = st.drop_thread(gid)
# CONVERGENCE, NOT A RETURN VALUE. 'completed teardown' has to mean the group cannot still be
# routed from durable state, so the row is re-read after the delete rather than inferred from a
# count. (The in-memory half is proved separately, in step 6.)
left = st.thread_row(gid)
st.close()
if left is not None:
    sys.stderr.write('thread row for %s survived the delete\n' % gid)
    raise SystemExit(1)
print(n)") || BR_RC=$?
    if [ "$BR_RC" -ne 0 ]; then
      echo "  !! could NOT remove the bridge thread record for group $GROUP_ID from" >&2
      echo "     $BRIDGE_STATE_DB — that group's conversation pointer and delivery journal are" >&2
      echo "     still there, so it remains routable from durable state. Retaining the registry" >&2
      echo "     so the group id survives for a retry; fix the store and re-run." >&2
      DEGRADED=1
    else
      BR_REMOVED="$BR_OUT"
      echo "   removed $BR_REMOVED bridge thread record(s) for that group"
    fi
  fi
fi

# RE-CHECKED, because the bridge step above can degrade the run. Local credentials are removed
# LAST and only when everything before them converged — otherwise one failed removal followed by
# one successful one destroys the inputs the next run needs.
if [ "$DEGRADED" = 0 ] && [ "$REVOCATION" != "BLOCKED" ]; then
  # Guidance and any redundant registry copy go first. The file we read credentials and
  # identifiers from is the retry authority for this run and is removed only after every
  # preceding local cleanup has succeeded. Otherwise one failed rm followed by one successful
  # rm can destroy the inputs needed to converge on the next run.
  for f in "$GUIDE_FILE" "$STAGED_FILE" "$ENV_FILE"; do
    [ "$f" != "$RETRY_REGISTRY_FILE" ] || continue
    [ -f "$f" ] || continue
    if rm -f "$f"; then RM_FILES=$((RM_FILES+1)); else
      echo "  !! could NOT remove $f — local teardown is incomplete" >&2; DEGRADED=1
    fi
  done
  if [ "$DEGRADED" = 0 ] && [ -n "$RETRY_REGISTRY_FILE" ] \
     && [ -f "$RETRY_REGISTRY_FILE" ]; then
    if rm -f "$RETRY_REGISTRY_FILE"; then RM_FILES=$((RM_FILES+1)); else
      echo "  !! could NOT remove $RETRY_REGISTRY_FILE — local teardown is incomplete" >&2
      DEGRADED=1
    fi
  elif [ "$DEGRADED" != 0 ] && [ -n "$RETRY_REGISTRY_FILE" ] \
       && [ -f "$RETRY_REGISTRY_FILE" ]; then
    echo "   retained registry retry authority at $RETRY_REGISTRY_FILE until local cleanup converges." >&2
  fi
  # The instant the last registry copy went, recorded BEFORE step 6 needs it. Step 6 asks whether
  # the running bridge started after this moment; a rerun reads it back from the receipt.
  if [ "$DEGRADED" = 0 ] && [ -z "${REGISTRY_REMOVED_AT:-}" ] \
     && [ ! -f "$ENV_FILE" ] && [ ! -f "$STAGED_FILE" ]; then
    REGISTRY_REMOVED_AT="$(date -u +%s)"
    receipt_set account_revoked
  fi
else
  echo "   retained registry/guidance so the failed or blocked teardown can be retried safely." >&2
fi

echo "-- 6/6 prove the group is no longer routable, in memory as well as on disk"
# SUCCESS MEANS ALL THREE LAYERS ARE GONE: no registry route, no durable row, and no route in the
# process that is serving right now. The third was previously a sentence in REMAINING BY DESIGN
# telling the operator to restart the bridge — which made "restart it" the lifecycle contract
# rather than an implementation detail, and left a successful-looking deprovision whose group was
# still dispatched by the running process.
#
# `telegram_bridge.main()` calls `load_groups()` ONCE at startup and holds the dict for the life
# of the process: no per-group drop, no reload path, no signal. So the only thing that retires an
# in-memory route is a replacement process — and the honest check is whether one has started
# SINCE the registry stopped naming this tenant. This script does not restart anything: the
# bridge serves every tenant from one process, and retiring one client must not interrupt the
# others on this script's initiative.
ROUTE_STATE="unknown"
if [ "$DEGRADED" = 0 ] && [ "$REVOCATION" != "BLOCKED" ]; then
  ROUTE_DOC=$(python3 "$ROUTE_AUTHORITY" --db "$BRIDGE_STATE_DB" \
    --registry-removed-at "${REGISTRY_REMOVED_AT:-}" \
    --seam-dir "$FLEET_REPO_ROOT/multi/seam")
  ROUTE_RESULT=$(printf '%s' "$ROUTE_DOC" | fleet_json "d.get('state') or 'UNKNOWN'")
  ROUTE_REASON=$(printf '%s' "$ROUTE_DOC" | fleet_json "d.get('reason') or 'no reason reported'")
  if [ "$ROUTE_RESULT" = "ABSENT" ]; then
    ROUTE_STATE="absent"
    echo "   route absence positively established: $ROUTE_REASON"
  elif [ "$ROUTE_RESULT" = "PRESENT" ]; then
    ROUTE_STATE="present"; DEGRADED=1
    echo "  !! the currently serving bridge still holds the group in memory:" >&2
    echo "     $ROUTE_REASON. Restart the bridge, then re-run this script to converge:" >&2
    echo "       systemctl restart bridge           # the unit this repository ships" >&2
    echo "     The teardown receipt retains group_id, so the rerun needs no registry." >&2
  else
    ROUTE_STATE="unknown"; DEGRADED=1
    echo "  !! in-memory route absence is UNMEASURED: $ROUTE_REASON" >&2
    echo "     Ambiguous service/process state cannot complete deprovisioning." >&2
  fi
else
  echo "   skipped: earlier steps did not converge, so the registry still routes this group." >&2
fi
echo
# THE EXIT CODE IS THE CLAIM, and so is this line. `$O`/`$A`/`$C`/`$T` and `$BR` are the
# INVENTORY — what was there before anything ran — so a degraded run that skipped step 5
# entirely still printed `bridge entry removed: 1`, and a run whose row deletion did not
# converge still printed `deleted rows org=1`, one line above a banner saying the opposite.
# Report what the post-delete verification and step 5 actually observed. `$BR_REMOVED` is set
# by step 5; on any path that skips it, it stays 0 because nothing was removed.
_rows_gone() { # inventory minus what is still there, per table
  # THIS FUNCTION MAY NOT ABORT. It runs after every irreversible step, so a failure here
  # reproduces exactly what step 3 was hardened against: no AUDIT line and no `!! DEGRADED`
  # banner, after the org token was deregistered and the sealed member deleted. `$(( ))` on a
  # non-numeric word is a HARD ERROR under `set -u` — bash resolves it as a variable name — and
  # psql can put a notice where an integer was expected. Empty is safe (it evaluates to 0), so
  # the digit test is what actually needs making, and it subsumes the exit-status test below.
  local v
  for v in "$O" "$A" "$C" "$T" "$O2" "$A2" "$C2" "$T2"; do
    case "$v" in (''|*[!0-9]*) echo "unmeasured"; return 0 ;; esac
  done
  if [ "$COUNTS_RC" -ne 0 ] || [ "$VERIFY_RC" -ne 0 ]; then echo "unmeasured"; return 0; fi
  printf 'org=%s acct=%s contact=%s activity=%s' \
    "$((O - O2))" "$((A - A2))" "$((C - C2))" "$((T - T2))"
}
echo "AUDIT '$SLUG': deleted rows $(_rows_gone) · files removed: registry+guidance=$RM_FILES staging=$RM_STAGE · tokens deregistered: $TOK_REG · bridge entry removed: ${BR_REMOVED:-0} · in-memory route: $ROUTE_STATE"
echo "REMAINING BY DESIGN: backup copies until scheduled expiry; journald service logs; the other machine's staging (run this script there)."
# The provisioning journal is state about a tenant that no longer exists.
python3 "$LIFECYCLE" journal clear "$SLUG" >/dev/null || true

if [ "$DEGRADED" = 1 ]; then
  echo "!! DEGRADED: at least one revocation step did not fully complete (see the '!!' line(s) above)." >&2
  echo "   - registry file not removed -> the client may still be routed (remove it by hand)." >&2
  echo "   - sealed account not deleted -> re-run step 2 with a current operator token:" >&2
  echo "     curl -X DELETE -H \"Authorization: Bearer \$WEBUI_TOKEN\" $IRONCLAW_API/api/webchat/v2/admin/users/$IC_UID" >&2
  exit 1
fi

# THE EXIT CODE IS THE CLAIM. A deprovision that removed every row and file but left a token
# that still opens the door has not ended access, and must not report that it has. Rerunning
# converges from the authenticated teardown receipt: already-removed rows/identities are normal,
# and the ledger entry is reused rather than duplicated.
case "$REVOCATION" in
  VERIFIED_REVOKED)
    receipt_set complete
    echo "✅ '$SLUG' deprovisioned and the member session is VERIFIED REVOKED."
    exit 0 ;;
  RESIDUAL)
    receipt_set complete
    echo "⚠️  '$SLUG' deprovisioned, but its member session STILL AUTHENTICATES." >&2
    echo "   Recorded in the residual-authority ledger with its expiry:" >&2
    python3 "$LIFECYCLE" residual list >&2 || true
    echo "   Read this as the pinned runtime's known limit, not as a failure of this run —" >&2
    echo "   SECURITY.md. If the token may have LEAKED, do not wait for expiry:" >&2
    echo "   follow the global rotation in deploy/README.md." >&2
    exit 3 ;;
  *)
    echo "⚠️  '$SLUG' deprovisioned, but revocation could NOT BE ESTABLISHED (probe blocked)." >&2
    echo "   This is not a success. Re-run the probe when the instance is reachable:" >&2
    echo "     WEBUI_TOKEN=... python3 multi/verify/test_session_revocation.py" >&2
    exit 4 ;;
esac
