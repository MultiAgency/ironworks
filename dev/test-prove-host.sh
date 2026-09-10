#!/usr/bin/env bash
# COST: free — offline fixture controls through a fake SSH; no host, no credentials.
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

remote_script="$(cat)"
case "$remote_script" in
  *'registry="/var/lib/ironworks/$slug/registry.json"'*) ;;
  *) exit 5 ;;
esac
case "$remote_script" in
  *'relay_env="/etc/ironworks/relay-$slug.env"'*) ;;
  *) exit 6 ;;
esac
case "$remote_script" in
  *'cell_user="ironworks-$slug"'*) ;;
  *) exit 7 ;;
esac
case "$remote_script" in
  *'attestation="/run/ironworks-attest/$slug/current.json"'*) ;;
  *) exit 8 ;;
esac
# The walk cases substitute a fixture tree for this one constant, so the leg itself is held to it.
case "$remote_script" in
  *'deploy_tree=/opt/ironworks'*) ;;
  *) exit 10 ;;
esac

printf 'CONTROL\t%s\n' "$nonce"
printf 'UNIT\tironworks-telegram@relationship-demo.service\tloaded\tactive\tenabled\n'
printf 'UNIT\tironworks-relay@relationship-demo.service\tloaded\tactive\tenabled\n'
printf 'UNIT\tironworks-ironclaw@relationship-demo.service\tloaded\tactive\tenabled\n'
printf 'UNIT\tironworks-devstore.service\tloaded\tinactive\tdisabled\n'
# Check 12. `stopped` and `disabled` are the two ways the timer stops refreshing, now or at the next
# boot; either leaves the relay refusing every new turn within ten minutes.
case "${FAKE_TIMER_CASE:-healthy}" in
  healthy)  printf 'UNIT\tironworks-attest@relationship-demo.timer\tloaded\tactive\tenabled\n' ;;
  stopped)  printf 'UNIT\tironworks-attest@relationship-demo.timer\tloaded\tinactive\tenabled\n' ;;
  disabled) printf 'UNIT\tironworks-attest@relationship-demo.timer\tloaded\tactive\tdisabled\n' ;;
esac
# Check 13. `fresh` and `expired` straddle the relay's own boundary — valid while younger than
# `VALIDITY_SECONDS`, expired at exactly it — so the scorer cannot be off by one in either direction
# and still pass both.
case "${FAKE_ATTEST_CASE:-fresh}" in
  fresh)   printf 'ATTEST\tread\trelationship-demo\t520144ef-c36a-4606-a885-62e553c56ce7\t1000000\t1000599\n' ;;
  expired) printf 'ATTEST\tread\trelationship-demo\t520144ef-c36a-4606-a885-62e553c56ce7\t1000000\t1000600\n' ;;
  absent)  printf 'ATTEST\tabsent\n' ;;
  unread)  printf 'ATTEST\tunread\n' ;;
esac
printf 'REGISTRY\t5c62e5168c417c8c\n'
printf 'BINDING\trelationship-demo\trelationship-intelligence\t/etc/ironworks/relationship-demo/guidance.md\n'
printf 'COMPOSE_RC\t0\n'
printf 'COMPOSE_BEGIN\t%s\n' "$nonce"
case "${FAKE_GUIDANCE_CASE:-clean}" in
  clean)
    printf 'service relationship-intelligence\n'
    printf 'guidance 1604 bytes (3cf2240fd1fe92d5)\n'
    ;;
  leaked)
    printf 'compose: WARNING model-visible guidance holds a comment-closing delimiter\n'
    ;;
  stubbed)
    printf 'compose: WARNING model-visible guidance holds 2 unwritten template stub(s)\n'
    ;;
esac
printf 'COMPOSE_END\t%s\n' "$nonce"
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

# The positive control for check 10. Without a case that emits an agreeing policy, that check could
# only ever be BLOCKED here, and a check whose controls never let it pass proves nothing (K-8).
printf 'MODELPOLICY_BEGIN\t%s\n' "$nonce"
policy_rc=0
case "${FAKE_POLICY_CASE:-matching}" in
  matching)
    printf 'requested-model policy matches MODEL_PIN exactly for qwen/qwen3.5-397b-a17b; effective provider model and TEE execution are not verified by this check\n'
    ;;
  drifted)
    printf 'ironworks: requested-model policy disagrees with MODEL_PIN (selection_enabled=false, default_matches=false, exact_allowlist=false, allowed_count=0)\n'
    policy_rc=2
    ;;
  # Exit 0 while naming a model that is not this tree's pin. The message-grep scorer passed this,
  # because "matches MODEL_PIN exactly" is present and it never compared the name to anything.
  wrong-pin)
    printf 'requested-model policy matches MODEL_PIN exactly for someone/else-4b; effective provider model and TEE execution are not verified by this check\n'
    ;;
  unreadable) policy_rc=127 ;;
