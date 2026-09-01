#!/usr/bin/env bash
# Composed tenant provisioning — a resumable, compensating state machine.
#
#   tenant = (Account-Service org + org token)              — their private data scope
#          + (sealed IronClaw member account + token)       — their private agent scope
#          + (a confined tool surface)                      — no egress, read-only
#          + (Telegram group id)                            — their channel
#          + (~/.agency/clients/<slug>.env)                — the seam/bridge registration
#
# WHY A STATE MACHINE. Provisioning creates authority in four systems. It used to run as a
# straight line that wrote the LIVE registry entry in step 3 and ran the isolation smoke test
# in step 4 — so a tenant that failed its own cross-org check was already routable, and a
# failure anywhere left the operator reading a terminal to work out what existed. The script's
# own error text admitted the gap: "Re-running mints a NEW org token and leaves the old entry
# registered." Three things follow from that, and they are the design:
#
#   1. EVERY CHECK THAT CAN BE MADE BEFORE CREATING AUTHORITY IS MADE FIRST (preflight).
#   2. THE LIVE REGISTRY ENTRY IS WRITTEN LAST, after the security and smoke gates pass. Until
#      then it sits in CLIENTS_DIR/.staging/, which the seam's `*.env` glob does not match, so
#      a half-provisioned tenant is not servable even for an instant.
#   3. A FAILURE COMPENSATES WHAT IT CREATED and, for anything it could not undo, says so and
#      exits non-zero. "Residual authority" is a reported state, never a silent one.
#
# Usage:
#   IRONCLAW_API=http://127.0.0.1:3020 \
#   IRONCLAW_OPERATOR_TOKEN=<operator token> \
#   ./provision.sh <slug> "<Display Name>" <telegram_group_id> [--service <name>]
#
#   ./provision.sh <slug> "<Name>" <gid> --dry-run   preflight only; creates nothing
#   ./provision.sh <slug> --status                   what exists for this slug right now
#   ./provision.sh <slug> "<Name>" <gid> --resume    continue an interrupted run
#
# Re-running without --resume against a slug with an unfinished journal is REFUSED, because a
# blind re-run is exactly what mints a second org token beside the first.
#
# MODEL_PIN: the model of record (repo root) — the smoke turn runs on the same model the
# product runs, or the smoke proves a path production never takes.
#
# Optional data: put candidate-shaped *.json under ~/.agency/account-data/<slug>/ BEFORE
# running and it is seeded into the new org (via seed-real.sh); with no data dir the org
# starts empty (valid — org existence is row scoping).
#
# The Telegram group itself stays manual (create private group, add the bot as admin, get the
# id — procedure in multi/README.md), as does restarting the bridge.
#
# Org tokens are HOT-RELOADED by the Account Service from ~/.agency/account-identities/
# identities.json (ACCOUNT_IDENTITIES_FILE) — provisioning never restarts the data layer,
# so current clients see zero interruption.
set -euo pipefail
cd "$(dirname "$0")"
. ../../deploy/lib/fleet.sh   # curl_header/curl_bearer (tokens off argv) + fleet_* helpers
# The Account-Store smoke ASSERTIONS, shared with prod-up.sh / dev-up.sh / seed-real.sh.
# `SMOKE_BASE` is bound below, once `ACCOUNT_BASE` is resolved.
. ../../deploy/account-intel/data/smoke.sh

REPO_ROOT="$FLEET_REPO_ROOT"
LIFECYCLE="$REPO_ROOT/deploy/lib/lifecycle.py"
IDENTITIES="$REPO_ROOT/deploy/lib/identities.py"   # AUTHORITATIVE Account-Service identity state
journal() { python3 "$LIFECYCLE" journal "$@"; }

# ── arguments ─────────────────────────────────────────────────────────────────────────
SLUG="" NAME="" GROUP_ID="" SERVICE=""
DRY_RUN=0 RESUME=0 STATUS=0
POSITIONAL=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --resume)  RESUME=1 ;;
    --status)  STATUS=1 ;;
    --service) SERVICE="${2:?--service needs a name}"; shift ;;
    # A Telegram group id is a NEGATIVE number, so `-1009...` must stay positional. Only a
    # leading `-` followed by a non-digit is a flag; anything else is an argument.
    -[!0-9]*) echo "!! unknown flag: $1" >&2; exit 2 ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done
SLUG="${POSITIONAL[0]:-}"
NAME="${POSITIONAL[1]:-}"
GROUP_ID="${POSITIONAL[2]:-}"
[ -n "$SLUG" ] || { echo "usage: provision.sh <slug> \"<Display Name>\" <telegram_group_id> [--service <name>] [--dry-run|--resume|--status]" >&2; exit 2; }
fleet_slug_valid "$SLUG" || { echo "!! slug must be lowercase [a-z0-9-]: $SLUG" >&2; exit 2; }

