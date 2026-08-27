#!/usr/bin/env bash
# egress-control.sh — apply, prove, and (deliberately) undo the network egress boundary.
#
#   ./deploy/egress/egress-control.sh status                 what is enforcing, right now
#   ./deploy/egress/egress-control.sh verify                 prove it against the live runtime
#   ./deploy/egress/egress-control.sh activate --confirm     put the boundary in place
#   ./deploy/egress/egress-control.sh rollback --i-accept-unrestricted-egress
#
# EVERY SUBCOMMAND IS IDEMPOTENT. Activating an already-active boundary re-converges and stays
# quiet; rolling back an already-rolled-back one is a no-op. `status` is derived from the
# RUNNING container, never from the presence of files — a compose overlay on disk proves an
# intention, and this repo has to be able to tell those apart.
#
# THERE IS NO AUTOMATIC FAIL-OPEN. If the gateway dies, the runtime stays internal-only and
# stops reaching its model provider: the product breaks and the containment holds. That is the
# intended trade and it is measured (deploy/egress/proof/, checks 17/17b). Restoring
# unrestricted networking takes `rollback`, an explicit flag whose name is the admission it
# is, and it records a DEGRADED security state that `ironworks doctor` reports until cleared.
#
# ACTIVATION IS A MAINTENANCE EVENT. It recreates the runtime container: every in-flight turn
# is lost and every client group is briefly unserved. Read SECURITY.md and deploy/README.md
# before running it on a host that is serving anyone.
set -euo pipefail
cd "$(dirname "$0")"
REPO="$(cd ../.. && pwd)"
# Sourced, not re-entered: `container()` used to spawn a whole new `bash -c '. fleet.sh; ...'`
# per call, from inside bash, twice. Sourcing once gives the same resolver plus FLEET_AGENCY_DIR.
. "$REPO/deploy/lib/fleet.sh"
BASE="$REPO/multi/instance/docker-compose.yml"
OVERLAY="$REPO/deploy/egress/docker-compose.egress.yml"
# `$SELF`, not `$0`: this script cd's to its own directory above, so a RELATIVE invocation —
# `./deploy/egress/egress-control.sh`, the form its own usage text documents — leaves `$0`
# pointing at nothing, and the usage output became a `sed: No such file` error.
SELF="$PWD/$(basename "$0")"
usage() { sed -n '2,12p' "$SELF" | sed 's/^# \{0,1\}//'; exit 64; }
[ $# -ge 1 ] || usage
CMD="$1"; shift || true

container() { fleet_mt_container_configured; }

# The mark's path comes from the module that READS it, never derived here: a writer and a reader
# with independent copies of that rule is how a rollback ends up recorded at one path, looked for
# at another, and reported as still-VERIFIED by `ironworks doctor`. A FUNCTION, not a variable
# set at startup: only `apply` and `rollback` need it, and resolving it up front put a python
# start — and a new way to fail before printing anything — in front of `status` and `usage` too.
# FAILS LOUDLY, because both callers treat the result as a path. `rm -f "$(...)"` on an empty
# substitution exits 0 without `set -euo pipefail` firing, so a missing python3, an
# `agency_paths` import error, or a moved deploy/lib left the DEGRADED mark in place through a
# successful `activate`. `egress_status.evaluate` then forces FAILED with "the boundary was
# deliberately ROLLED BACK" forever, pointing the operator at a problem that no longer exists —
# while the confirmation line printed an empty path.
degraded_mark_path() {
  local p
  p="$(LIB_DIR="$REPO/deploy/lib" python3 -c "
import os, sys
sys.path.insert(0, os.environ['LIB_DIR'])
import egress_status as es
print(es.degraded_mark_path())")" || {
    echo "!! could not resolve the degraded-mark path (deploy/lib/egress_status.py)" >&2
    return 1; }
  [ -n "$p" ] || {
    echo "!! the degraded-mark path resolved empty — refusing to act on it" >&2
    return 1; }
  printf '%s\n' "$p"
}

case "$CMD" in

  status)
    exec "$REPO/deploy/ironworks" egress status "$@"
    ;;

  verify)
    # The probe stamps a PASS against the running image, which is what turns RUNNING into
    # VERIFIED. It refuses to stamp anything it did not actually measure.
    exec "$REPO/deploy/egress/probe-egress.sh" "$(container)"
    ;;

  activate)
    [ "${1:-}" = "--confirm" ] || {
      echo "!! activate RECREATES the runtime container: in-flight turns are lost and every" >&2
      echo "   client group is briefly unserved. Re-run with --confirm once you have read" >&2
      echo "   SECURITY.md and deploy/README.md." >&2
      exit 2; }
    # Preflight: never apply an overlay that does not parse. A half-applied compose leaves the
    # runtime in whatever state the failure stopped at, which is the one outcome with no
    # defined containment.
    if ! docker compose -f "$BASE" -f "$OVERLAY" config -q; then
      echo "!! the merged compose does not validate — refusing to apply it" >&2; exit 1
    fi
    echo "== applying the egress boundary =="
    docker compose -f "$BASE" -f "$OVERLAY" up -d --remove-orphans
    MARK="$(degraded_mark_path)"    # aborts under set -e if it cannot be resolved
    rm -f "$MARK"
    echo
    echo "   applied. The boundary is RUNNING but NOT yet VERIFIED — prove it before believing it:"
    echo "     $0 verify"
    ;;

  rollback)
    # Deliberately awkward. Rolling back does not "restore service", it REMOVES a security
    # control, and the flag is named so that nobody types it by accident or pastes it from a
    # runbook without reading it.
    [ "${1:-}" = "--i-accept-unrestricted-egress" ] || {
      echo "!! rollback REMOVES the network boundary and returns the runtime to unrestricted" >&2
      echo "   public egress. Every tenant's book becomes exfiltratable by a prompt-injected" >&2
      echo "   turn that regains an HTTP capability. If the product is broken, prefer leaving" >&2
      echo "   it broken while you diagnose — that is fail-closed working as designed." >&2
      echo "   To proceed anyway:  $0 rollback --i-accept-unrestricted-egress" >&2
      exit 2; }
    echo "== ROLLING BACK the egress boundary — the runtime regains unrestricted egress =="
    # Resolved BEFORE the rollback and into a variable, so a resolver failure aborts here rather
    # than printing an empty path in the confirmation. A command substitution inside `echo` does
    # not trip `set -e` — echo's own status is what the shell sees — so the operator would have
    # been told the degraded state was "recorded at " and nothing else.
    MARK="$(degraded_mark_path)"
    docker compose -f "$BASE" up -d --remove-orphans
    # Recorded, not merely logged: a degraded security state that nobody is tracking is the
    # same as one nobody knows about. `ironworks doctor` reads this file, so the module that
    # reads it also writes it — atomically, 0600, in the one shape `evaluate()` expects.
    LIB_DIR="$REPO/deploy/lib" CONTAINER="$(container)" python3 -c "
import os, sys
sys.path.insert(0, os.environ['LIB_DIR'])
import egress_status as es
es.write_degraded_mark(os.environ['CONTAINER'],
                       'the network egress boundary was deliberately removed; '
                       'the runtime has unrestricted public egress')"
    echo
    echo "   rolled back. A DEGRADED security state is recorded at $MARK and"
    echo "   ./deploy/ironworks doctor will report it until the boundary is re-applied."
    ;;

  *) usage ;;
esac
