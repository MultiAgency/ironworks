# deploy/lib/fleet.sh — fleet-wide constants + helpers, sourced by
# provision-agent.sh, update-persona.sh, doctor.sh, and migrate-image.sh.
#
# The persona destination path lives HERE and nowhere else fleet-side. It
# mirrors three upstream-INTERNAL derivations (entrypoint home default +
# profile storage subdir + a pub(crate) prompt-path constant), none of which
# upstream promises to keep; migrate-image.sh gates every pin bump
# on all three still holding in the pinned source, and doctor.sh's stray-seed
# sweep is the runtime backstop if they silently move.
# shellcheck shell=bash

# shellcheck disable=SC2034  # consumed by the sourcing scripts, not here
FLEET_PERSONA_DST="/data/ironclaw-reborn/hosted-single-tenant-volume/system/prompts/default-system.md"

# IronClaw 1.4 creates this persistent directory during its root entrypoint pass. IronWorks
# deliberately withholds DAC_OVERRIDE from the runtime, so root cannot create a child beneath
# the image-seeded, mode-0755 home after that home is owned by uid 1000. Prepare the one path as
# the SAME uid that owns and later serves it; do not broaden the long-lived container instead.
# shellcheck disable=SC2034  # asserted by tests and used by fleet_prepare_workspace
FLEET_WORKSPACE_ROOT="/data/ironclaw-reborn/workspace"

# fleet_prepare_workspace <image> <volume> — deterministic, capability-free 1.4 workspace init.
#
# The official image seeds /data/ironclaw-reborn as 1000:1000. Docker copies that ownership into
# a fresh named volume before this process runs, so uid 1000 can create the child on both cold
# boot and restart. The initializer has no network, a read-only image filesystem, no capabilities,
# and no privilege escalation; its only writable object is the already-required /data volume.
fleet_prepare_workspace() {
  local _image="${1:?fleet_prepare_workspace requires an image}"
  local _volume="${2:?fleet_prepare_workspace requires a volume}"
  docker run --rm --network none --read-only --user 1000:1000 \
    --cap-drop ALL --security-opt no-new-privileges:true \
    --entrypoint /bin/mkdir -v "$_volume:/data" "$_image" \
    -p "$FLEET_WORKSPACE_ROOT"
}

# fleet_agent_env <slug> — path of the agent's recorded secrets/env file.
fleet_agent_env() { printf '%s/%s.env\n' "${MULTRON_SECRETS_DIR:-$FLEET_AGENCY_DIR/agents}" "$1"; }

# fleet_install_persona <container> <composed-prompt> — write a composed persona to the one
# path. Both writers (provision + update) go through here so the install mechanism exists once.
fleet_install_persona() {
  printf '%s\n' "$2" | docker exec -i "$1" sh -c "cat > '$FLEET_PERSONA_DST'"
}

# fleet_container <slug> — the CONTAINER= value recorded at provision time (never re-derive
# when the record exists), else ironclaw-<slug>.
fleet_container() {
  local envf c=""
  envf="$(fleet_agent_env "$1")"
  if [ -f "$envf" ]; then c="$(fleet_env_get "$envf" CONTAINER)"; fi
  printf '%s\n' "${c:-ironclaw-$1}"
}

# The lib dir + repo root, resolved from THIS file rather than from $0, so a helper
# behaves the same whichever script sourced it and from whatever cwd.
_FLEET_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FLEET_REPO_ROOT="$(cd "$_FLEET_LIB_DIR/../.." && pwd)"

# The operator state directory — registry, identity map, journals, ledgers, account data, env
# files. AGENCY_DIR was honoured by deploy/lib/lifecycle.py and by NOTHING in shell, while
# provision.sh printed it to the operator as though it governed. Setting it therefore moved the
# journal and the residual-authority ledger and left the registry, the identity map and the
# staging tree behind — a half-honoured knob, which is worse than no knob, because the operator
# is told the relocation happened.
# shellcheck disable=SC2034  # consumed by the sourcing scripts, not here
FLEET_AGENCY_DIR="${AGENCY_DIR:-$HOME/.agency}"

