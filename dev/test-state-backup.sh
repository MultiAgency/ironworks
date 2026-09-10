#!/usr/bin/env bash
# COST: free — offline fixture controls through a fake restic and verifier; no repository, no host.
# Verdict controls for `deploy/check-state-backup.sh` and `deploy/rehearse-state-restore.sh`.
#
# Each script's PASS is the positive control for its refusals: a fake that could never pass would
# confirm every negative case whatever the script did. Every BLOCKED case here is one the scripts
# used to score 2 — a corrupt backup — when nothing had been measured.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/state-backup-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
check="$root/deploy/check-state-backup.sh"
rehearse="$root/deploy/rehearse-state-restore.sh"

# Three PATH components, so a case can remove exactly one dependency: `base` is everything else
# the scripts and fakes call, `py` is python3, `fake` is restic.
mkdir "$tmp/base" "$tmp/py" "$tmp/fake" "$tmp/scratch"
for tool in bash cat find grep mkdir mktemp rm tail; do
  real="$(command -v "$tool")" || { echo "test-state-backup: needs $tool" >&2; exit 1; }
  ln -s "$real" "$tmp/base/$tool"
done
# The interpreter itself rather than whatever shim resolves it, since a shim needs its own PATH.
python="$(python3 -c 'import sys; print(sys.executable)')" \
  || { echo "test-state-backup: needs python3" >&2; exit 1; }
printf '#!/bin/sh\nexec '\''%s'\'' "$@"\n' "$python" > "$tmp/py/python3"
chmod 0755 "$tmp/py/python3"

# Refuses every invocation but the two the scripts make, so a regression in how they call restic
# fails the positive controls rather than passing through them. The wording is fixture text: only
# the exit status and the transport phrase are scored.
cat >"$tmp/fake/restic" <<'FAKE'
#!/usr/bin/env bash
[ "$#" -ge 5 ] && [ "$1 $2 $3" = "--repo fixture:repository --password-file" ] && [ -r "$4" ] \
  || exit 99
shift 4
case "${FAKE_RESTIC:-ok}" in
  unreachable) echo 'Fatal: dial tcp 198.51.100.7:443: connect: connection refused'; exit 1 ;;
  locked) echo 'Fatal: repository is already locked'; exit 11 ;;
  corrupt) echo 'Fatal: repository contains errors'; exit 1 ;;
  password) echo 'Fatal: wrong password or no key found'; exit 12 ;;
esac
if [ "$*" = check ]; then
  echo 'no errors were found'
  exit 0
fi
[ "$#" -eq 6 ] && [ "$1 $2 $3 $4 $5" = "restore latest --tag ironworks-state-v2 --target" ] \
  || exit 98
case "${FAKE_RESTORE:-one}" in
  one) mkdir -p "$6/staging" && echo '{}' > "$6/staging/snapshot-manifest.json" ;;
  two) for d in a b; do mkdir -p "$6/$d" && echo '{}' > "$6/$d/snapshot-manifest.json"; done ;;
  none) mkdir -p "$6/staging" ;;
esac
FAKE
chmod 0755 "$tmp/fake/restic"

# The verifier accepts only the invocation the rehearsal makes, on a manifest that exists.
cat >"$tmp/verifier" <<'FAKE'
#!/usr/bin/env bash
[ "$#" -eq 4 ] && [ "$1 $2 $3" = "state verify-snapshot --manifest" ] && [ -f "$4" ] || exit 64
exit "${FAKE_VERIFY:-0}"
FAKE
chmod 0755 "$tmp/verifier"

printf 'fixture\n' > "$tmp/password"
printf '{"repository": "fixture:repository", "password_file": "%s"}\n' "$tmp/password" \
  > "$tmp/backup.json"
printf '{"repository": "fixture:repository", "password_file": "%s"}\n' "$tmp/absent-password" \
  > "$tmp/lost-password.json"
printf '{"repository": "fixture:repository"}\n' > "$tmp/no-password.json"
printf 'not json\n' > "$tmp/garbled.json"

full="$tmp/fake:$tmp/py:$tmp/base"
failures=0
expect() { # expect NAME WANT_EXIT COMMAND...
  local name="$1" want="$2" output rc
  shift 2
  output="$("$@" 2>&1)"
  rc=$?
  if [ "$rc" -eq "$want" ]; then
    printf 'PASS %s\n' "$name"
  else
    printf 'FAIL %s: exit %s, expected %s\n%s\n' "$name" "$rc" "$want" "$output"
    failures=$((failures + 1))
  fi
}