CLIENTS_DIR="${CLIENTS_DIR:-$FLEET_AGENCY_DIR/clients}"
STAGING_DIR="$CLIENTS_DIR/.staging"
ENV_FILE="$CLIENTS_DIR/$SLUG.env"
STAGED_FILE="$STAGING_DIR/$SLUG.env"
CANONICAL_GUIDANCE_FILE="$CLIENTS_DIR/$SLUG.guidance.md"
GUIDANCE_FILE_OVERRIDE="${GUIDANCE_FILE:-}"
GUIDANCE_FILE="${GUIDANCE_FILE_OVERRIDE:-$CANONICAL_GUIDANCE_FILE}"
ACCOUNT_BASE="${ACCOUNT_BASE:-http://127.0.0.1:8443}"
# shellcheck disable=SC2034  # read by the sourced smoke.sh, which shellcheck does not follow
SMOKE_BASE="$ACCOUNT_BASE"   # smoke.sh asserts against the base this run provisions into
# The default service is the SEAM's to declare (`services.DEFAULT_SERVICE`: "the service every
# tenant gets unless it says otherwise"). A copy of the name here writes it into every registry
# entry, so changing the default upstream would go on minting tenants pinned to the old one with
# nothing comparing the two.
SERVICE="${SERVICE:-$(PYTHONPATH="$(cd ../seam && pwd)" python3 -c \
  'import services; print(services.DEFAULT_SERVICE)')}"
# The smoke turn below posts to the same `/v1/responses` the product does, so it sends the same
# User-Agent — read from the seam by the line above's idiom, not spelled again here. It was a
# bare "Mozilla/5.0", the fifth spelling in the tree; `responses.py` records why one owner
# exists and what was measured (the instance is UA-blind; the header is for a hosted edge that
# 1010-blocks non-browser agents).
BROWSER_UA="$(PYTHONPATH="$(cd ../seam && pwd)" python3 -c \
  'import responses; print(responses.BROWSER_UA)')"
CONT="$(fleet_account_service_container)"   # one resolver; ACCOUNT_SERVICE_CONTAINER still wins
DATA_DIR_ROOT="$FLEET_AGENCY_DIR/account-data"
ORG_ID="$SLUG"

# ── --status: what exists for this slug, right now ────────────────────────────────────
if [ "$STATUS" -eq 1 ]; then
  echo "== provisioning status for '$SLUG' =="
  stage="$(journal stage "$SLUG")"
  echo "   journal stage        : ${stage:-<none — never started, or cleared after activation>}"
  echo "   live registry entry  : $([ -f "$ENV_FILE" ] && echo present || echo absent)"
  echo "   STAGED registry entry: $([ -f "$STAGED_FILE" ] && echo "present ($STAGED_FILE) — NOT servable" || echo absent)"
  echo "   guidance file        : $([ -f "$GUIDANCE_FILE" ] && echo present || echo absent)"
  [ -n "$stage" ] && journal get "$SLUG"
  if [ -f "$STAGED_FILE" ] && [ ! -f "$ENV_FILE" ]; then
    echo
    echo "   An unfinished provision is staged. Continue it:  ./provision.sh $SLUG \"<Name>\" <gid> --resume"
    echo "   Or tear it down:                                  ./deprovision.sh --execute --confirm $SLUG"
  fi
  exit 0
fi

[ -n "$NAME" ] || { echo "!! client display name required, e.g. \"Acme Corp\"" >&2; exit 2; }
[ -n "$GROUP_ID" ] || { echo "!! telegram group id required (negative for groups; see multi/README.md)" >&2; exit 2; }
if [ -n "$GUIDANCE_FILE_OVERRIDE" ] && [ "$GUIDANCE_FILE_OVERRIDE" != "$CANONICAL_GUIDANCE_FILE" ]; then
  echo "!! non-default GUIDANCE_FILE is not supported on the canonical IronWorks tenant path." >&2
  echo "   Move the guidance to $CANONICAL_GUIDANCE_FILE and unset GUIDANCE_FILE." >&2
  exit 2
fi

API="${IRONCLAW_API:?set IRONCLAW_API (the multi-tenant instance base URL)}"
: "${IRONCLAW_OPERATOR_TOKEN:?set IRONCLAW_OPERATOR_TOKEN (a current operator/admin token)}"
MODEL_PIN="$(fleet_pin_of_record MODEL_PIN)"  # canonical tenants never inherit a MODEL override
DATA_COMPOSE_DIR="$(cd ../../deploy/account-intel/data && pwd)"