esac
printf 'MODELPOLICY_END\t%s\n' "$nonce"
printf 'MODELPOLICYRC\t%s\t%s\n' "$nonce" "$policy_rc"

# Check 11. The canned cases exercise the scorer. `unscannable` emits no PRIVSCANNED row at all,
# which is the case that separates "this host runs nothing as root" from "no unit could be read" —
# without it the check would pass on silence, which is the §K-8 defect it exists to avoid.
#
# The `walk-*` cases are not canned. They run the remote leg's own `privileged_scan` and deployment
# tree scan, cut from the script exactly as transmitted, against the stub `systemctl`, `stat` and
# `find` in FAKE_WALK_DIR over real fixture files — so the rows scored are the rows that code emits,
# and a regression in which units it reads, which commands it counts as root, or which tree it
# scans reaches the verdict. The deployment tree is the one constant substituted, with the fixture
# tree; the guard above holds the transmitted leg to `/opt/ironworks`.
case "${FAKE_PRIV_CASE:-clean}" in
  clean)
    for u in ironworks-relay ironworks-telegram ironworks-ironclaw; do
      printf 'PRIVSCANNED\t%s@relationship-demo.service\n' "$u"
      printf 'PRIVPATH\t%s@relationship-demo.service\t/opt/ironworks/deploy/check-serving-cell.sh\t\n' "$u"
    done
    printf 'PRIVSCANNED\tironworks-attest@relationship-demo.service\n'
    printf 'PRIVPATH\tironworks-attest@relationship-demo.service\t/opt/ironworks/deploy/refresh-attestation.sh\t\n'
    printf 'PRIVTREE\t/opt/ironworks\tscanned\t0\n'
    ;;
  writable)
    for u in ironworks-relay ironworks-telegram ironworks-ironclaw ironworks-attest; do
      printf 'PRIVSCANNED\t%s@relationship-demo.service\n' "$u"
    done
    printf 'PRIVPATH\tironworks-relay@relationship-demo.service\t/opt/ironworks/deploy/check-serving-cell.sh\t\n'
    printf 'PRIVPATH\tironworks-telegram@relationship-demo.service\t/opt/ironworks/deploy/check-serving-cell.sh\t/opt/ironworks/deploy is owned by uid 1000\n'
    printf 'PRIVTREE\t/opt/ironworks\tscanned\t0\n'
    ;;
  unscannable) : ;;
  walk-*)
    walker="$(printf '%s\n' "$remote_script" \
      | sed -n '/^privileged_scan() {$/,/^# end of the root-execution scan/p')"
    [ -n "$walker" ] || exit 9
    case_dir="$FAKE_WALK_DIR/$FAKE_PRIV_CASE"
    printf 'slug=relationship-demo\ndeploy_tree=%s\n%s\n' "$FAKE_WALK_DIR/fs/opt/ironworks" "$walker" \
      | PATH="$FAKE_WALK_DIR/bin:$PATH" FAKE_CASE_DIR="$case_dir" FAKE_UNITS="$case_dir/units" \
          FAKE_OWNERS="$case_dir/owners" bash
    ;;
esac
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
  'PASS      guidance.publication_gate' \
  'PASS      unit.ironworks-attest@relationship-demo.timer' \
  'PASS      attestation.current' \
  '13 passed, 0 failed, 0 blocked'
run_case blocked blocked 3 \
  'BLOCKED   member.bound' \
  'live member binding could not be established' \
  'BLOCKED   confinement.current_policy' \
  '11 passed, 0 failed, 2 blocked'