expect "check: a clean repository passes" 0 \
  env PATH="$full" "$check" "$tmp/backup.json"
expect "check: a corrupt repository fails" 2 \
  env PATH="$full" FAKE_RESTIC=corrupt "$check" "$tmp/backup.json"
expect "check: a refused password fails" 2 \
  env PATH="$full" FAKE_RESTIC=password "$check" "$tmp/backup.json"
expect "check: an unreachable repository blocks" 3 \
  env PATH="$full" FAKE_RESTIC=unreachable "$check" "$tmp/backup.json"
expect "check: a locked repository blocks" 3 \
  env PATH="$full" FAKE_RESTIC=locked "$check" "$tmp/backup.json"
expect "check: no restic blocks" 3 \
  env PATH="$tmp/py:$tmp/base" "$check" "$tmp/backup.json"
expect "check: no python3 blocks" 3 \
  env PATH="$tmp/fake:$tmp/base" "$check" "$tmp/backup.json"
expect "check: an absent configuration blocks" 3 \
  env PATH="$full" "$check" "$tmp/absent.json"
expect "check: a configuration that is not JSON blocks" 3 \
  env PATH="$full" "$check" "$tmp/garbled.json"
expect "check: a configuration naming no password file blocks" 3 \
  env PATH="$full" "$check" "$tmp/no-password.json"
expect "check: an unreadable password file blocks" 3 \
  env PATH="$full" "$check" "$tmp/lost-password.json"

expect "rehearse: a verified restore passes" 0 \
  env PATH="$full" "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a restore the verifier refuses fails" 2 \
  env PATH="$full" FAKE_VERIFY=2 "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a verifier that reaches no verdict blocks" 3 \
  env PATH="$full" FAKE_VERIFY=101 "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: an absent verifier blocks" 3 \
  env PATH="$full" "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/absent-verifier"
expect "rehearse: a restore holding no manifest fails" 2 \
  env PATH="$full" FAKE_RESTORE=none "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a restore holding two manifests fails" 2 \
  env PATH="$full" FAKE_RESTORE=two "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a refused restore fails" 2 \
  env PATH="$full" FAKE_RESTIC=corrupt "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: an unreachable repository blocks" 3 \
  env PATH="$full" FAKE_RESTIC=unreachable \
  "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a locked repository blocks" 3 \
  env PATH="$full" FAKE_RESTIC=locked "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: no scratch directory blocks" 3 \
  env PATH="$full" "$rehearse" "$tmp/backup.json" "$tmp/absent-scratch" "$tmp/verifier"
expect "rehearse: no restic blocks" 3 \
  env PATH="$tmp/py:$tmp/base" "$rehearse" "$tmp/backup.json" "$tmp/scratch" "$tmp/verifier"
expect "rehearse: a configuration that is not JSON blocks" 3 \
  env PATH="$full" "$rehearse" "$tmp/garbled.json" "$tmp/scratch" "$tmp/verifier"

# Every run above made its restore directory under `scratch`, and every one must have removed it.
if [ -z "$(ls -A "$tmp/scratch")" ]; then
  printf 'PASS rehearse: no restore is left behind\n'
else
  printf 'FAIL rehearse: restores left behind: %s\n' "$(ls -A "$tmp/scratch")"
  failures=$((failures + 1))
fi

# The rehearsal's production verifier is the binary the other root-run deploy script executes —
# one string, not two that happen to agree today. It once named `/opt/ironworks/bin/`, which
# nothing installs, so a restore that succeeded exited 127.
attest_bin="$(sed -n 's/^bin=//p' "$root/deploy/refresh-attestation.sh")"
rehearse_bin="$(sed -n 's/^bin="\${3:-\(.*\)}"$/\1/p' "$rehearse")"
if [ -n "$attest_bin" ] && [ "$attest_bin" = "$rehearse_bin" ]; then
  printf 'PASS rehearse: the production verifier is %s\n' "$attest_bin"
else
  printf 'FAIL rehearse: verifier %s differs from refresh-attestation.sh %s\n' \
    "${rehearse_bin:-?}" "${attest_bin:-?}"
  failures=$((failures + 1))
fi

[ "$failures" -eq 0 ] || exit 1