# curl_bearer/curl_tg live next door and fleet_wait_api needs them. Sourcing here is
# idempotent (these are function definitions) and by absolute path, so a caller no longer
# has to know about curl-private.sh *or* be in the right cwd to reach it.
. "$_FLEET_LIB_DIR/curl-private.sh"

# fleet_env_get <env-file> <key> — read KEY=VALUE from an agent/client env file.
#
# ONE quoting rule for the whole fleet: leading/trailing whitespace tolerated, both quote
# styles stripped. A stricter reader here once rejected a padded value, SKIPPED the account
# deletion it was gating, and exited 0 with the sealed account still alive.
#
# Absent key prints nothing and still returns 0, deliberately: callers test emptiness, and
# non-zero would abort the assignment under `set -e`. No grep either — sed returns 0 on no
# match.
fleet_env_get() {
  local _v=""
  if [ -f "$1" ]; then
    _v="$(sed -n -E "s/^[[:space:]]*$2=[[:space:]]*(.*)\$/\1/p" "$1" | head -n1)"
    _v="${_v%"${_v##*[![:space:]]}"}"                       # rstrip
    case "$_v" in
      \"*\") _v="${_v#\"}"; _v="${_v%\"}" ;;
      \'*\') _v="${_v#\'}"; _v="${_v%\'}" ;;
    esac
  fi
  printf '%s\n' "$_v"
}

# fleet_json <python-expr> — evaluate a Python expression against JSON on stdin; `d` is the
# parsed document. Loud on purpose: a malformed body should say why, so a caller that wants
# silence asks for it with `2>/dev/null`.
fleet_json() { python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }

# fleet_sh_quote <value> — the value as ONE shell word, safe to write into a file that will be
# `source`d. Operator-supplied text reaches these env files (a client's display name), and the
# files are plainly `. `-sourced by seed-real.sh in the same provisioning run, so `NAME="$v"`
# in a heredoc executed `$(…)` and backticks at source time and died on an unbalanced quote.
# `shlex.quote` for the same reason `fleet_json` exists: escaping rules belong in one place.
fleet_sh_quote() { python3 -c 'import shlex,sys;print(shlex.quote(sys.argv[1]))' "$1"; }

# fleet_slug <display-text> — the slug a display name derives to. Lowercased, every run of
# non-alphanumerics collapsed to one `-`, no leading or trailing `-`.
#
# Two scripts carried this pipeline byte-for-byte (provision-agent.sh and provision-client.sh)
# and three more carried the matching VALIDATION rule with three different messages and two
# different exit codes. A slug names a container, a volume, a hostname, an env file and a
# directory under the operator state root, so two answers to "what is this client called" is two
# answers to "which files are this client's".
fleet_slug() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-//;s/-$//'
}

# fleet_slug_valid <slug> — true when the slug is exactly lowercase letters, digits and `-`,
# and non-empty.
#
# POSIX CLASSES, NOT `a-z0-9`, AND THE DIFFERENCE IS NOT COSMETIC. A bracket RANGE in a shell
# pattern is resolved by the current collation, and under `en_US.UTF-8` — the default on macOS
# and most dev shells — `[a-z]` matches `A` through `Z` as well. All three copies of this rule
# used `*[!a-z0-9-]*`, so all three ACCEPTED `Acme` while their own error message said
# "must be lowercase". Measured on this tree's shell: `case Acme in *[!a-z0-9-]*)` does not
# match, and the same test under `LC_ALL=C` does.
#
# The consequence is not traversal — `/` and `.` are rejected either way, which is what
# seed-real.sh's guard is really for — but a hand-typed `Acme` produced `~/.agency/clients/
# Acme.env` beside the `acme.env` that `fleet_slug` derives, and on a case-insensitive
# filesystem those are the same file with two names. `[:lower:]` and `[:digit:]` are single
# classes, not ranges, so no collation can widen them.
#
# NON-EMPTY IS PART OF THE RULE, not an extra: `*[!…]*` does not match the empty string, so the
# copy that omitted `''` accepted "" as a valid slug. Callers keep their own message and exit
# code — seed-real.sh exits 1 where the provisioning pair exits 2, and that is their contract,
# not this function's business. What they share is the answer.
fleet_slug_valid() {
  case "${1:-}" in ''|*[![:lower:][:digit:]-]*) return 1 ;; *) return 0 ;; esac
}

