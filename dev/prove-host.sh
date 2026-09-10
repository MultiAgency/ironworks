#!/usr/bin/env bash
# COST: a live host serving a real tenant. Read-only, but every run there is the
# operator's decision in the moment, not once.
# Prove the documented serving-host facts by observing `multi-serve` over SSH.
#
#   dev/prove-host.sh             # LIVE: reaches the real serving host
#
# Thirteen read-only checks. The remote leg uses `systemctl show`, hashes and parses the registry,
# reads the current confinement attestation, and runs `lifecycle inspect --confinement` plus
# `compose`; it does not start, stop, enable, write or publish anything. Check 1 is the positive
# control for all twelve facts below it: a per-run nonce must
# return from a Host block resolving to the expected address and user. An absent alias, an
# unreachable host, a failed non-interactive sudo, a dropped connection or silent SSH output is
# BLOCKED before any configuration can pass.
#
#   1. control       the documented SSH route executes a nonce     not resolution alone
#   2. unit          ironworks-telegram is loaded/enabled/active
#   3. unit          ironworks-relay is loaded/enabled/active
#   4. unit          the tenant's IronClaw is loaded/enabled/active
#   5. unit          ironworks-devstore is loaded/disabled/inactive
#   6. member        live confinement binds as the recorded member
#   7. confinement   current policy baseline, 53/2, approval off
#   8. registry      the live file has the recorded digest
#   9. guidance      registry-derived live guidance composes with neither publication hazard
#  10. model         the requested-model policy is MODEL_PIN, not the runtime default
#  11. privilege     every path systemd runs as root — a `+`/`!` line, or a unit whose effective
#                    `User=` is root or unset, the attestation service included — is root-owned,
#                    ancestors too; and when one lies under /opt/ironworks, every object there is
#                    root-owned and not group/other-writable
#  12. unit          the tenant's ironworks-attest timer is loaded/enabled/active
#  13. attestation   the relay's current evidence names this cell and member and has not expired
#
# ── what this proves, and what it does not ──────────────────────────────────────────────────
#
# It proves one read-only snapshot of the installed units, member binding, settings-catalogue
# confinement, registry bytes and attestation age matches the deployment facts recorded in
# `deploy/README.md`. Confinement remains custody-backed and covers only catalogued settings. This
# does NOT prove a turn can be served, that the model lacks uncatalogued authority, that records or
# channels are tenant-isolated, that the registry's credential handles still work for a turn, that
# the attestation's profile and policy fingerprints are the ones the relay will compute, or that the
# snapshot remains true after this process exits. It sends no message and calls no model.
#
# Checks 12 and 13 are one obligation seen from both sides. The relay refuses every new turn
# without a current attestation, so 13 is the property and 12 is what keeps it true past the next
# ten minutes. Neither is enough alone: a timer can be active while every refresh it starts fails,
# and a fresh file says nothing about whether another will replace it.
#
# The remote leg requires passwordless root through `sudo -n`: root reads the owner-only registry,
# lifecycle store and `/etc/ironworks/relay-<slug>.env`. It sources that environment to resolve the live
# member token named by the registry, then pipes only the two required values from that file —
# `IRONCLAW_API` and
# `IRONCLAW_MEMBER_TOKEN` into the cell's `ironworks-<slug>` child under an empty environment. That child
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
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

TARGET="${PROVE_HOST_TARGET:-multi-serve}"
ADDRESS="${PROVE_HOST_ADDRESS:-}"
REMOTE_USER="${PROVE_HOST_USER:-}"
SLUG="${PROVE_HOST_SLUG:-relationship-demo}"
MEMBER="${PROVE_HOST_MEMBER:-520144ef-c36a-4606-a885-62e553c56ce7}"
CATALOGUED="${PROVE_HOST_CATALOGUED:-53}"
REACHABLE="${PROVE_HOST_REACHABLE:-2}"
# Changed from `a3f91ef2615d440f` when the tenant's registry gained its `org` field: the migration
# reported `MIGRATED relationship-demo registry a3f91ef2615d440f -> 5c62e5168c417c8c`
# (`docs/experiments/0056-the-served-host-was-never-running-the-pinned-model.md`). The old value
# stayed here afterwards and scored a correct host as a difference.
REGISTRY_DIGEST="${PROVE_HOST_REGISTRY_DIGEST:-5c62e5168c417c8c}"
SSH_BIN="${PROVE_HOST_SSH:-ssh}"

