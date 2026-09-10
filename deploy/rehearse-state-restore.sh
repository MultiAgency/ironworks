#!/usr/bin/env bash
# Restore the newest encrypted snapshot into disposable space and verify it without live services.
# PASS 0, FAIL 2, BLOCKED 3.
#
#   rehearse-state-restore.sh [CONFIG [SCRATCH_PARENT [VERIFIER]]]
#
# Production passes no arguments. The other two exist so `dev/test-state-backup.sh` can name
# fixtures, the way `check-ironclaw-artifacts.sh` takes its paths.
#
# FAIL is a repository that was reached and refused the restore, or a restore that is corrupt,
# incomplete, wrongly bound or wrongly permissioned — `ironworks state verify-snapshot` exits 2 for
# every one of those. BLOCKED is every case in which that was never measured, each named below
# rather than being whatever is left over: the leftover used to be FAIL, and the verifier's path
# named a directory nothing installs, so a restore that succeeded exited 127.
set -u
config="${1:-/etc/ironworks/backup.json}"
scratch_parent="${2:-/var/lib/ironworks}"
bin="${3:-/opt/ironworks/target/release/ironworks}"

blocked() { echo "restore rehearsal: BLOCKED — $*" >&2; exit 3; }
failed() { echo "restore rehearsal: FAIL — $*" >&2; exit 2; }

command -v python3 >/dev/null 2>&1 || blocked "python3 is not installed; the configuration cannot be read"
command -v restic >/dev/null 2>&1 || blocked "restic is not installed"
# Before the restore, so a missing verifier does not cost a download first.
[ -x "$bin" ] || blocked "the verifier $bin is not installed"
[ -r "$config" ] || blocked "cannot read $config"
settings="$(python3 - "$config" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
values = [d["repository"], d["password_file"]]
if not all(isinstance(v, str) and v and "\n" not in v for v in values):
    sys.exit(1)
print("\n".join(values))
PY
)" || blocked "$config does not name a repository and a password file"
{ IFS= read -r repository; IFS= read -r password_file; } <<<"$settings"
[ -r "$password_file" ] || blocked "cannot read the password file $password_file"

work="$(mktemp -d "$scratch_parent/restore-rehearsal.XXXXXX")" \
  || blocked "no disposable directory could be made under $scratch_parent"
trap 'rm -rf -- "$work"' EXIT

# restic's own exit, classified as `check-state-backup.sh` classifies it.
output="$(restic --repo "$repository" --password-file "$password_file" \
  restore latest --tag ironworks-state-v2 --target "$work" 2>&1)"
status=$?
if [ "$status" -ne 0 ]; then
  [ "$status" -ne 11 ] || blocked "the repository is locked by another process"
  if grep -qiE 'connection refused|connection timed out|network is unreachable|no route to host|name resolution|could not resolve host|connection reset|i/o timeout' <<<"$output"; then
    blocked "the repository is unreachable: $(tail -n1 <<<"$output")"
  fi
  failed "restic restore exited $status: $(tail -n1 <<<"$output")"
fi

manifests="$(find "$work" -type f -name snapshot-manifest.json -print)" \
  || blocked "the restored tree could not be searched"
case "$manifests" in
  "") failed "the restored snapshot holds no snapshot-manifest.json" ;;
  *$'\n'*) failed "the restored snapshot holds more than one snapshot-manifest.json" ;;
esac

# The verifier's exit is mapped, not passed through: 0 and 2 are its verdicts, and anything else —
# a usage refusal, a panic — means it did not reach one.
"$bin" state verify-snapshot --manifest "$manifests"
verified=$?
case "$verified" in
  0) exit 0 ;;
  2) exit 2 ;;
  *) blocked "the verifier exited $verified, which is not a verdict" ;;
esac