# A binding failure stops confinement from being measured at all, so this run carries a FAIL *and*
# a BLOCKED — and blocked outranks failed (`dev/lib.sh::summary`, §F-14). Exit 3 is the answer the
# operator has to see first: some of this was never measured. This case expected 2 while asserting
# `1 blocked` in the same breath, because `dev/prove-host.sh` used to rank the two the other way
# round and `dev/lib.sh` corrected it. `different` below is the clean FAIL that still exits 2.
run_case failed failed 3 \
  'FAIL      member.bound' \
  'inspection reported a binding failure for recorded member' \
  'BLOCKED   confinement.current_policy' \
  'member binding failed before confinement was measured' \
  '11 passed, 1 failed, 1 blocked'
run_case different different 2 \
  'FAIL      member.bound' \
  'inspection bound a member other than recorded member' \
  '11 passed, 2 failed, 0 blocked'

# Two positive controls for the negative live-guidance assertion. A scorer that merely accepts
# every compose report would pass the clean case above and fail both of these controls.
for guidance_case in leaked stubbed; do
  hazard_output="$(FAKE_INSPECT_CASE=matching FAKE_GUIDANCE_CASE="$guidance_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  hazard_rc=$?
  if [ "$hazard_rc" -eq 2 ] \
     && grep -Fq 'FAIL      guidance.publication_gate' <<<"$hazard_output" \
     && grep -Fq 'live guidance contains a refused publication hazard' <<<"$hazard_output"; then
    printf 'PASS guidance-%s-positive-control\n' "$guidance_case"
  else
    printf 'FAIL guidance-%s-positive-control: exit %s\n%s\n' \
      "$guidance_case" "$hazard_rc" "$hazard_output"
    failures=$((failures + 1))
  fi
done

# The other half of check 10. The case above proves it can pass; these prove it can fail and can
# report not-measured — a scorer that accepted any policy output would pass all three. `drifted` is
# the state the served host was actually found in on 2026-09-07, and `unreadable` is what an absent
# operator credential looks like, which must block rather than pass.
for policy_case in drifted unreadable wrong-pin; do
  case "$policy_case" in
    drifted)    want_rc=2; want_line='FAIL      model.policy_matches_pin' ;;
    unreadable) want_rc=3; want_line='BLOCKED   model.policy_matches_pin' ;;
    # The control for the clause that compares the named model against this tree's pin. A scorer
    # that only greps the success sentence passes this case, which is why it is here.
    wrong-pin)  want_rc=2; want_line='FAIL      model.policy_matches_pin' ;;
  esac
  policy_output="$(FAKE_INSPECT_CASE=matching FAKE_POLICY_CASE="$policy_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  policy_rc=$?
  if [ "$policy_rc" -eq "$want_rc" ] && grep -Fq "$want_line" <<<"$policy_output"; then
    printf 'PASS model-policy-%s-control\n' "$policy_case"
  else
    printf 'FAIL model-policy-%s-control: exit %s\n%s\n' "$policy_case" "$policy_rc" "$policy_output"
    failures=$((failures + 1))
  fi
done

# The other half of check 11. `writable` is the state the served host was actually found in on
# 2026-09-07 — an `ExecStartPre=+` script whose directory the login account owned, executed as root
# at every restart — and `unscannable` is a host whose units could not be read, which must block
# rather than pass. A scorer that accepted any ownership report would pass the clean case and both
# of these.
for priv_case in writable unscannable; do
  case "$priv_case" in
    writable)    want_rc=2; want_line='FAIL      privilege.root_execution_paths' ;;
    unscannable) want_rc=3; want_line='BLOCKED   privilege.root_execution_paths' ;;
  esac
  priv_output="$(FAKE_INSPECT_CASE=matching FAKE_PRIV_CASE="$priv_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  priv_rc=$?
  if [ "$priv_rc" -eq "$want_rc" ] && grep -Fq "$want_line" <<<"$priv_output"; then
    printf 'PASS privilege-%s-control\n' "$priv_case"
  else
    printf 'FAIL privilege-%s-control: exit %s\n%s\n' "$priv_case" "$priv_rc" "$priv_output"
    failures=$((failures + 1))
  fi
done