case "$TARGET" in
  ""|-*|*[[:space:]]*) give_up "control.host_reachable" "invalid SSH target" ;;
esac
# BLOCKED rather than a silent pass: with no expected address the alias check below would compare a
# value against itself and report a route nobody stated. An unstated expectation measures nothing,
# and a check that cannot fail is the defect §K-8 names.
if [ -z "$ADDRESS" ] || [ -z "$REMOTE_USER" ]; then
  give_up "control.host_reachable" \
    "set PROVE_HOST_ADDRESS and PROVE_HOST_USER to the route \`$TARGET\` must resolve to"
fi
case "$SLUG" in
  ""|*[!a-z0-9-]*|-*|*-) give_up "control.host_reachable" "invalid tenant slug" ;;
esac
case "$CATALOGUED:$REACHABLE:$REGISTRY_DIGEST" in
  *[!0-9a-f:]*) give_up "control.host_reachable" "invalid expected host facts" ;;
esac
[ -x "$SSH_BIN" ] || command -v "$SSH_BIN" >/dev/null 2>&1 || {
  give_up "control.host_reachable" "SSH executable not found: $SSH_BIN";
}

# `ssh -G` performs no network access. It makes the alias part of check 1 rather than allowing a
# missing Host block to fall through to a later, ambiguous connection error.
ssh_config="$($SSH_BIN -G -- "$TARGET" 2>/dev/null)" || ssh_config=""
resolved_address="$(printf '%s\n' "$ssh_config" | awk '$1 == "hostname" { print $2; exit }')"
resolved_user="$(printf '%s\n' "$ssh_config" | awk '$1 == "user" { print $2; exit }')"
if [ "$resolved_address" != "$ADDRESS" ] || [ "$resolved_user" != "$REMOTE_USER" ]; then
  give_up "control.host_reachable" \
    "$TARGET must resolve as $REMOTE_USER@$ADDRESS (got ${resolved_user:-?}@${resolved_address:-?})"
fi

nonce="prove-host-$$-$(date +%s)"
remote="$($SSH_BIN -o BatchMode=yes -o ConnectTimeout=10 -- "$TARGET" \
  sudo -n bash -s -- "$nonce" "$SLUG" 2>&1 <<'REMOTE'
set -uo pipefail

nonce="$1"
slug="$2"
registry="/var/lib/ironworks/$slug/registry.json"
store=/var/lib/ironworks/lifecycle.store
bin=/opt/ironworks/target/release/ironworks
# The deployment tree `deploy/README.md` requires to be root-owned end to end. Check 11 scans all
# of it whenever a root-run path lies inside it.
deploy_tree=/opt/ironworks
cell_user="ironworks-$slug"
relay_env="/etc/ironworks/relay-$slug.env"

printf 'CONTROL\t%s\n' "$nonce"

unit() {
  local name="$1" load active enabled
  load="$(systemctl show "$name" --property=LoadState --value 2>/dev/null)" || load=""
  active="$(systemctl show "$name" --property=ActiveState --value 2>/dev/null)" || active=""
  enabled="$(systemctl show "$name" --property=UnitFileState --value 2>/dev/null)" || enabled=""
  printf 'UNIT\t%s\t%s\t%s\t%s\n' "$name" "$load" "$active" "$enabled"
}

unit "ironworks-telegram@$slug.service"
unit "ironworks-relay@$slug.service"
unit "ironworks-ironclaw@$slug.service"
unit ironworks-devstore.service
unit "ironworks-attest@$slug.timer"

# The evidence the relay requires for every new turn (`--confinement-attestation` in
# `deploy/ironworks-relay@.service`). Read, never refreshed: its identity fields, its observation
# time, and the clock of this host, which is the clock the relay judges it by. `absent` and
# `malformed` are files the relay would refuse; `unread` is a file nothing here could open, which
# measures nothing. (No apostrophes in this heredoc: bash 3.2 matches quotes inside a heredoc that
# sits in a command substitution, and one unmatched quote makes the whole script unparseable.)
attestation="/run/ironworks-attest/$slug/current.json"
if [ ! -e "$attestation" ]; then
  printf 'ATTEST\tabsent\n'
