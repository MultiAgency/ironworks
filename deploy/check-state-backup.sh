#!/usr/bin/env bash
# Weekly encrypted-repository integrity check. PASS 0, FAIL 2, BLOCKED 3.
#
# FAIL is a repository that was reached and failed integrity or authentication. BLOCKED is every
# case in which `restic check` never measured it, and each of those is named below rather than
# being whatever is left over — the leftover used to be FAIL, so a host without `restic` installed
# reported a corrupt backup.
set -u
config="${1:-/etc/ironworks/backup.json}"

blocked() { echo "state backup check: BLOCKED — $*" >&2; exit 3; }
failed() { echo "state backup check: FAIL — $*" >&2; exit 2; }

command -v python3 >/dev/null 2>&1 || blocked "python3 is not installed; the configuration cannot be read"
command -v restic >/dev/null 2>&1 || blocked "restic is not installed"
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

# restic's own exit, classified. 11 is its documented "repository is already locked" (`restic check
# --help`): another process holds the repository and nothing was measured. A transport failure is
# recognized by its text — the class `crates/cli/src/state.rs::repository_unavailable` scores
# BLOCKED for an upload.
output="$(restic --repo "$repository" --password-file "$password_file" check 2>&1)"
status=$?
[ "$status" -ne 0 ] || exit 0
[ "$status" -ne 11 ] || blocked "the repository is locked by another process"
if grep -qiE 'connection refused|connection timed out|network is unreachable|no route to host|name resolution|could not resolve host|connection reset|i/o timeout' <<<"$output"; then
  blocked "the repository is unreachable: $(tail -n1 <<<"$output")"
fi
failed "restic check exited $status: $(tail -n1 <<<"$output")"
