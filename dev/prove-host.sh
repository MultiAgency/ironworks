#!/usr/bin/env bash
# Prove the documented serving-host facts by observing `multi-serve` over SSH.
#
#   dev/prove-host.sh             # LIVE: reaches the real serving host
#
# Eight read-only checks. The remote leg uses `systemctl show`, hashes and parses the registry,
# and runs `lifecycle inspect --confinement`; it does not start, stop, enable, write or publish
# anything. Check 1 is the positive control for all seven facts below it: a per-run nonce must
# return from a Host block resolving to the expected address and user. An absent alias, an
# unreachable host, a failed non-interactive sudo, a dropped connection or silent SSH output is
# BLOCKED before any configuration can pass.
#
#   1. control       the documented SSH route executes a nonce     not resolution alone
#   2. unit          rustworks-telegram is loaded/enabled/active
#   3. unit          rustworks-relay is loaded/enabled/active
#   4. unit          the tenant's IronClaw is loaded/enabled/active
#   5. unit          rustworks-devstore is loaded/disabled/inactive
#   6. member        live confinement binds as the recorded member
#   7. confinement   current policy baseline, 53/2, approval off
#   8. registry      the live file has the recorded digest
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves one read-only snapshot of the installed units, member binding, settings-catalogue
# confinement and registry bytes matches the deployment facts recorded in `deploy/README.md`.
# Confinement remains custody-backed and covers only catalogued settings. This does NOT prove a
# turn can be served, that the model lacks uncatalogued authority, that records or channels are
# tenant-isolated, that the registry's credential handles still work for a turn, or that the
# snapshot remains true after this process exits. It sends no message and calls no model.
#
# The remote leg requires passwordless root through `sudo -n`: root reads the owner-only registry,
# lifecycle store and `/etc/rustworks/relay.env`. It sources that environment to resolve the live
# member token named by the registry, then pipes only the two required values from that file —
# `IRONCLAW_API` and
# `IRONCLAW_MEMBER_TOKEN` into a `rustworks` child started under an empty environment. That child
# exports them internally before `exec`, so the token is in process memory for checks 6 and 7 but
# never in an argument or output.
#
# The defaults name a live host serving a real tenant, so most of them are the documented claim
# this proof was built to check and may be overridden explicitly when the deployment changes.
# **The address and the remote user are the exception: they have no default and this proof refuses
# to run without them.** This repository is public, and a serving host's SSH endpoint written into
# it is a targeting aid that git history does not take back. Check 1 is no weaker for it — the
# control is that a nonce returns through the alias, and what the operator supplies is only what
# the alias is expected to resolve to:
#
#   PROVE_HOST_ADDRESS=<address> PROVE_HOST_USER=<user> dev/prove-host.sh
#
# Do not run this proof until the operator approves reaching that host in the moment.
#
# Verdicts follow the operator convention: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
set -uo pipefail

TARGET="${PROVE_HOST_TARGET:-multi-serve}"
ADDRESS="${PROVE_HOST_ADDRESS:-}"
REMOTE_USER="${PROVE_HOST_USER:-}"
SLUG="${PROVE_HOST_SLUG:-relationship-demo}"
MEMBER="${PROVE_HOST_MEMBER:-520144ef-c36a-4606-a885-62e553c56ce7}"
CATALOGUED="${PROVE_HOST_CATALOGUED:-53}"
REACHABLE="${PROVE_HOST_REACHABLE:-2}"
REGISTRY_DIGEST="${PROVE_HOST_REGISTRY_DIGEST:-a3f91ef2615d440f}"
SSH_BIN="${PROVE_HOST_SSH:-ssh}"

pass=0; failed=0; blocked=0
verdict() {
  printf '%-9s %-34s %s\n' "$2" "$1" "${3:-}"
  case "$2" in
    PASS) pass=$((pass + 1)) ;;
    FAIL) failed=$((failed + 1)) ;;
    BLOCKED) blocked=$((blocked + 1)) ;;
  esac
}

case "$TARGET" in
  ""|-*|*[[:space:]]*) verdict "control.host_reachable" BLOCKED "invalid SSH target"; exit 3 ;;
esac
# BLOCKED rather than a silent pass: with no expected address the alias check below would compare a
# value against itself and report a route nobody stated. An unstated expectation measures nothing,
# and a check that cannot fail is the defect §K-8 names.
if [ -z "$ADDRESS" ] || [ -z "$REMOTE_USER" ]; then
  verdict "control.host_reachable" BLOCKED \
    "set PROVE_HOST_ADDRESS and PROVE_HOST_USER to the route \`$TARGET\` must resolve to"
  exit 3