elif [ ! -r "$attestation" ] || ! command -v python3 >/dev/null 2>&1; then
  printf 'ATTEST\tunread\n'
else
  python3 - "$attestation" <<'PY'
import calendar, json, sys, time
try:
    d = json.load(open(sys.argv[1], encoding="utf-8"))
    observed = d["observed_at"]
    # The one shape `crates/domain/src/attestation.rs::Utc::parse` accepts.
    if not (isinstance(observed, str) and len(observed) == 25 and observed.endswith("+00:00")):
        raise ValueError(observed)
    seconds = calendar.timegm(time.strptime(observed[:19], "%Y-%m-%dT%H:%M:%S"))
    names = [d["slug"], d["member_id"]]
    if not all(isinstance(v, str) and v and "\t" not in v and "\n" not in v for v in names):
        raise ValueError(names)
    print("ATTEST\tread\t%s\t%s\t%d\t%d" % (names[0], names[1], seconds, int(time.time())))
except Exception:
    print("ATTEST\tmalformed")
PY
fi

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
binding=""
if [ -r "$registry" ] && command -v python3 >/dev/null 2>&1; then
  binding="$(python3 - "$registry" <<'PY'
import json, re, sys
try:
    tenants = json.load(open(sys.argv[1], encoding="utf-8")).get("tenants", [])
    row = tenants[0] if len(tenants) == 1 else {}
    handle = row.get("runtime_credential", "")
    name = handle[4:] if handle.startswith("env:") else ""
    name = name if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) else ""
    values = [row.get("slug", ""), row.get("service", ""), row.get("guidance", ""), name]
    print("\t".join(v if isinstance(v, str) and "\t" not in v and "\n" not in v else "" for v in values))
except Exception:
    print("\t\t\t")
PY
)"
fi
IFS=$'\t' read -r live_slug live_service live_guidance member_env <<<"$binding"
printf 'BINDING\t%s\t%s\t%s\n' "${live_slug:-}" "${live_service:-}" "${live_guidance:-}"

compose=""
compose_rc=3
if [ -x "$bin" ] && [ -n "${live_slug:-}" ] && [ -n "${live_service:-}" ] \
   && [ -r "${live_guidance:-}" ]; then
  compose="$(runuser -u "$cell_user" -- "$bin" compose --slug "$live_slug" \
    --service "$live_service" --guidance "$live_guidance" 2>&1)"
  compose_rc=$?
fi
printf 'COMPOSE_RC\t%s\n' "$compose_rc"
printf 'COMPOSE_BEGIN\t%s\n' "$nonce"
printf '%s\n' "$compose"
printf 'COMPOSE_END\t%s\n' "$nonce"

if [ -x "$bin" ] && [ -r "$relay_env" ] && [ -n "${member_env:-}" ]; then
  # The install procedure defines this file as shell-compatible NAME=value entries and uses the
  # same sourcing path for attended lifecycle commands. It is sourced as root because the file is
  # root-only; only the runtime base and selected member token cross into the unprivileged child.
  . "$relay_env"
  member_token="${!member_env:-}"
  runtime_api="${IRONCLAW_API:-}"
  if [ -n "$member_token" ] && [ -n "$runtime_api" ]; then
    inspect="$(printf '%s\n%s\n' "$runtime_api" "$member_token" | \
      runuser -u "$cell_user" -- /usr/bin/env -i /bin/bash -c '
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

# A pin is not self-enforcing. A runtime rebuilt, restored or replaced comes back with its own
# default and serves; measured 2026-09-07, this host had never been told the pin at all. The
# operator credential is read from the IronClaw environment rather than the cell environment,
# because the serving account must never hold it.
ironclaw_env="/etc/ironworks/ironclaw-$slug.env"
policy=""
# `policy_rc` is the verdict; the text is only for the operator to read. `model-policy check`
# returns `Result<(), Exit>` and `require_model_policy_exact` is what fails it, so a mismatch is
# already a non-zero exit — this carries that back instead of leaving the scorer to infer it from a
# sentence. 127 is the "never ran" value, distinct from any exit the command itself produces.
policy_rc=127
if [ -r "$relay_env" ] && [ -r "$ironclaw_env" ]; then
  policy="$(cd /opt/ironworks && set -a && . "$relay_env" && . "$ironclaw_env" && set +a &&
    IRONCLAW_OPERATOR_TOKEN="${IRONCLAW_REBORN_WEBUI_TOKEN:-}" "$bin" model-policy check 2>&1)"
  policy_rc=$?