# Check 11 through the real walker and tree scan. Three fixture units mirror the served ones — cell
# accounts, one `+` preflight each, and IronClaw's two unprefixed preflights that must not be
# walked — and the attestation service varies.
#
# The walk: `User=root` over root-owned ancestors is the positive control; `User=root`, or no
# `User=` at all, under a login-owned directory must FAIL; a cell account with no `+` must not be
# walked whatever sits above its script; and an attestation service systemd cannot show must
# BLOCK. Those scripts sit outside the deployment tree, under `srv/`, so the tree scan cannot see
# them and a FAIL can only be that unit being walked: the `+`-only scan this replaced passed
# `walk-root-writable`.
#
# The tree: a clean tree with a deep root-owned `target/release/ironworks` — named by no unit line,
# executed by the attestation script — is the positive control; that binary owned by the login uid,
# and a setgid group-writable directory deep in `target/`, must FAIL; a tree `find` cannot read must
# BLOCK; and a root-run path outside the tree must not make its own top directory a scan.
walk="$scratch/walk"
fs="$walk/fs"
mkdir -p "$walk/bin" "$fs/opt/ironworks/deploy" "$fs/opt/ironworks/attest" "$fs/opt/ironworks/dev" \
  "$fs/opt/ironworks/target/release/deps" "$fs/opt/ironclaw/bin" "$fs/srv/attest" "$fs/srv/other"
for fixture_file in deploy/check-serving-cell.sh deploy/check-ironclaw-instance.sh \
    deploy/check-ironclaw-artifacts.sh attest/refresh-attestation.sh dev/relay-loop.sh \
    target/release/ironworks-telegram target/release/ironworks target/release/deps/libfixture.rlib; do
  : > "$fs/opt/ironworks/$fixture_file"
done
: > "$fs/opt/ironclaw/bin/ironclaw"
: > "$fs/srv/attest/refresh-attestation.sh"
: > "$fs/srv/other/login-owned"

cat >"$walk/bin/systemctl" <<'STUB'
#!/usr/bin/env bash
# `systemctl cat UNIT` and `systemctl show UNIT --property=NAME --value`, answered from FAKE_UNITS.
# An absent unit fails `cat` and shows an empty value, as systemd does.
case "$1" in
  cat)  [ -f "$FAKE_UNITS/$2" ] && cat "$FAKE_UNITS/$2" ;;
  show) f="$FAKE_UNITS/$2.${3#--property=}"; [ -f "$f" ] && cat "$f"; exit 0 ;;
  *)    exit 2 ;;
esac
STUB
cat >"$walk/bin/stat" <<'STUB'
#!/usr/bin/env bash
# `stat -c %u|%a PATH` for a path that exists: root-owned 0755 unless FAKE_OWNERS names it.
[ "$1" = -c ] && [ -e "$3" ] || exit 1
row="$(awk -F '\t' -v p="$3" '$1 == p { print $2 " " $3; exit }' "$FAKE_OWNERS" 2>/dev/null)"
read -r uid mode <<<"${row:-0 755}"
case "$2" in %u) echo "$uid" ;; %a) echo "$mode" ;; *) exit 2 ;; esac
STUB
cat >"$walk/bin/find" <<'STUB'
#!/usr/bin/env bash
# `find -P ROOT -xdev -printf '%U\t%m\t%y\t%p\n'` over the fixture tree: the system find lists it,
# and uid and mode come from FAKE_OWNERS (root and 0755 otherwise). It only lists, as the real
# command is asked to, so the offender rule under test is the remote leg's own. Every ROOT asked
# for is logged, which is how a test sees which trees were scanned.
[ "$#" -eq 5 ] && [ "$1" = -P ] && [ "$3" = -xdev ] && [ "$4" = -printf ] \
  && [ "$5" = '%U\t%m\t%y\t%p\n' ] || exit 64
printf '%s\n' "$2" >> "$FAKE_CASE_DIR/find-roots"
if [ -e "$FAKE_CASE_DIR/find-fails" ]; then
  printf '0\t755\td\t%s\n' "$2"
  echo "find: $2/target: Permission denied" >&2
  exit 1
fi
/usr/bin/find "$2" | while IFS= read -r path; do
  row="$(awk -F '\t' -v p="$path" '$1 == p { print $2 " " $3; exit }' "$FAKE_OWNERS")"
  read -r uid mode <<<"${row:-0 755}"
  if [ -L "$path" ]; then type=l; elif [ -d "$path" ]; then type=d; else type=f; fi
  printf '%s\t%s\t%s\t%s\n' "$uid" "$mode" "$type" "$path"
done
STUB
chmod 0755 "$walk/bin/systemctl" "$walk/bin/stat" "$walk/bin/find"