# fleet_first_free_port <lo> [hi] — print the lowest port in [lo,hi] nothing is bound to, or
# return 1 if the whole range is taken. `hi` defaults to `lo`, which is the "is THIS port free?"
# question.
#
# NOT `lsof`, and that is the point. Two callers asked this with lsof and both inherited its
# absence as a wrong answer: `if lsof …; then` treats "not installed" (exit 127) identically to
# "the port is free", so run-proof.sh's collision guard was inert on any host without it —
# including `ubuntu-latest`, which scheduled-integration.yml runs it on — while provision-agent's
# scan fell back to an EMPTY listener list and handed out 3001 with an agent already on it.
#
# A bind attempt cannot be wrong by being absent, and it answers the question docker will ask.
# Binding 0.0.0.0 rather than loopback for the same reason: `docker run -p <port>:3000` publishes
# on all interfaces, so that is the conflict that matters. The socket is never listened on and is
# closed immediately, so nothing can connect to it.
#
# ONE process for the whole range, which is also why the range form exists: provision-agent.sh
# scans 3001-3099 and its previous shape spawned a full lsof per candidate — each walking every
# open fd on the box — so finding port 3021 on a twenty-agent host cost twenty whole-system scans.
fleet_first_free_port() {
  python3 - "$1" "${2:-$1}" <<'PY'
import socket, sys
lo, hi = int(sys.argv[1]), int(sys.argv[2])
for port in range(lo, hi + 1):
    s = socket.socket()
    # SO_REUSEADDR so a lingering TIME_WAIT socket is not mistaken for a live listener. A real
    # listener still refuses the bind, which is the state worth reporting.
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        s.bind(("0.0.0.0", port))
    except OSError:
        continue
    finally:
        s.close()
    print(port)
    sys.exit(0)
sys.exit(1)
PY
}

# fleet_require_container <name> — hard-fail unless the container is running. `grep -qx`, not
# a substring match: `secretary-ironclaw-1` must not satisfy a check for `ironclaw-1`.
fleet_require_container() {
  docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$1" || {
    echo "!! container is not running: $1" >&2; return 1; }
}

# fleet_http_code <curl-invocation…> — run a curl (or curl_bearer/curl_header/curl_tg)
# invocation whose ONLY stdout is `%{http_code}`, and print that code. A request that never left
# prints `000`.
#
# THE SWALLOW RULE, WRITTEN ONCE. Without `-f`, curl exits 0 on 4xx/5xx — but NON-ZERO when the
# request never left (connection refused, DNS, timeout). Under `set -euo pipefail` that aborts
# the assignment, which is how an unreachable instance killed a teardown mid-way (after the org
# token had already been deregistered) and how a retry loop died on its first iteration instead
# of reaching the diagnostic written for exactly that case. So every caller needs the failure
# swallowed into a code it can interpret.
#
# The WRONG way to swallow it is `|| echo 000`, and it is wrong in a way that reads as correct:
# curl has ALREADY written `000` before it exits non-zero, so appending another yields the
# literal `000000`. That value matches no case arm and no equality test, so a probe that never
# reached the instance falls through to whatever the else branch says — in deprovision.sh, to
# "the former member token STILL AUTHENTICATES", a fabricated residual-authority ledger entry
# and exit 3 where the contract says 4. `|| true` plus a `${…:-000}` default is the right way,
# and it lives here so no call site has to remember which is which.
fleet_http_code() {
  local _code
  _code="$("$@" || true)"
  printf '%s\n' "${_code:-000}"
}

# fleet_delete_member <operator-token> <api-base> <user-id> — delete a sealed member record,
# printing the HTTP code. Teardown happens in two places and they wrote this twice:
# provision.sh's compensator (undoing what one failed run created) and deprovision.sh (removing
# a tenant for good). They must agree, because the question both are answering is the same one.
fleet_delete_member() {
  fleet_http_code curl_bearer "$1" -s -o /dev/null -w '%{http_code}' -X DELETE \
    "$2/api/webchat/v2/admin/users/$3"
}