fi
printf 'MODELPOLICY_BEGIN\t%s\n' "$nonce"
printf '%s\n' "$policy"
printf 'MODELPOLICY_END\t%s\n' "$nonce"
printf 'MODELPOLICYRC\t%s\t%s\n' "$nonce" "$policy_rc"

# What systemd runs as ROOT. A command prefixed `+` or `!` ignores `User=`, and every command of a
# system unit whose effective `User=` is root, or unset, runs as root. If such a path — or any
# directory on the way to it — is writable by a non-root account, that account executes code as
# root at the next start, and `Restart=always` and timers make starts unattended. Measured
# 2026-09-07: two active units ran their preflight from a tree owned by the unprivileged login
# account
# (`docs/experiments/0058-the-serving-host-ran-its-own-preflight-from-a-directory-the-login-account-could-write.md`).
# The attestation service is the case a prefix-only scan misses: `User=root`, no `+`, every five
# minutes.
# **Ancestors are walked, not just the leaf.** Replacing a directory replaces what is under it, so
# a check that read only the script would have passed that host on the day it was exploitable.
# **`User=` is read from systemd, never from the unit files in this tree**: `systemctl show`
# reports the effective value after drop-ins, which is the one that decides who runs the command.
# Unset counts as root even where `DynamicUser=` would supply a transient account; none of these
# units sets it, and that error runs in the safe direction — a path walked that need not have been.
# What a root command executes in turn is named by no unit line — `refresh-attestation.sh` runs
# the `ironworks` binary as root — so whenever a root-run path lies under the deployment tree, the
# whole tree is scanned as well (`tree_scan` below). `deploy/README.md` requires every object in it
# to be root-owned, and holding the tree to that covers the binary and anything a root script there
# runs later. Nothing outside that tree is scanned wholesale.
# A `PRIVSCANNED` row per unit distinguishes "no privileged paths" from "nothing could be read":
# without it an unreadable unit and a clean one look identical, and the check would pass on silence.
# `dev/test-prove-host.sh` runs this function, from the script as transmitted, against stubbed
# `systemctl` and `stat`; it finds it by the two lines that open and close the block below.
privileged_scan() {
  local privileged_unit unit_text unit_user unit_root
  for privileged_unit in "$@"; do
    unit_text="$(systemctl cat "$privileged_unit" 2>/dev/null)" || unit_text=""
    [ -n "$unit_text" ] || continue
    unit_user="$(systemctl show "$privileged_unit" --property=User --value 2>/dev/null)" \
      || unit_user=""
    # Every `case` here opens its patterns with `(`. Bash 3.2 — the one an operator Mac runs this
    # with — counts parentheses inside a heredoc nested in a command substitution, so a bare
    # `pattern)` closes the substitution early and the rest of this leg runs locally.
    case "$unit_user" in (""|root|0) unit_root=yes ;; (*) unit_root=no ;; esac
    printf 'PRIVSCANNED\t%s\n' "$privileged_unit"
    printf '%s\n' "$unit_text" \
      | sed -En 's/^(ExecStart(Pre|Post)?|ExecStop(Post)?|ExecReload|ExecCondition)=([-@:+!]*)([^[:space:]]+).*/\5 \4/p' \
      | while read -r privileged_path prefix; do
          case "$unit_root:$prefix" in (yes:*|*[+!]*) ;; (*) continue ;; esac
          offender=""
          node="$privileged_path"
          while :; do
            if [ ! -e "$node" ]; then
              offender="$node is absent"
            else
              node_uid="$(stat -c '%u' "$node" 2>/dev/null)" || node_uid=""
              node_mode="$(stat -c '%a' "$node" 2>/dev/null)" || node_mode=""
              if [ -z "$node_uid" ] || [ -z "$node_mode" ]; then
                offender="$node could not be read"
              elif [ "$node_uid" != 0 ]; then
                offender="$node is owned by uid $node_uid"
              elif [ $(( 8#$node_mode & 0022 )) -ne 0 ]; then
                offender="$node is mode $node_mode"
              fi
            fi
            [ -n "$offender" ] && break
            [ "$node" = / ] && break
            node="$(dirname "$node")"
          done
          printf 'PRIVPATH\t%s\t%s\t%s\n' "$privileged_unit" "$privileged_path" "$offender"
        done
  done
}
# Every object under ROOT that is not root-owned, or that group or other can write. One `find`,
# never following a symlink or crossing into another filesystem; it only lists, and the rule is
# applied here in awk, so what decides is this code rather than a `find` predicate. Only the first
# five offenders and a count cross the wire, because this host once held 11,142. A symlink is
# judged by its owner alone: its mode is always 777 and grants nothing. A listing that failed, or
# listed nothing at all — ROOT itself is always listed — is `unreadable`, which blocks the check.
tree_scan() { # tree_scan ROOT
  local listing tree_rc
  listing="$(find -P "$1" -xdev -printf '%U\t%m\t%y\t%p\n' 2>/dev/null)"
  tree_rc=$?
  printf '%s\n' "$listing" | awk -F '\t' -v root="$1" -v rc="$tree_rc" '
    NF >= 4 {
      seen++
      path = $4
      for (i = 5; i <= NF; i++) path = path "\t" $i
      group = substr($2, length($2) - 1, 1)
      other = substr($2, length($2), 1)
      reason = ""
      if ($1 != "0") reason = "owned by uid " $1
      else if ($3 != "l" && (group ~ /[2367]/ || other ~ /[2367]/)) reason = "mode " $2
      if (reason != "") { bad++; if (bad <= 5) printf "PRIVTREEOBJ\t%s\t%s\n", path, reason }
    }
    END {
      state = (rc == 0 && seen > 0) ? "scanned" : "unreadable"
      printf "PRIVTREE\t%s\t%s\t%d\n", root, state, bad
    }'
}
# The attestation service is named, and so is whatever its timer effectively triggers: a timer
# retargeted by a drop-in starts that unit instead.
scan_units=("ironworks-telegram@$slug.service" "ironworks-relay@$slug.service"
            "ironworks-ironclaw@$slug.service" "ironworks-attest@$slug.service")
triggers="$(systemctl show "ironworks-attest@$slug.timer" --property=Triggers --value 2>/dev/null)" \
  || triggers=""
for triggered in $triggers; do
  case " ${scan_units[*]} " in (*" $triggered "*) ;; (*) scan_units+=("$triggered") ;; esac
done
priv_rows="$(privileged_scan "${scan_units[@]}")"
printf '%s\n' "$priv_rows"
# The deployment tree is scanned when root runs anything from it, and it is the only tree scanned:
# a root line naming a system binary does not make `/usr` part of this check.
if printf '%s\n' "$priv_rows" | awk -F '\t' -v tree="$deploy_tree/" \
     '$1 == "PRIVPATH" && index($3, tree) == 1 { found = 1 } END { exit !found }'; then
  tree_scan "$deploy_tree"
else
  printf 'PRIVTREE\t%s\tnot-needed\t0\n' "$deploy_tree"
fi
# end of the root-execution scan: dev/test-prove-host.sh cuts the block ending here
REMOTE
)"
ssh_rc=$?

if [ "$ssh_rc" -ne 0 ] || ! printf '%s\n' "$remote" | awk -F '\t' -v n="$nonce" \
    '$1 == "CONTROL" && $2 == n { found=1 } END { exit !found }'; then
  give_up "control.host_reachable" "SSH/sudo did not return the per-run control (rc=$ssh_rc)"
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

unit_check "ironworks-telegram@$SLUG.service" active enabled
unit_check "ironworks-relay@$SLUG.service" active enabled
unit_check "ironworks-ironclaw@$SLUG.service" active enabled
unit_check ironworks-devstore.service inactive disabled

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

binding="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "BINDING" { print; exit }')"
IFS=$'\t' read -r _ live_slug live_service live_guidance <<<"$binding"
compose_rc="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "COMPOSE_RC" { print $2; exit }')"
compose="$(printf '%s\n' "$remote" | awk -v b="COMPOSE_BEGIN\t$nonce" -v e="COMPOSE_END\t$nonce" \
  '$0 == b { copying=1; next } $0 == e { copying=0; exit } copying')"
if [ "$live_slug" != "$SLUG" ] || [ -z "${live_service:-}" ] || [ -z "${live_guidance:-}" ]; then
  verdict "guidance.publication_gate" FAIL "one complete live registry binding was not derived"
elif [ "$compose_rc" != 0 ] || [ -z "$compose" ]; then
  verdict "guidance.publication_gate" FAIL "live registry guidance did not compose cleanly"
elif grep -q 'WARNING model-visible guidance holds' <<<"$compose"; then
  verdict "guidance.publication_gate" FAIL "live guidance contains a refused publication hazard"
else
  verdict "guidance.publication_gate" PASS "registry-derived $live_slug/$live_service guidance is clean"
fi

policy="$(printf '%s\n' "$remote" | awk -v b="MODELPOLICY_BEGIN\t$nonce" -v e="MODELPOLICY_END\t$nonce" \
  '$0 == b { copying=1; next } $0 == e { copying=0; exit } copying')"
# Scored on the command's exit code and on the pin it names, never on its wording. The message
# grep this replaced would have passed on any reword, and — worse — would have passed a host whose
# policy matched *a* pin that is not the one this tree holds, because it never compared them.
# `MODEL_PIN`'s first line up to `#`, trimmed, is the same rule `crates/domain/src/pins.rs::parse`
# applies.
policy_rc="$(printf '%s\n' "$remote" | awk -F '\t' -v n="$nonce" \
  '$1 == "MODELPOLICYRC" && $2 == n { print $3; exit }')"
want_pin="$(head -n1 "$ROOT/MODEL_PIN" | cut -d'#' -f1 | tr -d '[:space:]')"
if [ -z "${policy//[[:space:]]/}" ] || [ -z "$policy_rc" ] || [ "$policy_rc" = "127" ]; then
  verdict "model.policy_matches_pin" BLOCKED "the requested-model policy could not be read"
elif [ "$policy_rc" != "0" ]; then
  verdict "model.policy_matches_pin" FAIL "$(why "$policy")"
elif [ -z "$want_pin" ]; then
  verdict "model.policy_matches_pin" BLOCKED "MODEL_PIN names nothing on its first line"
elif ! grep -qF "$want_pin" <<<"$policy"; then
  # Exit 0 while naming a different model is the one shape a bare exit-code check would miss.
  verdict "model.policy_matches_pin" FAIL "policy check passed but did not name $want_pin"
else
  verdict "model.policy_matches_pin" PASS "the requested-model policy is $want_pin"
fi

# A unit's `+` prefix is one character that turns a safety check into root execution, and a
# `User=root` unit needs none. This scores the ownership of every path the serving units and the
# attestation service run as root, and of every directory leading to one.
priv_scanned="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "PRIVSCANNED" { n++ } END { print n + 0 }')"
priv_paths="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "PRIVPATH" { print }')"
priv_offenders="$(printf '%s\n' "$priv_paths" | awk -F '\t' '$4 != "" { print $2 ": " $4 }')"
priv_count="$(printf '%s\n' "$priv_paths" | grep -c . || true)"
# Every unit the remote leg must read. One that could not be read was not walked, so it blocks
# rather than letting the others pass for it — the attestation service is the one that would.
priv_unread=""
for priv_unit in "ironworks-telegram@$SLUG.service" "ironworks-relay@$SLUG.service" \
                 "ironworks-ironclaw@$SLUG.service" "ironworks-attest@$SLUG.service"; do
  printf '%s\n' "$remote" | awk -F '\t' -v u="$priv_unit" \
    '$1 == "PRIVSCANNED" && $2 == u { found=1 } END { exit !found }' \
    || priv_unread="$priv_unread $priv_unit"
done
# The deployment-tree scan. The remote leg always reports one `PRIVTREE` row. Whether a scan was
# owed is derived again here, from the root-run paths themselves, so a leg that skipped the tree
# while running something from it blocks rather than passes.
priv_tree_row="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "PRIVTREE" { print; exit }')"
IFS=$'\t' read -r _ priv_tree priv_tree_state priv_tree_bad <<<"$priv_tree_row"
priv_tree_objects="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "PRIVTREEOBJ" { print $2 ": " $3 }')"
priv_tree_owed=no
if [ -n "${priv_tree:-}" ] && printf '%s\n' "$priv_paths" | awk -F '\t' -v tree="$priv_tree/" \
     'index($3, tree) == 1 { found = 1 } END { exit !found }'; then
  priv_tree_owed=yes
fi
if [ "$priv_scanned" -eq 0 ]; then
  verdict "privilege.root_execution_paths" BLOCKED "no unit could be read for privileged paths"
elif [ -n "$priv_offenders" ]; then
  verdict "privilege.root_execution_paths" FAIL "$(why "$priv_offenders")"
elif [[ "${priv_tree_bad:-}" =~ ^[1-9][0-9]*$ ]]; then
  verdict "privilege.root_execution_paths" FAIL \
    "$priv_tree_bad object(s) under $priv_tree are not root-owned or are group/other-writable"
  printf '%s\n' "$priv_tree_objects" | sed 's/^/          /'
elif [ -n "$priv_unread" ]; then
  verdict "privilege.root_execution_paths" BLOCKED "not read, so not walked:$priv_unread"
elif [ -z "$priv_tree_row" ]; then
  verdict "privilege.root_execution_paths" BLOCKED "the deployment-tree scan reported nothing"
elif [ "$priv_tree_state" = unreadable ]; then
  verdict "privilege.root_execution_paths" BLOCKED "$priv_tree could not be read in full"
elif [ "$priv_tree_owed" = yes ] && [ "$priv_tree_state" != scanned ]; then
  verdict "privilege.root_execution_paths" BLOCKED \
    "root runs paths under $priv_tree, but the tree was not scanned"
elif [ "$priv_tree_state" = scanned ]; then
  verdict "privilege.root_execution_paths" PASS \
    "$priv_count path(s) run as root are root-owned, and so is every directory above them; and every object under $priv_tree is root-owned and not group/other-writable"
elif [ "$priv_tree_state" = not-needed ]; then
  verdict "privilege.root_execution_paths" PASS \
    "$priv_count path(s) run as root are root-owned, and so is every directory above them; none is under $priv_tree"
else
  verdict "privilege.root_execution_paths" BLOCKED "the deployment-tree scan reported $priv_tree_state"
fi

unit_check "ironworks-attest@$SLUG.timer" active enabled

# Check 13. The window is read from the constant the relay enforces rather than restated, the way
# check 10 reads `MODEL_PIN`: a second copy of ten minutes is a second thing to keep in step. The
# boundary is the relay's too — valid while younger than the window, expired at exactly it.
validity_expr="$(sed -n 's/^pub const VALIDITY_SECONDS: i64 = \([0-9 *]*\);$/\1/p' \
  "$ROOT/crates/domain/src/attestation.rs")"
attest="$(printf '%s\n' "$remote" | awk -F '\t' '$1 == "ATTEST" { print; exit }')"
IFS=$'\t' read -r _ attest_state attest_slug attest_member attest_observed attest_now <<<"$attest"
if [ -z "$attest" ] || [ "${attest_state:-}" = unread ]; then
  verdict "attestation.current" BLOCKED "the current attestation could not be read"
elif [ -z "$validity_expr" ]; then
  verdict "attestation.current" BLOCKED \
    "VALIDITY_SECONDS could not be read from crates/domain/src/attestation.rs"
elif [ "$attest_state" = absent ]; then
  verdict "attestation.current" FAIL "no current attestation; the relay refuses every new turn"
elif [ "$attest_state" != read ] || ! [[ "${attest_observed:-}" =~ ^[0-9]+$ ]] \
     || ! [[ "${attest_now:-}" =~ ^[0-9]+$ ]]; then
  verdict "attestation.current" FAIL "the current attestation is malformed; the relay refuses it"
elif [ "$attest_slug" != "$SLUG" ] || [ "$attest_member" != "$MEMBER" ]; then
  verdict "attestation.current" FAIL \
    "it attests $attest_slug as $attest_member, not $SLUG as $MEMBER"
else
  validity=$(( validity_expr ))
  attest_age=$(( attest_now - attest_observed ))
  if [ "$attest_age" -lt 0 ]; then
    verdict "attestation.current" FAIL "observed ${attest_age#-}s in the future; the relay refuses it"
  elif [ "$attest_age" -ge "$validity" ]; then
    verdict "attestation.current" FAIL \
      "observed ${attest_age}s ago and expired at ${validity}s; the relay refuses every new turn"
  else
    verdict "attestation.current" PASS "observed ${attest_age}s ago, valid for ${validity}s"
  fi
fi

summary
