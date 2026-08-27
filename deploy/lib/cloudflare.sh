# deploy/lib/cloudflare.sh — the cloudflared tunnel surface: ingress rules, reload, DNS wait.
#
# Factored out when two scripts carried divergent copies of all of this. Both sourcers still
# exist: provision-agent.sh (tracked) and repoint-hostname.sh (GITIGNORED, .gitignore:55, but
# present on operator boxes). Removing an export from this file needs
# `find . -type f -exec grep -l`, not `grep -r`, which does not descend into ignored paths —
# this header once said only one script shipped, and a caller in the invisible half spent that
# whole time calling a `cf_ingress_swap` that had been removed along with the divergent copies.
# It is defined below now, because the caller needs it: a library that documents a broken
# caller instead of serving it has described the drift rather than closed it.
#
# The two lessons their divergence taught are kept, because both are easy to reintroduce and
# neither announces itself:
#
#   1. MATCH THE WHOLE RULE LINE, never a substring. The retired copy used
#      `f"hostname: {old}" in s` plus a file-wide `str.replace`, which failed silently two
#      ways: a COMMENTED-OUT mention ("# was: hostname: new.example.com") satisfied the
#      presence check so the edit was skipped while reporting success, and a hostname
#      appearing in two rules had BOTH rewritten. cf_ingress_add below anchors on the whole
#      line for exactly this reason.
#   2. WARN WHEN THE DNS WAIT TIMES OUT. The retired copy dropped the warning and fell
#      through silently, so a hostname that never resolved looked like one that had.
#
# shellcheck shell=bash

_CF_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=deploy/lib/fleet.sh
. "$_CF_LIB_DIR/fleet.sh"

# cf_ingress_add <config> <hostname> <port> — insert a rule before the catch-all. Idempotent.
cf_ingress_add() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys
cfg, host, port = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(cfg).read().splitlines()
if any(l.strip() == f"- hostname: {host}" for l in lines):
    print("   ingress: rule already present"); sys.exit(0)
rule = [f"  - hostname: {host}", f"    service: http://localhost:{port}"]
out, inserted = [], False
for l in lines:                       # insert before the catch-all (`- service:` with no hostname)
    if (not inserted) and l.strip().startswith("- service:") and "hostname" not in l:
        out += rule; inserted = True
    out.append(l)
if not inserted:
    out += rule
open(cfg, "w").write("\n".join(out) + "\n")
print("   ingress: added", host, "->", "http://localhost:" + port)
PY
}

# cf_ingress_swap <config> <old-hostname> <new-hostname> — repoint one existing rule.
#
# The rule MOVES rather than being duplicated: leaving the old rule behind is what would keep
# the old hostname serving, and repoint-hostname.sh promises the opposite in its header ("the
# old DNS CNAME stops routing once the ingress rule is swapped").
#
# Lesson 1 above governs every match here — whole stripped line, never a substring — so a
# commented-out `# was: hostname: ...` cannot satisfy a presence test, and a hostname appearing
# in two rules is reported rather than having both rewritten. Absent-old is a HARD ERROR unless
# the new rule is already there (the idempotent re-run), because "swap a rule that is not in
# this file" has no correct silent answer.
cf_ingress_swap() {
  python3 - "$1" "$2" "$3" <<'CFSWAP'
import sys
cfg, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(cfg).read().splitlines()
olds = [i for i, l in enumerate(lines) if l.strip() == f"- hostname: {old}"]
news = [i for i, l in enumerate(lines) if l.strip() == f"- hostname: {new}"]
if old == new:
    if not olds:
        sys.exit(f"!! ingress: no rule for {new} in {cfg} - nothing to re-register against")
    print("   ingress: same hostname, rule left as is"); sys.exit(0)
if news:
    if olds:
        sys.exit(f"!! ingress: BOTH {old} and {new} have rules in {cfg} - resolve by hand; "
                 "a swap here would leave one of them serving")
    print("   ingress: rule already points at", new); sys.exit(0)
if not olds:
    sys.exit(f"!! ingress: no rule for {old} in {cfg} - refusing to guess which one to repoint")
if len(olds) > 1:
    sys.exit(f"!! ingress: {len(olds)} rules for {old} in {cfg} - resolve by hand")
i = olds[0]
lines[i] = lines[i].replace(f"- hostname: {old}", f"- hostname: {new}")
open(cfg, "w").write("\n".join(lines) + "\n")
print("   ingress: swapped", old, "->", new)
CFSWAP
}

# cf_reload — SIGHUP hot-reloads ingress without dropping other tunnels.
cf_reload() {
  local _pid
  # `|| true`: no match must not abort under pipefail — the emptiness test is the handler.
  _pid=$(pgrep -f 'cloudflared tunnel run' | head -1 || true)
  if [ -n "$_pid" ]; then
    kill -HUP "$_pid" && echo "   cloudflared reloaded (SIGHUP -> pid $_pid)"
  else
    echo "   !! cloudflared 'tunnel run' not found — reload it manually to activate ${1:-the new hostname}"
  fi
}

# cf_wait_dns <hostname> — block until the name resolves on TWO public resolvers, then settle.
#
# Telegram resolves the webhook host at setWebhook time; calling it before the fresh CNAME
# propagates gets a 400, and the NXDOMAIN then sits in Telegram's resolver's negative cache for
# many minutes (three hostnames were burned learning this). The 20s settle after first resolution is
# that fix, not padding.
cf_wait_dns() {
  local _host="$1" i
  echo "-- waiting for $_host to resolve publicly --"
  for i in $(seq 1 60); do
    if [ -n "$(dig +short @1.1.1.1 "$_host" 2>/dev/null)" ] && \
       [ -n "$(dig +short @8.8.8.8 "$_host" 2>/dev/null)" ]; then
      echo "   resolves (attempt $i); settling 20s"; sleep 20; return 0
    fi
    sleep 5
  done
  echo "!! $_host still not resolving after 5m — continuing anyway" >&2
}
