# deploy/account-intel/data/smoke.sh — the bring-up smoke ASSERTIONS for the Account Store.
#
# Sourced by prod-up.sh, dev-up.sh and seed-real.sh, all of which already source
# ../../lib/fleet.sh (which brings curl_header in with it).
#
# WHY THIS FILE EXISTS. All three scripts used to PRINT what they measured and compare
# nothing: `curl -w '%{http_code}\n'` echoed 200 where 401 was required and the script still
# exited 0. Those lines are the only checks that the production store is fail-closed, that the
# well-known dev token is dead there, and that one org cannot read another's accounts — so a
# check that cannot fail is the same as no check. Each helper returns non-zero on mismatch,
# which under the callers' `set -euo pipefail` ends the run.
#
# One copy, because all three are answering the same question and two copies can disagree
# about what a passing answer looks like.
#
# shellcheck shell=bash

SMOKE_BASE="${SMOKE_BASE:-http://127.0.0.1:8443}"

# smoke_code <label> <want-code> <secret-header|""> <path> [curl args…] — assert an HTTP status.
#
# `|| true` and a `${…:-000}` default, never `|| echo 000`: on a refused connection curl exits
# NON-ZERO (which would abort the assignment under `set -e`) and still writes `000` itself, so
# appending another would produce the literal `000000`, matching no expectation for the wrong
# reason. The secret header goes through curl_header to stay off argv; "" skips it, which is
# what the no-token check needs.
smoke_code() {
  local _label="$1" _want="$2" _hdr="$3" _path="$4" _got; shift 4
  if [ -n "$_hdr" ]; then
    _got="$(curl_header "$_hdr" -s -o /dev/null -w '%{http_code}' -m 15 "$@" "$SMOKE_BASE$_path" || true)"
  else
    _got="$(curl -s -o /dev/null -w '%{http_code}' -m 15 "$@" "$SMOKE_BASE$_path" || true)"
  fi
  _got="${_got:-000}"
  [ "$_got" = "$_want" ] || {
    echo "!! FAIL  $_label -> HTTP $_got (want $_want)" >&2
    [ "$_got" = 000 ] && echo "         000 means the request never arrived — nothing was proved." >&2
    return 1; }
  echo "   ok    $_label -> HTTP $_got"
}

# smoke_matches <label> <want> <secret-header> <query> [curl args…] — assert find_account's
# match_count for one caller. <want> is an exact count (`0`, `1`) or a floor (`>=1`), because
# the two callers can know different things: the dev fixtures pin an exact number, while
# seed-real.sh queries a word out of the operator's own data and cannot know how many accounts
# share it. A floor still fails the case that matters — the org reading back nothing.
#
# THE PIPEFAIL HAZARD, stated because this repo has been bitten by it: a `$(… | python3 …)`
# whose right-hand side fails aborts the assignment under `set -euo pipefail` — so an error
# body (`{"error":"unauthorized"}`) would kill the script AT the assignment, with no verdict
# printed and no indication which check died. `|| true` keeps it alive and the emptiness test
# below IS the handler; it must stay reachable. `d['match_count']` rather than `.get`,
# deliberately: a missing key must land in that handler and print the body, not print the
# string "None" and be compared against a count.
smoke_matches() {
  local _label="$1" _want="$2" _hdr="$3" _q="$4" _body _got; shift 4
  _body="$(curl_header "$_hdr" -s -m 15 "$@" "$SMOKE_BASE/find_account?query=$_q" || true)"
  _got="$(printf '%s' "$_body" | fleet_json "d['match_count']" 2>/dev/null || true)"
  [ -n "$_got" ] || {
    echo "!! FAIL  $_label -> no match_count in the response: ${_body:-(empty — the request never arrived)}" >&2
    return 1; }
  case "$_want" in
    ">="*) [ "$_got" -ge "${_want#>=}" ] || {
             echo "!! FAIL  $_label -> match_count $_got (want at least ${_want#>=})" >&2; return 1; } ;;
    *)     [ "$_got" = "$_want" ] || {
             echo "!! FAIL  $_label -> match_count $_got (want $_want)" >&2; return 1; } ;;
  esac
  echo "   ok    $_label -> match_count $_got"
}