fi
case "$SLUG" in
  ""|*[!a-z0-9-]*|-*|*-) verdict "control.host_reachable" BLOCKED "invalid tenant slug"; exit 3 ;;
esac
case "$CATALOGUED:$REACHABLE:$REGISTRY_DIGEST" in
  *[!0-9a-f:]*) verdict "control.host_reachable" BLOCKED "invalid expected host facts"; exit 3 ;;
esac
[ -x "$SSH_BIN" ] || command -v "$SSH_BIN" >/dev/null 2>&1 || {
  verdict "control.host_reachable" BLOCKED "SSH executable not found: $SSH_BIN"
  exit 3
}

# `ssh -G` performs no network access. It makes the alias part of check 1 rather than allowing a
# missing Host block to fall through to a later, ambiguous connection error.
ssh_config="$($SSH_BIN -G -- "$TARGET" 2>/dev/null)" || ssh_config=""
resolved_address="$(printf '%s\n' "$ssh_config" | awk '$1 == "hostname" { print $2; exit }')"
resolved_user="$(printf '%s\n' "$ssh_config" | awk '$1 == "user" { print $2; exit }')"
if [ "$resolved_address" != "$ADDRESS" ] || [ "$resolved_user" != "$REMOTE_USER" ]; then
  verdict "control.host_reachable" BLOCKED \
    "$TARGET must resolve as $REMOTE_USER@$ADDRESS (got ${resolved_user:-?}@${resolved_address:-?})"
  exit 3
fi

nonce="prove-host-$$-$(date +%s)"
remote="$($SSH_BIN -o BatchMode=yes -o ConnectTimeout=10 -- "$TARGET" \
  sudo -n bash -s -- "$nonce" "$SLUG" 2>&1 <<'REMOTE'
set -uo pipefail

nonce="$1"
slug="$2"
registry=/var/lib/rustworks/registry.json
store=/var/lib/rustworks/lifecycle.store
bin=/opt/rustworks/target/release/ironworks

printf 'CONTROL\t%s\n' "$nonce"

unit() {
  local name="$1" load active enabled
  load="$(systemctl show "$name" --property=LoadState --value 2>/dev/null)" || load=""
  active="$(systemctl show "$name" --property=ActiveState --value 2>/dev/null)" || active=""
  enabled="$(systemctl show "$name" --property=UnitFileState --value 2>/dev/null)" || enabled=""
  printf 'UNIT\t%s\t%s\t%s\t%s\n' "$name" "$load" "$active" "$enabled"
}

unit rustworks-telegram.service
unit rustworks-relay.service
unit "rustworks-ironclaw@$slug.service"
unit rustworks-devstore.service

digest=""
if [ -r "$registry" ]; then
  if command -v sha256sum >/dev/null 2>&1; then
    digest="$(sha256sum "$registry" 2>/dev/null | cut -c1-16)"
  elif command -v shasum >/dev/null 2>&1; then
    digest="$(shasum -a 256 "$registry" 2>/dev/null | cut -c1-16)"
  fi
fi
printf 'REGISTRY\t%s\n' "$digest"

inspect=""
member_env=""
if [ -r "$registry" ] && command -v python3 >/dev/null 2>&1; then
  member_env="$(python3 - "$registry" "$slug" <<'PY'
import json, re, sys
try:
    tenants = json.load(open(sys.argv[1], encoding="utf-8")).get("tenants", [])
    matches = [x for x in tenants if x.get("slug") == sys.argv[2]]
    handle = matches[0].get("runtime_credential", "") if len(matches) == 1 else ""
    name = handle[4:] if handle.startswith("env:") else ""
    print(name if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) else "")
except Exception:
    print("")
PY
)"
fi

if [ -x "$bin" ] && [ -r /etc/rustworks/relay.env ] && [ -n "$member_env" ]; then
  # The install procedure defines this file as shell-compatible NAME=value entries and uses the
  # same sourcing path for attended lifecycle commands. It is sourced as root because the file is
  # root-only; only the runtime base and selected member token cross into the unprivileged child.
  . /etc/rustworks/relay.env
  member_token="${!member_env:-}"
  runtime_api="${IRONCLAW_API:-}"
  if [ -n "$member_token" ] && [ -n "$runtime_api" ]; then
    inspect="$(printf '%s\n%s\n' "$runtime_api" "$member_token" | \
      runuser -u rustworks -- /usr/bin/env -i /bin/bash -c '
        IFS= read -r IRONCLAW_API || exit 3
        IFS= read -r IRONCLAW_MEMBER_TOKEN || exit 3
        export IRONCLAW_API IRONCLAW_MEMBER_TOKEN
        exec "$1" lifecycle inspect "$2" --store "$3" --confinement
      ' bash "$bin" "$slug" "$store" 2>&1)"
  fi
