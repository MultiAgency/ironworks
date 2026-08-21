# deploy/lib/cloudflare.sh — the cloudflared tunnel surface: ingress rules, reload, DNS wait.
#
# Factored out when two scripts carried divergent copies of all of this. Only one of them
# ships now (provision-agent.sh), so the swap/presence helpers that served the other are gone
# with it — but the two lessons their divergence taught are kept, because both are easy to
# reintroduce and neither announces itself:
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