walk_fixture() { # walk_fixture CASE ATTEST_USER|unset|absent [ATTEST_SCRIPT]
  local units="$walk/$1/units" cell
  local attest_script="${3:-$fs/opt/ironworks/attest/refresh-attestation.sh}"
  mkdir -p "$units"
  for cell in telegram relay; do
    printf 'ExecStartPre=+%s/opt/ironworks/deploy/check-serving-cell.sh %%i\nUser=ironworks-%%i\n' \
      "$fs" > "$units/ironworks-$cell@relationship-demo.service"
    printf 'ironworks-relationship-demo\n' > "$units/ironworks-$cell@relationship-demo.service.User"
  done
  printf 'ExecStart=%s/opt/ironworks/target/release/ironworks-telegram --port 3994\n' "$fs" \
    >> "$units/ironworks-telegram@relationship-demo.service"
  printf 'ExecStart=%s/opt/ironworks/dev/relay-loop.sh \\\n    --registry /var/lib/ironworks/%%i/registry.json\n' \
    "$fs" >> "$units/ironworks-relay@relationship-demo.service"
  {
    printf 'ExecStartPre=%s/opt/ironworks/deploy/check-ironclaw-instance.sh %%i\n' "$fs"
    printf 'ExecStartPre=+%s/opt/ironworks/deploy/check-serving-cell.sh %%i\n' "$fs"
    printf 'ExecStartPre=%s/opt/ironworks/deploy/check-ironclaw-artifacts.sh\n' "$fs"
    printf 'ExecStart=%s/opt/ironclaw/bin/ironclaw serve --host 127.0.0.1\n' "$fs"
    printf 'User=ironclaw-%%i\n'
  } > "$units/ironworks-ironclaw@relationship-demo.service"
  printf 'ironclaw-relationship-demo\n' > "$units/ironworks-ironclaw@relationship-demo.service.User"
  if [ "$2" != absent ]; then
    printf 'ExecStart=%s %%i\n' "$attest_script" > "$units/ironworks-attest@relationship-demo.service"
    [ "$2" = unset ] || printf '%s\n' "$2" > "$units/ironworks-attest@relationship-demo.service.User"
  fi
  printf 'ironworks-attest@relationship-demo.service\n' \
    > "$units/ironworks-attest@relationship-demo.timer.Triggers"
  : > "$walk/$1/owners"
}
owned_by() { # owned_by CASE PATH UID MODE — one fixture ownership row; root and 0755 otherwise
  printf '%s\t%s\t%s\n' "$2" "$3" "$4" >> "$walk/$1/owners"
}
outside="$fs/srv/attest/refresh-attestation.sh"
walk_fixture walk-root-clean root
walk_fixture walk-root-writable root "$outside"
owned_by walk-root-writable "$fs/srv/attest" 1000 755
walk_fixture walk-unset-writable unset "$outside"
owned_by walk-unset-writable "$fs/srv/attest" 1000 755
walk_fixture walk-cell-user ironworks-relationship-demo "$outside"
owned_by walk-cell-user "$fs/srv/attest" 1000 755
walk_fixture walk-attest-unread absent
walk_fixture walk-tree-binary root
owned_by walk-tree-binary "$fs/opt/ironworks/target/release/ironworks" 1000 755
walk_fixture walk-tree-group-writable root
owned_by walk-tree-group-writable "$fs/opt/ironworks/target/release/deps" 0 2775
walk_fixture walk-tree-unreadable root
: > "$walk/walk-tree-unreadable/find-fails"
walk_fixture walk-outside-tree root "$outside"
owned_by walk-outside-tree "$fs/srv/other/login-owned" 1000 644