fi

printf 'INSPECT_BEGIN\t%s\n' "$nonce"
printf '%s\n' "$inspect"
printf 'INSPECT_END\t%s\n' "$nonce"
REMOTE
)"
ssh_rc=$?

if [ "$ssh_rc" -ne 0 ] || ! printf '%s\n' "$remote" | awk -F '\t' -v n="$nonce" \
    '$1 == "CONTROL" && $2 == n { found=1 } END { exit !found }'; then
  verdict "control.host_reachable" BLOCKED "SSH/sudo did not return the per-run control (rc=$ssh_rc)"
  exit 3
fi
verdict "control.host_reachable" PASS "$TARGET executed the per-run control"

unit_check() { # unit_check name expected-active expected-enabled
  local name="$1" want_active="$2" want_enabled="$3" row load active enabled
  row="$(printf '%s\n' "$remote" | awk -F '\t' -v n="$name" '$1 == "UNIT" && $2 == n { print; exit }')"
  IFS=$'\t' read -r _ _ load active enabled <<<"$row"
  if [ -z "$row" ] || [ -z "${load:-}" ] || [ -z "${active:-}" ] || [ -z "${enabled:-}" ]; then
    verdict "unit.$name" BLOCKED "systemd state could not be read"
  elif [ "$load" = loaded ] && [ "$active" = "$want_active" ] && [ "$enabled" = "$want_enabled" ]; then
    verdict "unit.$name" PASS "$load, $enabled, $active"
  else
    verdict "unit.$name" FAIL \
      "got $load/$enabled/$active; expected loaded/$want_enabled/$want_active"
  fi
}

unit_check rustworks-telegram.service active enabled
unit_check rustworks-relay.service active enabled
unit_check "rustworks-ironclaw@$SLUG.service" active enabled
unit_check rustworks-devstore.service inactive disabled

inspect="$(printf '%s\n' "$remote" | awk -v b="INSPECT_BEGIN\t$nonce" -v e="INSPECT_END\t$nonce" \
  '$0 == b { copying=1; next } $0 == e { copying=0; exit } copying')"
if [ -z "$inspect" ]; then
  verdict "member.bound" BLOCKED "lifecycle confinement inspection produced no report"
  verdict "confinement.current_policy" BLOCKED "lifecycle confinement inspection produced no report"
else
  subject="live: bound as $MEMBER in tenant"
  if printf '%s\n' "$inspect" | grep -Fq "$subject"; then
    verdict "member.bound" PASS "live credential bound as recorded member $MEMBER"
  elif printf '%s\n' "$inspect" | grep -Eq '^[[:space:]]*BLOCKED'; then
    verdict "member.bound" BLOCKED "live member binding could not be established"
  elif printf '%s\n' "$inspect" | grep -Eq '^[[:space:]]*FAIL'; then
    verdict "member.bound" FAIL "inspection reported a binding failure for recorded member $MEMBER"
  else
    verdict "member.bound" FAIL "inspection bound a member other than recorded member $MEMBER"
  fi

  live="live: $CATALOGUED catalogued, $REACHABLE reachable, approval off"
  if printf '%s\n' "$inspect" | grep -Fq "$live" \
     && printf '%s\n' "$inspect" | grep -Fq "verified live against a recorded baseline"; then
    verdict "confinement.current_policy" PASS "$CATALOGUED catalogued, $REACHABLE reachable, approval off"
  elif printf '%s\n' "$inspect" | grep -Eq '^[[:space:]]*BLOCKED'; then
    verdict "confinement.current_policy" BLOCKED "current confinement could not be established"
  elif ! printf '%s\n' "$inspect" | grep -Eq '^[[:space:]]*live: bound as '; then
    verdict "confinement.current_policy" BLOCKED "member binding failed before confinement was measured"
  else
    verdict "confinement.current_policy" FAIL "live confinement differs from the recorded claim"
  fi
fi

live_digest="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "REGISTRY" { print $2; exit }')"
if [ -z "$live_digest" ]; then
  verdict "registry.digest" BLOCKED "the live registry could not be read or hashed"
elif [ "$live_digest" = "$REGISTRY_DIGEST" ]; then
  verdict "registry.digest" PASS "$live_digest"
else
  verdict "registry.digest" FAIL "got $live_digest; expected $REGISTRY_DIGEST"
fi

printf '\n%d passed, %d failed, %d blocked\n' "$pass" "$failed" "$blocked"
[ "$failed" -gt 0 ] && exit 2
[ "$blocked" -gt 0 ] && exit 3
exit 0