# fleet_member_is_gone <http-code> — the one rule for reading that code. 404 counts as gone: the
# desired end state is "no such record", and a second teardown of an already-deleted tenant must
# be a no-op rather than a failure. 000 is deliberately NOT gone — the request never arrived, so
# nothing was learned, and reporting that as success is how residual authority goes unrecorded.
fleet_member_is_gone() {
  case "$1" in 2??|404) return 0 ;; *) return 1 ;; esac
}

# fleet_wait_api <operator-token> <api-base> — block until the operator API answers. The
# failure clause lives here so it cannot be the thing a caller forgets: without it a
# never-ready instance falls through to being configured anyway.
fleet_wait_api() {
  local _tok="$1" _api="$2"
  curl_bearer "$_tok" -s -o /dev/null --retry 20 --retry-delay 2 --retry-connrefused \
    --retry-all-errors --max-time 10 "$_api/api/webchat/v2/admin/users" || {
      echo "!! operator API never became ready at $_api (retried 20x/2s)" >&2; return 1; }
}

# THE SECOND ADAPTER at a real seam: multi/seam/pins.py is the reader every Python caller uses,
# and provisioning is shell, so it cannot import it. That makes silent divergence the risk —
# provisioning smoke-testing one model while the seam serves another — so
# multi/seam/test_pins.py RUNS these functions and asserts they agree with the module. Change
# the parse rule in one and the gate fails, not a client turn.
#
# NO fallback literal, deliberately; the reasoning is written once, in pins.py's header.

# fleet_pin_of_record <pin-file> — THE PARSE RULE, once, shell side: everything before the first
# `#`, first line only, whitespace stripped. Mirrors pins.py::pin_value.
#
# Both pin files carry a trailing comment explaining the choice, and MODEL_PIN carries a further
# twenty lines of it. `cut -d' ' -f1` — which three call sites used before this function existed
# — is NOT this rule: it yields `<value>#` when no space precedes the comment, and on a file with
# a second comment line it returns a MULTI-LINE value that no equality test can ever match.
fleet_pin_of_record() {
  local _f="$FLEET_REPO_ROOT/$1" _v
  [ -f "$_f" ] || { echo "!! no $1 at $_f — it is tracked; this is a broken checkout" >&2; return 1; }
  _v="$(cut -d'#' -f1 < "$_f" | head -n1 | tr -d '[:space:]')"
  [ -n "$_v" ] || { echo "!! $_f names nothing on its first line" >&2; return 1; }
  printf '%s\n' "$_v"
}

# fleet_model_pin — the model of record. `MODEL` env wins for a one-off.
fleet_model_pin() {
  [ -n "${MODEL:-}" ] && { printf '%s\n' "$MODEL"; return 0; }
  fleet_pin_of_record MODEL_PIN
}

# fleet_ironclaw_pin — the runtime rev of record, the rev every image is built from.
#
# No env override, matching pins.py::ironclaw_pin: `MODEL` is a documented one-off for the model
# and must not leak into the runtime pin (multi/seam/test_pins.py asserts that separation).
fleet_ironclaw_pin() {
  fleet_pin_of_record IRONCLAW_PIN
}

# Two resolvers, and picking the wrong one is silent:
#   fleet_mt_container_configured — CONFIGURED name only (MT_CONTAINER > compose
#     container_name > default). No live lookup, so it is the right one for a REMOTE target:
#     what runs on this laptop says nothing about what runs there.
#   fleet_mt_container — the same, then resolved against LOCAL reality. If the configured name
#     is not running but the legacy one is, trust reality; that covers the window after a new
#     compose syncs but before MT is recreated.
#
# A missing compose is a HARD ERROR, never a guess: a provenance gate that checks the wrong
# container certifies nothing. `|| true` on the grep is required — under pipefail an unmatched
# grep aborts the assignment, so the `:=` default would never run.
fleet_mt_container_configured() {
  local _compose="$FLEET_REPO_ROOT/multi/instance/docker-compose.yml" _name
  [ -n "${MT_CONTAINER:-}" ] && { printf '%s\n' "$MT_CONTAINER"; return 0; }
  [ -f "$_compose" ] || {
    echo "!! MT compose not found: $_compose — cannot derive the container name" >&2; return 1; }
  _name="$(grep -E '^[[:space:]]*container_name:[[:space:]]' "$_compose" | head -1 | awk '{print $2}' || true)"
  printf '%s\n' "${_name:-multi-ironclaw-1}"
}