# ── compensation ──────────────────────────────────────────────────────────────────────
# What was created THIS run, so a failure can undo exactly that and nothing else. Each entry
# is undone in reverse order; each undo is VERIFIED, and anything that could not be verified
# is reported as residual authority rather than assumed gone.
CREATED_ORG=0 CREATED_MEMBER=0 CREATED_STAGED=0
RESIDUAL=()

compensate() {
  local code=$?
  set +e
  [ "$code" -eq 0 ] && return 0
  # Nothing created (a preflight failure, or a bad argument) needs no compensation report —
  # printing one would teach the operator to skim the banner that matters.
  if [ "$CREATED_ORG" -eq 0 ] && [ "$CREATED_MEMBER" -eq 0 ] && [ "$CREATED_STAGED" -eq 0 ]; then
    exit "$code"
  fi
  echo >&2
  echo "== provisioning FAILED (exit $code) — compensating what this run created ==" >&2

  if [ "$CREATED_STAGED" -eq 1 ] && [ -f "$STAGED_FILE" ]; then
    if rm -f "$STAGED_FILE"; then echo "   removed staged registry entry (was never servable)" >&2
    else RESIDUAL+=("staged registry file $STAGED_FILE could not be removed"); fi
  fi

  if [ "$CREATED_MEMBER" -eq 1 ] && [ -n "${IRONCLAW_USER_ID:-}" ]; then
    local dcode
    dcode=$(fleet_delete_member "$IRONCLAW_OPERATOR_TOKEN" "$API" "$IRONCLAW_USER_ID")
    if fleet_member_is_gone "$dcode"; then
      echo "   deleted the sealed member record ($IRONCLAW_USER_ID) -> HTTP $dcode" >&2
    else
      RESIDUAL+=("sealed IronClaw member $IRONCLAW_USER_ID still EXISTS (DELETE -> HTTP $dcode)")
    fi
    # Deleting the record does NOT revoke the token it issued (measured:
    # multi/verify/test_session_revocation.py). The token never left this process, so custody
    # contains it — but a compensation report that omitted this would be claiming more than it
    # did, which is the exact habit this rewrite exists to break.
    RESIDUAL+=("the member token minted this run stays VALID until it expires (no upstream session-revoke) — it never left this process, so custody is the containment")
  fi

  if [ "$CREATED_ORG" -eq 1 ]; then
    # One writer for this map (deploy/lib/identities.py), so the compensator and deprovision.sh
    # cannot deregister differently. Zero removed is success: already-absent is the end state.
    if python3 "$IDENTITIES" remove "$ORG_ID" >/dev/null; then
      # Verify the compensation rather than assume it: the file is hot-reloaded, so the answer
      # is observable immediately.
      local ocode
      ocode=$(fleet_http_code curl_header "X-Service-Token: $ACCOUNT_TOKEN" \
        -s -o /dev/null -w '%{http_code}' "$ACCOUNT_BASE/list_accounts")
      if [ "$ocode" = "401" ]; then
        echo "   deregistered the org token — VERIFIED revoked (Account Service now answers 401)" >&2
      else
        RESIDUAL+=("org token for '$ORG_ID' still authenticates the Account Service (HTTP $ocode)")
      fi
    else
      RESIDUAL+=("org token for '$ORG_ID' could not be deregistered from the identities file")
    fi
    RESIDUAL+=("Account-Store rows for org '$ORG_ID' (if any were seeded) are NOT removed here — run: ./deprovision.sh --execute --confirm $SLUG")
  fi

  echo >&2
  if [ "${#RESIDUAL[@]}" -gt 0 ]; then
    echo "!! RESIDUAL AUTHORITY after compensation — read every line:" >&2
    for r in "${RESIDUAL[@]}"; do echo "   - $r" >&2; done
  else
    echo "   compensation complete: nothing this run created still exists." >&2
  fi
  echo >&2
  echo "   The journal is kept at ${AGENCY_DIR:-$FLEET_AGENCY_DIR}/provision-journal/$SLUG.json:" >&2
  echo "     ./provision.sh $SLUG --status" >&2
  echo "   Resume with --resume once the cause is fixed; do NOT re-run without it." >&2
  exit "$code"
}
trap compensate EXIT

# ── preflight: everything checkable BEFORE any authority is created ───────────────────
echo "== preflight =="
PF_FAIL=0
pf() { # pf <ok?> <label> [remedy]
  if [ "$1" = "0" ]; then echo "   [x] $2"; else
    echo "   [!] $2"; [ -n "${3:-}" ] && echo "       -> $3"; PF_FAIL=1; fi
}

# pf_http <label> <url> <remedy> [bearer] — "does this endpoint answer 200?", the shape three
# preflight probes had copy-pasted. `fleet_http_code` keeps a connection failure a FINDING rather
# than a `set -e` abort with no preflight summary printed.
pf_http() {
  local code
  if [ -n "${4:-}" ]; then
    code="$(fleet_http_code curl_bearer "$4" -s -o /dev/null -w '%{http_code}' "$2")"
  else
    code="$(fleet_http_code curl -s -o /dev/null -w '%{http_code}' "$2")"
  fi
  if [ "$code" = "200" ]; then pf 0 "$1"; else pf 1 "$1" "$3 (HTTP $code)"; fi
}

