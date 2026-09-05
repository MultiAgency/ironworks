#!/usr/bin/env bash
# Regression controls for prove-host's local scoring. No network access and no live credentials.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/prove-host-test.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

fake_ssh="$scratch/ssh"
cat >"$fake_ssh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail

if [ "${1:-}" = -G ]; then
  printf 'hostname 203.0.113.10\nuser operator\n'
  exit 0
fi

nonce=""
for arg in "$@"; do
  case "$arg" in prove-host-*) nonce="$arg" ;; esac
done
[ -n "$nonce" ] || exit 4

printf 'CONTROL\t%s\n' "$nonce"
printf 'UNIT\trustworks-telegram.service\tloaded\tactive\tenabled\n'
printf 'UNIT\trustworks-relay.service\tloaded\tactive\tenabled\n'
printf 'UNIT\trustworks-ironclaw@relationship-demo.service\tloaded\tactive\tenabled\n'
printf 'UNIT\trustworks-devstore.service\tloaded\tinactive\tdisabled\n'
printf 'REGISTRY\ta3f91ef2615d440f\n'
printf 'INSPECT_BEGIN\t%s\n' "$nonce"
case "${FAKE_INSPECT_CASE:-matching}" in
  matching)
    printf 'live: bound as 520144ef-c36a-4606-a885-62e553c56ce7 in tenant relationship-demo.\n'
    printf 'live: 53 catalogued, 2 reachable, approval off — nothing outside policy.\n'
    printf '  SCOPE: about the 53 catalogued settings rows, verified live against a recorded baseline.\n'
    ;;
  blocked)
    printf 'BLOCKED — nothing could be measured now: runtime unavailable.\n'
    ;;
  failed)
    printf 'FAIL — the live credential subject does not match the recorded member.\n'
    ;;
  different)
    printf 'live: bound as another-member in tenant relationship-demo.\n'
    ;;
esac
printf 'INSPECT_END\t%s\n' "$nonce"
FAKE
chmod +x "$fake_ssh"

# The address and user the fake resolver reports, RFC 5737 reserved: the proof has no default for
# either, so every case states them the way an operator must.
export PROVE_HOST_ADDRESS=203.0.113.10 PROVE_HOST_USER=operator

failures=0
run_case() {
  local name="$1" inspect_case="$2" expected_rc="$3"; shift 3
  local output rc pattern
  output="$(FAKE_INSPECT_CASE="$inspect_case" PROVE_HOST_SSH="$fake_ssh" \
    "$root/dev/prove-host.sh" 2>&1)"
  rc=$?
  if [ "$rc" -ne "$expected_rc" ]; then
    printf 'FAIL %s: exit %s, expected %s\n%s\n' "$name" "$rc" "$expected_rc" "$output"
    failures=$((failures + 1))
    return
  fi
  for pattern in "$@"; do
    if ! grep -Fq "$pattern" <<<"$output"; then
      printf 'FAIL %s: missing %s\n%s\n' "$name" "$pattern" "$output"
      failures=$((failures + 1))
      return
    fi
  done
  printf 'PASS %s\n' "$name"
}

run_case matching matching 0 \
  'PASS      member.bound' \
  'recorded member 520144ef-c36a-4606-a885-62e553c56ce7' \
  '8 passed, 0 failed, 0 blocked'
run_case blocked blocked 3 \
  'BLOCKED   member.bound' \
  'live member binding could not be established' \
  'BLOCKED   confinement.current_policy' \
  '6 passed, 0 failed, 2 blocked'
run_case failed failed 2 \
  'FAIL      member.bound' \
  'inspection reported a binding failure for recorded member' \
  'BLOCKED   confinement.current_policy' \
  'member binding failed before confinement was measured' \
  '6 passed, 1 failed, 1 blocked'
run_case different different 2 \
  'FAIL      member.bound' \
  'inspection bound a member other than recorded member'

# The control for the requirement itself. Without an expected route the alias check would compare a
# resolved value against nothing and report a pass, so an unset address must block before any
# connection is attempted — not fall through to a check that cannot fail.
unset_output="$(PROVE_HOST_SSH="$fake_ssh" PROVE_HOST_ADDRESS='' PROVE_HOST_USER='' \
  "$root/dev/prove-host.sh" 2>&1)"
unset_rc=$?
if [ "$unset_rc" -eq 3 ] && grep -Fq 'set PROVE_HOST_ADDRESS and PROVE_HOST_USER' <<<"$unset_output"; then
  printf 'PASS unset-route\n'
else
  printf 'FAIL unset-route: exit %s\n%s\n' "$unset_rc" "$unset_output"
  failures=$((failures + 1))
fi

[ "$failures" -eq 0 ] || exit 1
