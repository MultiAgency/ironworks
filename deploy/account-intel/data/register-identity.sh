#!/usr/bin/env bash
# Register (or update) one org token in the Account Service's HOT-RELOADED identity file —
# ~/.agency/account-identities/identities.json, read via ACCOUNT_IDENTITIES_FILE (service.py).
# Atomic tmp+rename in the mounted DIRECTORY, then polls until the service resolves the token.
# No restart, no interruption for current clients. Never touches the repo or the container.
#
# The org token is a CLIENT-DATA credential — it is passed via the ORG_TOKEN env var, NOT on the
# command line, so it never appears in the process table (`ps`) on a shared host. The poll
# below likewise keeps the token off curl's argv, via curl_header from deploy/lib/curl-private.sh.
#
# Usage: ORG_TOKEN=<token> register-identity.sh <org_id>     [ACCOUNT_BASE=http://127.0.0.1:8443]
set -euo pipefail
. "$(dirname "$0")/../../lib/curl-private.sh"   # curl_header: keep the org token off argv
ORG="${1:?usage: ORG_TOKEN=<token> register-identity.sh <org_id>}"
TOKEN="${ORG_TOKEN:?set ORG_TOKEN=<token> in the environment (not on argv — it is a secret)}"
ACCOUNT_BASE="${ACCOUNT_BASE:-http://127.0.0.1:8443}"
IDENT_DIR="$HOME/.agency/account-identities"

umask 077
mkdir -p "$IDENT_DIR"
FILE="$IDENT_DIR/identities.json" RT="$TOKEN" RO="$ORG" python3 - <<'PY'
import json, os, sys, tempfile
path = os.environ["FILE"]
try:
    d = json.load(open(path))
except FileNotFoundError:
    d = {}                       # first registration — a genuinely absent file starts empty
except ValueError as e:
    # A CORRUPT existing file is NOT an empty one: defaulting to {} here would rewrite the file
    # with only the new token, silently REVOKING every other client's org token (hot-reloaded ->
    # immediate 401 for all of them). Abort loudly and leave the file untouched.
    sys.exit(f"!! identities file is corrupt ({path}): {e}. Refusing to rewrite — this would wipe "
             "every other client's token. Fix the file by hand, then re-run.")
d[os.environ["RT"]] = os.environ["RO"]
fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path))
os.fchmod(fd, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump(d, f, indent=1)
os.replace(tmp, path)
PY

for _ in $(seq 1 10); do
  # `|| echo 000`: a connection refusal (service down) makes curl exit non-zero, which under
  # `set -e` would kill the script on the FIRST iteration — never retrying, never reaching the
  # "is the data stack running?" diagnostic below (written for exactly this case). Swallow the
  # curl failure into a sentinel code so the loop runs and the diagnostic prints.
  code=$(curl_header "X-Service-Token: $TOKEN" -s -o /dev/null -w '%{http_code}' "$ACCOUNT_BASE/list_accounts" || echo 000)
  [ "$code" = "200" ] && { echo "   identity live for org '$ORG' (hot reload — no restart)"; exit 0; }
  sleep 1
done
echo "!! token for org '$ORG' not resolving (HTTP $code) — is the data stack running with the" >&2
echo "   identities mount? One-time after upgrading: cd $(dirname "$0") && docker compose up -d account-service" >&2
exit 1