# The one thing a re-run must not do is create a second copy of everything.
STAGE="$(journal stage "$SLUG")"
if [ -f "$ENV_FILE" ]; then
  pf 1 "tenant '$SLUG' is not already provisioned" "live registry entry exists: $ENV_FILE"
elif [ -n "$STAGE" ] && [ "$RESUME" -ne 1 ]; then
  pf 1 "no unfinished provisioning run for '$SLUG'" \
     "journal stage is '$STAGE'. Re-run with --resume, or tear down first: ./deprovision.sh --execute --confirm $SLUG"
else
  pf 0 "tenant '$SLUG' is not already provisioned$([ -n "$STAGE" ] && echo " (resuming from '$STAGE')")"
fi

# Telegram group ids must be unique across tenants: a duplicate misroutes a whole group to
# another tenant's credentials and data. load_clients fails closed on this at runtime too.
DUP=""
if [ -d "$CLIENTS_DIR" ]; then
  for cf in "$CLIENTS_DIR"/*.env; do
    [ -e "$cf" ] || continue
    [ "$(fleet_env_get "$cf" TELEGRAM_GROUP_ID)" = "$GROUP_ID" ] && DUP="$(basename "$cf" .env)"
  done
fi
if [ -n "$DUP" ]; then pf 1 "telegram group $GROUP_ID is unused" "already maps to tenant '$DUP' — one group = one tenant"
else pf 0 "telegram group $GROUP_ID is unused"; fi

# Guidance + service definition, validated by the SAME code the seam uses at load time. A
# guidance file that would fail at bridge startup must fail here, in front of the operator,
# before any authority exists.
if [ ! -f "$GUIDANCE_FILE" ]; then
  pf 1 "client guidance exists" "copy multi/clients/GUIDANCE.template.md to $GUIDANCE_FILE, fill it with the client, chmod 600"
else
  # NO TEMP FILE AT ALL, where this used to write to `mktemp` and `rm` it on the happy path —
  # so a Ctrl-C in between left a file quoting the tenant's guidance path in a world-writable
  # directory.
  #
  # A `trap` is the reflex fix and it is WRONG HERE. Line 208 installs `trap compensate EXIT`,
  # the handler that undoes partially-created authority, and bash REPLACES an EXIT trap rather
  # than chaining it — so adding one for this temp file would have silently disarmed
  # compensation for the rest of preflight. Capturing to a variable needs no trap at all.
  #
  # `if _pf_out=$(...)` preserves the exit status: an assignment takes the status of the command
  # substitution, so the branch below still tests the composition, not the assignment.
  if _pf_out=$(PYTHONPATH="$(cd ../seam && pwd)" GF="$GUIDANCE_FILE" SLUG="$SLUG" SVC="$SERVICE" \
     PERSONA_ROOT="$REPO_ROOT" python3 - 2>&1 <<'PY'
import os, sys
from persona import compose_service_persona, GuidanceError
from services import load_service, ServiceError
try:
    defn = load_service(os.environ["SVC"])
    compose_service_persona(defn, os.environ["GF"], os.environ["SLUG"])
except (GuidanceError, ServiceError, FileNotFoundError) as e:
    sys.exit(str(e))
PY
  )
  then pf 0 "guidance validates and composes against service '$SERVICE'"
  else pf 1 "guidance validates and composes against service '$SERVICE'" "$_pf_out"; fi
  # Guidance is that tenant's own data. A group- or world-readable file beside the tokens is a
  # finding here, where it is cheap to fix, not after it has been read.
  # GNU FIRST, AND ASSIGN ONLY ON SUCCESS. Both halves of that are load-bearing, and the
  # previous form — `$(stat -f '%Lp' … || stat -c '%a' … || echo "")` — got both wrong on Linux:
  #
  #   * `-f` is BSD's "format" and GNU's "--file-system". On GNU it is not an unknown option, so
  #     it does not fail cleanly: it PRINTS FIVE LINES OF FILESYSTEM STATISTICS TO STDOUT and
  #     then exits 1.
  #   * because the whole chain sits in ONE command substitution, that stdout is captured even
  #     though the command failed. The `||` fallback then appends the real mode, so `gmode`
  #     became "<filesystem blob>\n600" — which matches neither `600|400` nor `""`, so it fell
  #     through to the failure arm and reported a correctly-chmod-600 file as world-readable.
  #
  # Measured 2026-09-01: this failed preflight for a mode-600 file on the serve host, with the
  # remedy line telling the operator to `chmod 600` a file that already was. Ordering GNU first
  # is not enough on its own — a failing probe that writes to stdout would still poison a shared
  # substitution — so each probe assigns separately and only when it exits 0. macOS `stat -c`
  # writes nothing to stdout before failing, so the fallback is clean there. Verified on both.
  gmode="$(stat -c '%a' "$GUIDANCE_FILE" 2>/dev/null)" \
    || gmode="$(stat -f '%Lp' "$GUIDANCE_FILE" 2>/dev/null)" \
    || gmode=""
  case "$gmode" in
    600|400) pf 0 "guidance file is not group/world readable (mode $gmode)" ;;
    "") pf 0 "guidance file mode not checkable on this platform (skipped)" ;;
    *) pf 1 "guidance file is not group/world readable" "mode $gmode — chmod 600 $GUIDANCE_FILE" ;;
  esac
fi

# `if`, not `A && pf 0 || pf 1`: pf always succeeds, so the short-circuit form draws an
# SC2015 finding, and CI runs the linter bare (info-level findings fail that gate).
if fleet_require_container "$CONT" >/dev/null 2>&1; then
  pf 0 "Account Service container is running ($CONT)"
else
  pf 1 "Account Service container is running ($CONT)" "the data layer must be up before provisioning"
fi
pf_http "Account Service answers /health" "$ACCOUNT_BASE/health" "check $ACCOUNT_BASE"
pf_http "IronClaw instance answers /api/health" "$API/api/health" "check $API"
pf_http "the operator token is accepted by the admin API" \
        "$API/api/webchat/v2/admin/users" "refresh IRONCLAW_OPERATOR_TOKEN" \
        "$IRONCLAW_OPERATOR_TOKEN"

# Pin agreement: the image actually running must be the pinned rev. A tenant provisioned
# against an unpinned image is confined against a tool taxonomy nobody has measured.
if command -v docker >/dev/null 2>&1; then
  if "$REPO_ROOT/deploy/verify-pin.sh" "$(fleet_mt_container)" >/dev/null 2>&1; then
    pf 0 "the running MT image matches IRONCLAW_PIN"
  else
    pf 1 "the running MT image matches IRONCLAW_PIN" \
       "deploy/verify-pin.sh reports a MISMATCH or an UNLABELED image — see deploy/README.md"
  fi
else
  pf 0 "pin provenance not checkable here (no docker) — check it on the box that runs the container"
fi

[ -d "$CLIENTS_DIR" ] || mkdir -p "$CLIENTS_DIR"
pf 0 "registry directory is writable ($CLIENTS_DIR)"

if [ "$PF_FAIL" -ne 0 ]; then
  echo >&2
  echo "!! preflight FAILED — nothing was created. Fix the [!] lines above and re-run." >&2
  # No compensation reset here: preflight runs BEFORE anything is created, so the three
  # CREATED_* flags are still 0 from their declaration and re-zeroing them one line before
  # `exit 1` changed nothing.
  exit 1
fi
echo "   preflight clean."

if [ "$DRY_RUN" -eq 1 ]; then
  echo
  echo "DRY RUN — nothing was created. Re-run without --dry-run to provision '$SLUG'."
  trap - EXIT
  exit 0
fi

journal set "$SLUG" preflight_passed "org_id=$ORG_ID" "service=$SERVICE" "group_id=$GROUP_ID" >/dev/null

# ── 1. Account-Service org + token ────────────────────────────────────────────────────
umask 077
mkdir -p "$DATA_DIR_ROOT" "$STAGING_DIR"
chmod 700 "$STAGING_DIR"

# WHICH QUESTION THIS ASKS. Not "did a previous run of this tool register an org?" — that is
# journal provenance, and it is the wrong question. An org can be created by any supported path
# (`seed-real.sh` does), and when one was, the journal is silent while a perfectly valid
# credential exists. Answering from the journal then mints a SECOND live token, overwrites the
# credential file, and leaves the first authenticating: this tool manufacturing the exact
# credential accumulation it exists to avoid. So the decision comes from what the Account
# Service HAS (deploy/lib/identities.py); the journal keeps recording only what was DONE.
EXISTING_IDENTITIES="$(python3 "$IDENTITIES" count "$ORG_ID")" || {
  echo "!! cannot read Account-Service identity state — refusing to create authority blind" >&2; exit 1; }
if [ "$EXISTING_IDENTITIES" -gt 1 ]; then
  # Never choose between them: an arbitrary pick blesses one credential and leaves the rest
  # live and unaccounted for.
  echo "!! org '$ORG_ID' already has $EXISTING_IDENTITIES live Account-Service credentials." >&2
  echo "   Provisioning will not choose between them. Deregister the stale ones from" >&2
  echo "   ~/.agency/account-identities/identities.json, then re-run." >&2
  exit 1
elif [ "$EXISTING_IDENTITIES" -eq 1 ]; then
  echo "== 1/5 org '$ORG_ID' already holds an Account-Service identity — REUSING it (none minted) =="
  ACCOUNT_TOKEN="$(python3 "$IDENTITIES" resolve "$ORG_ID")" || {
    echo "!! could not resolve the existing identity for '$ORG_ID'" >&2; exit 1; }
  # Never clobber an existing credential file: it is the operator's reference to authority this
  # run did not create. Write one only when it is missing.
  if [ ! -f "$DATA_DIR_ROOT/$SLUG.env" ]; then
    cat > "$DATA_DIR_ROOT/$SLUG.env" <<EOF
REAL_ORG_ID=$(fleet_sh_quote "$ORG_ID")
REAL_ORG_NAME=$(fleet_sh_quote "$NAME")
REAL_ACCOUNT_TOKEN=$(fleet_sh_quote "$ACCOUNT_TOKEN")
EOF
  fi
  # CREATED_ORG stays 0 ON PURPOSE. Compensation deregisters EVERY token for the org, so
  # marking a reused org as created would let a later failure revoke authority that predates
  # this run — for a canonical dataset, its only credential.
  journal set "$SLUG" org_registered >/dev/null
else
  # No identity exists. A journal that claims otherwise is describing authority that is not
  # there; continuing would serve a tenant a credential the Account Service rejects.
  if python3 "$LIFECYCLE" journal reached "$SLUG" org_registered; then
    echo "!! the provisioning journal says org '$ORG_ID' is registered, but the Account Service" >&2
    echo "   holds NO identity for it. Authoritative state wins: the journal is stale or the" >&2
    echo "   identity was deregistered. Clear the journal (lifecycle.py journal clear $SLUG)" >&2
    echo "   and re-run, or restore the identity." >&2
    exit 1
  fi
  echo "== 1/5 Account-Service org '$ORG_ID' + token (hot-reloaded — zero interruption) =="
  ACCOUNT_TOKEN="$(openssl rand -hex 24)"
  # `fleet_sh_quote`, not bare "$NAME": this file is `. `-sourced by seed-real.sh, which step 2
  # of THIS script invokes. A display name containing $(…) or a backtick was executed at source
  # time, and one with an odd number of quotes killed the seeder on a parse error.
  cat > "$DATA_DIR_ROOT/$SLUG.env" <<EOF
REAL_ORG_ID=$(fleet_sh_quote "$ORG_ID")
REAL_ORG_NAME=$(fleet_sh_quote "$NAME")
REAL_ACCOUNT_TOKEN=$(fleet_sh_quote "$ACCOUNT_TOKEN")
EOF
  CREATED_ORG=1   # this run created it, so a failure must compensate it
  ORG_TOKEN="$ACCOUNT_TOKEN" "$DATA_COMPOSE_DIR/register-identity.sh" "$ORG_ID"
  journal set "$SLUG" org_registered >/dev/null
fi

# ── 2. seed the tenant's book (if the operator staged one) ────────────────────────────
if python3 "$LIFECYCLE" journal reached "$SLUG" data_seeded; then
  echo "== 2/5 data — already seeded by an earlier run =="
elif ls "$DATA_DIR_ROOT/$SLUG"/*.json >/dev/null 2>&1; then
  echo "== 2/5 seed the tenant's book =="
  "$DATA_COMPOSE_DIR/seed-real.sh" "$SLUG"        # register identity + seed data + isolation smoke
  journal set "$SLUG" data_seeded >/dev/null
else
  echo "== 2/5 no staged data for '$SLUG' — the org starts empty (valid: org existence is row scoping) =="
  journal set "$SLUG" data_seeded >/dev/null
fi

# ── 3. sealed IronClaw member + confinement ───────────────────────────────────────────
if python3 "$LIFECYCLE" journal reached "$SLUG" staged && [ -f "$STAGED_FILE" ]; then
  echo "== 3/5 sealed member — reusing the one staged by an earlier run =="
  IRONCLAW_USER_ID="$(fleet_env_get "$STAGED_FILE" IRONCLAW_USER_ID)"
  IRONCLAW_TOKEN="$(fleet_env_get "$STAGED_FILE" IRONCLAW_TOKEN)"
  [ -n "$IRONCLAW_TOKEN" ] || { echo "!! resume: staged entry carries no IRONCLAW_TOKEN — tear down and start again" >&2; exit 1; }
  CREATED_MEMBER=1 CREATED_STAGED=1
else
  echo "== 3/5 sealed IronClaw member account on $API =="
  # No eval: run the child, check its exit status, then parse only the two expected
  # KEY=VALUE lines — child stdout is data, never shell code.
  if ! sealed_out="$(./provision-client.sh --env "$NAME")"; then
    echo "!! sealed-account provisioning failed" >&2
    exit 1
  fi
  IRONCLAW_USER_ID="$(printf '%s' "$sealed_out" | sed -n 's/^IRONCLAW_USER_ID=//p')"
  IRONCLAW_TOKEN="$(printf '%s' "$sealed_out" | sed -n 's/^IRONCLAW_TOKEN=//p')"
  # `if`, not `A && B || C`. Both operands here are pure `[` tests, so the old form behaved
  # correctly — but this guards a member credential that has just been MINTED, and the next
  # line records it in the journal. In that shape any command later inserted between the
  # operands makes its failure fire the abort too, silently widening what "no usable id" means.
  # The compensator path is the wrong place to leave that edge.
  if [ -z "$IRONCLAW_USER_ID" ] || [ -z "$IRONCLAW_TOKEN" ]; then
    echo "!! provision-client.sh --env returned no usable IRONCLAW_USER_ID/IRONCLAW_TOKEN" >&2
    exit 1
  fi
  CREATED_MEMBER=1
  journal set "$SLUG" member_minted "ironclaw_user_id=$IRONCLAW_USER_ID" >/dev/null

  # Confine the member to a read-only, NO-EGRESS surface BEFORE it is servable. A fresh member
  # ships builtin.http (wildcard egress) always_allow; a prompt-injected turn holding this
  # tenant's private book could otherwise POST it to an attacker host. ironclaw exposes no
  # config egress allowlist (compiled-in), so we disable the egress/write/escalate tools
  # per-bearer with the member's OWN token and PROBE fail-closed. FATAL if confinement cannot
  # be certified — a tenant that cannot be confined must not be provisioned.
  echo "== 3b/5 confine the sealed member (no-egress, read-only) =="
  IRONCLAW_API="$API" IRONCLAW_MEMBER_TOKEN="$IRONCLAW_TOKEN" ./confine-member.sh \
    || { echo "!! member confinement FAILED — refusing to provision an un-confined tenant" >&2; exit 1; }
  journal set "$SLUG" member_confined >/dev/null

  # STAGED, not live. CLIENTS_DIR/.staging/ is a subdirectory, and the seam reads CLIENTS_DIR
  # with a `*.env` glob — which does not descend. So this file is complete, correct, and
  # unservable until the smoke gates below pass and it is moved.
  echo "== 4/5 stage the registry entry (NOT yet servable) =="
  cat > "$STAGED_FILE" <<EOF
CLIENT_SLUG="$SLUG"
CLIENT_NAME="$NAME"
SERVICE="$SERVICE"
ORG_ID="$ORG_ID"
ACCOUNT_TOKEN="$ACCOUNT_TOKEN"
IRONCLAW_TOKEN="$IRONCLAW_TOKEN"
IRONCLAW_USER_ID="$IRONCLAW_USER_ID"
TELEGRAM_GROUP_ID="$GROUP_ID"
EOF
  chmod 600 "$STAGED_FILE"
  CREATED_STAGED=1
  journal set "$SLUG" staged >/dev/null
fi

# ── 5. smoke: EVERY LEG ASSERTS, AND A FAILURE STOPS THE RUN ──────────────────────────
# This block once only PRINTED, and the run went on to declare the client provisioned even
# after emitting the literal string "LEAK: <org>". A smoke test whose output nothing reads is
# a status report, not a gate — and it is the last thing between provisioning and a tenant
# being declared live.
echo "== 5/5 smoke =="
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

# `smoke_code`, the copy `smoke.sh` exists to be. Its header states the reason: "all three are
# answering the same question and two copies can disagree about what a passing answer looks
# like" — and this was the fourth copy, written before that file and never converted. It was
# also the only place in the repository that passed a token through a raw `-H` rather than
# `curl_header`, which puts it on argv where `ps` can read it. Fake token here, real pattern.
#
# `|| smoke_fail`, not a bare call: `smoke_code` returns non-zero on mismatch, and this block
# ACCUMULATES failures rather than aborting — the comment above it explains why the whole smoke
# section is a gate rather than a status report.
smoke_code "unknown token -> 401 (fail closed)" 401 \
  "X-Service-Token: not-a-real-token-$(openssl rand -hex 8)" /list_accounts \
  || smoke_fail "the store is not failing closed on an unknown token"

# isolation: every OTHER registered client token must resolve to its own org, never this one
OTHER=$(python3 "$IDENTITIES" other "$ORG_ID")
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
  echo "   (first tenant — no other org token to cross-check yet)"
  echo "      NOTE: cross-org isolation is UNPROVEN for this tenant. There is nothing to"
  echo "      cross-check against until a second tenant exists — re-run this leg then."
fi

echo -n "   sealed IronClaw token answers: "
ans=$(curl_bearer "$IRONCLAW_TOKEN" -sf -m 120 -X POST -H 'content-type: application/json' \
  -H "User-Agent: $BROWSER_UA" \
  -d '{"model":"'"$MODEL_PIN"'","input":"Reply with the single word: ready"}' \
  "$API/v1/responses" 2>/dev/null || true)
if [ -z "$ans" ]; then
  echo "(no response)"
  smoke_fail "the sealed member token got no answer from IronClaw — the tenant cannot be served"
else
  # `responses.output_text`, not a fourth hand-rolled walk: this leg DECIDES whether the tenant
  # is activated, and a walk with no content-type filter passes on a reasoning item — text the
  # product would never deliver to the client. The seam module is import-weight-free by design.
  printf '%s' "$ans" | PYTHONPATH="$(cd ../seam && pwd)" python3 -c "
import sys, json, responses
d = json.load(sys.stdin)
t = (responses.output_text(d) or '').strip()
print((t or d.get('status', '?'))[:40])
sys.exit(0 if t else 1)" \
    || smoke_fail "the sealed member token produced no message text — the turn did not complete"
fi

# THE LIVE REGISTRY PLUS THIS TENANT, not this tenant alone. `load_clients` finds a reused
# ACCOUNT_TOKEN, IRONCLAW_TOKEN, group id or slug by comparing entries against EACH OTHER, so a
# directory holding one entry satisfies every one of those rules vacuously — which is how a
# second tenant on an org whose credential step 1 REUSED could pass this leg, activate, and then
# refuse the whole registry at the next bridge start. Two phases, because "no" has two meanings
# and only one of them is this tenant's fault; deploy/lib/registry_validation.py owns both.
echo "   the live registry still loads WITH this tenant in it:"
# `|| _rv=$?`, not `&& _rv=0 || _rv=$?`: the short-circuit form draws SC2015 and CI runs the
# linter bare. No temp file either — the module writes its verdict to stdout on success and to
# stderr otherwise, and neither carries a credential value (load_clients names the file and the
# owning slug, never the token).
_rv=0
IRONCLAW_API="$API" PERSONA_ROOT="$REPO_ROOT" \
  python3 "$REPO_ROOT/deploy/lib/registry_validation.py" \
  "$CLIENTS_DIR" "$STAGED_FILE" "$GUIDANCE_FILE" || _rv=$?
case "$_rv" in
  0) : ;;
  3) smoke_fail "the EXISTING registry does not load, with or without this tenant — fix that first.
      This is NOT a defect in '$SLUG': its entry is still staged and uninvolved. Run
      ./deploy/ironworks --offline doctor to see which tenant is broken." ;;
  *) smoke_fail "adding '$SLUG' breaks the registry the bridge reads — it would refuse to start
      for EVERY tenant, not just this one. The rule that refused it is named above." ;;
esac

echo
if [ "$SMOKE_FAIL" -ne 0 ]; then
  echo "❌ smoke FAILED — '$SLUG' is NOT activated. The registry entry is still STAGED and the" >&2
  echo "   tenant is not servable. Fix the cause and re-run with --resume, or tear it down:" >&2
  echo "     ./deprovision.sh --execute --confirm $SLUG" >&2
  exit 1
fi
journal set "$SLUG" smoke_passed >/dev/null

# ── activate ──────────────────────────────────────────────────────────────────────────
# The one moment this tenant becomes servable, and it is a rename of a file that has already
# passed every gate.
# The mode is set on the STAGED file, before the rename, so the file is never readable at its
# live path for even an instant — and so that nothing between here and the disarm below can
# fail. `mv` within one directory is atomic.
chmod 600 "$STAGED_FILE"
mv "$STAGED_FILE" "$ENV_FILE"
# DISARM IMMEDIATELY. This was three statements later, which made the window between "the
# tenant is live" and "the compensator is off" reachable: a failing chmod or journal write
# (read-only or full ~/.agency, EPERM) ran `compensate` with CREATED_MEMBER=1, whose staged-file
# arm tests `[ -f "$STAGED_FILE" ]` — false, because the rename already happened. So the live,
# servable $ENV_FILE survived while the sealed member was DELETEd and every org token
# deregistered: a tenant the bridge routes, holding credentials that were just revoked, and a
# RESIDUAL report that never mentions the registry file it left behind.
trap - EXIT
journal set "$SLUG" activated >/dev/null

echo "✅ tenant '$NAME' provisioned and ACTIVATED as '$SLUG'"
echo "   org=$ORG_ID  service=$SERVICE  group=$GROUP_ID"
echo "   -> restart telegram_bridge.py to pick it up (the registry is read at startup)"
echo "   -> verify:  ./deploy/ironworks tenant inspect $SLUG"