export FAKE_WALK_DIR="$walk"
for walk_case in walk-root-clean walk-cell-user walk-root-writable walk-unset-writable \
                 walk-attest-unread walk-tree-binary walk-tree-group-writable \
                 walk-tree-unreadable walk-outside-tree; do
  want_more=""
  case "$walk_case" in
    # Three `+` preflights and the attestation script; IronClaw's unprefixed lines are not walked.
    walk-root-clean)
      want_rc=0; want_line='4 path(s) run as root are root-owned'
      want_more='and every object under' ;;
    # The same three, and not the cell-account script under a login-owned directory.
    walk-cell-user)    want_rc=0; want_line='3 path(s) run as root are root-owned' ;;
    walk-outside-tree) want_rc=0; want_line='4 path(s) run as root are root-owned' ;;
    walk-root-writable|walk-unset-writable)
      want_rc=2; want_line='FAIL      privilege.root_execution_paths' ;;
    walk-tree-binary)
      want_rc=2; want_line='1 object(s) under'
      want_more='target/release/ironworks: owned by uid 1000' ;;
    walk-tree-group-writable)
      want_rc=2; want_line='1 object(s) under'; want_more='target/release/deps: mode 2775' ;;
    walk-attest-unread)
      want_rc=3; want_line='not read, so not walked: ironworks-attest@relationship-demo.service' ;;
    walk-tree-unreadable) want_rc=3; want_line='could not be read in full' ;;
  esac
  walk_output="$(FAKE_INSPECT_CASE=matching FAKE_PRIV_CASE="$walk_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  walk_rc=$?
  if [ "$walk_rc" -eq "$want_rc" ] && grep -Fq "$want_line" <<<"$walk_output" \
     && { [ -z "$want_more" ] || grep -Fq "$want_more" <<<"$walk_output"; }; then
    printf 'PASS privilege-%s-control\n' "$walk_case"
  else
    printf 'FAIL privilege-%s-control: exit %s\n%s\n' "$walk_case" "$walk_rc" "$walk_output"
    failures=$((failures + 1))
  fi
done

# The deployment tree is the only tree scanned. A root-run path outside it names a directory of its
# own, and scanning that as well would turn one root line naming a system binary into a scan of
# `/usr`. The login-owned file beside the outside script is what such a scan would have failed on.
scanned_roots="$(cat "$walk/walk-outside-tree/find-roots" 2>/dev/null)"
if [ "$scanned_roots" = "$fs/opt/ironworks" ]; then
  printf 'PASS privilege-outside-path-scans-only-the-deployment-tree\n'
else
  printf 'FAIL privilege-outside-path-scans-only-the-deployment-tree: scanned %s\n' \
    "${scanned_roots:-nothing}"
  failures=$((failures + 1))
fi

# The other halves of checks 12 and 13. The healthy timer and fresh attestation in `matching` are
# their positive controls; these prove each can fail, and that an attestation nothing could read
# blocks rather than passes. A timer that stopped, or will not start at the next boot, lets the
# evidence expire and every new turn be refused while the other eleven checks still pass.
for timer_case in stopped disabled; do
  timer_output="$(FAKE_INSPECT_CASE=matching FAKE_TIMER_CASE="$timer_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  timer_rc=$?
  if [ "$timer_rc" -eq 2 ] \
     && grep -Fq 'FAIL      unit.ironworks-attest@relationship-demo.timer' <<<"$timer_output"; then
    printf 'PASS attest-timer-%s-control\n' "$timer_case"
  else
    printf 'FAIL attest-timer-%s-control: exit %s\n%s\n' "$timer_case" "$timer_rc" "$timer_output"
    failures=$((failures + 1))
  fi
done
for attest_case in expired absent unread; do
  case "$attest_case" in
    expired|absent) want_rc=2; want_line='FAIL      attestation.current' ;;
    unread)         want_rc=3; want_line='BLOCKED   attestation.current' ;;
  esac
  attest_output="$(FAKE_INSPECT_CASE=matching FAKE_ATTEST_CASE="$attest_case" \
    PROVE_HOST_SSH="$fake_ssh" "$root/dev/prove-host.sh" 2>&1)"
  attest_rc=$?
  if [ "$attest_rc" -eq "$want_rc" ] && grep -Fq "$want_line" <<<"$attest_output"; then
    printf 'PASS attestation-%s-control\n' "$attest_case"
  else
    printf 'FAIL attestation-%s-control: exit %s\n%s\n' "$attest_case" "$attest_rc" "$attest_output"
    failures=$((failures + 1))
  fi
done

# The fake refuses a remote leg that does not read `/run/ironworks-attest/$slug/current.json`; this
# holds the relay unit to the same file. Two spellings of one path would let check 13 pass on a file
# the relay never opens.
if grep -Fq -- '--confinement-attestation /run/ironworks-attest/%i/current.json' \
     "$root/deploy/ironworks-relay@.service"; then
  printf 'PASS attestation-path-matches-relay\n'
else
  printf 'FAIL attestation-path-matches-relay: the relay unit requires a different file\n'
  failures=$((failures + 1))
fi

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