fleet_mt_container() {
  local _name
  _name="$(fleet_mt_container_configured)" || return 1
  if ! docker inspect "$_name" >/dev/null 2>&1 && docker inspect multi-ironclaw-1 >/dev/null 2>&1; then
    _name=multi-ironclaw-1
  fi
  printf '%s\n' "$_name"
}

# The ACCOUNT STACK's two containers. The MT container got a resolver; these did not, so their
# names were spelled four different ways across five files — two byte-identical literal lines
# (provision.sh and seed-real.sh), a `docker ps | grep`, a `docker ps -qf`, and a Python default.
# The `-qf` one was the dangerous shape: `docker ps -qf` EXITS 0 on no match, so a renamed
# project turned `docker exec "$(...)" pg_dump` into `docker exec "" pg_dump`.
#
# One precedence, a superset of what each site did on its own:
#   1. the env override every caller already honoured,
#   2. the compose default name if that container exists,
#   3. a live lookup, which is what covers a renamed compose project,
# and a HARD ERROR if none matched — never an empty string handed to `docker exec`.
_fleet_resolve_container() {
  local _override="$1" _default="$2" _match="$3" _found
  [ -n "$_override" ] && { printf '%s\n' "$_override"; return 0; }
  if docker inspect "$_default" >/dev/null 2>&1; then printf '%s\n' "$_default"; return 0; fi
  _found="$(docker ps --format '{{.Names}}' | grep -E -- "$_match" | head -n1 || true)"
  [ -n "$_found" ] || {
    echo "!! no running container matches '$_match', and the default '$_default' does not" \
         "exist. Start the stack, or set the matching *_CONTAINER variable." >&2
    return 1; }
  printf '%s\n' "$_found"
}

fleet_account_service_container() {
  _fleet_resolve_container "${ACCOUNT_SERVICE_CONTAINER:-}" \
    multiagency-data-account-service-1 'account-service'
}

fleet_account_db_container() {
  _fleet_resolve_container "${ACCOUNT_DB_CONTAINER:-}" \
    multiagency-data-account-db-1 'account-db'
}

# The MT instance's OWN database (IronClaw threads/memory), as distinct from fleet_mt_container,
# which resolves the app. The legacy `mt-experiment` generation stays in the MATCH only — the
# instance project was renamed to `multi`, and this is the one place that still needs to find a
# box that predates the rename.
fleet_mt_db_container() {
  _fleet_resolve_container "${MT_DB_CONTAINER:-}" multi-db-1 '(multi|mt-experiment)-db'
}

# fleet_wait_health <url> [attempts] [compose-service] — poll a /health endpoint, then dump that
# service's compose logs and fail if it never came up.
#
# The log dump runs `docker compose` with NO -f, so it resolves against the CALLER'S cwd. Both
# callers run from deploy/account-intel/data, which is why `account-service` is a sensible
# default and why neither has ever passed the third argument. A caller from anywhere else gets
# a failed `compose logs` (swallowed by `|| true`) rather than a wrong one — the wait itself
# still reports correctly, so this degrades the diagnostic and not the verdict.
fleet_wait_health() {
  local _url="$1" _n="${2:-30}" _svc="${3:-account-service}" i
  for i in $(seq 1 "$_n"); do
    if curl -sf "$_url" >/dev/null 2>&1; then echo "  healthy (attempt $i)"; return 0; fi
    sleep 2
  done
  echo "!! service did not become healthy at $_url" >&2
  docker compose logs --tail=40 "$_svc" || true
  return 1
}
