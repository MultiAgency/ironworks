#!/usr/bin/env bash
# COST: the probe — its first act is to tear the stack down. No model key.
# Prove IronClaw boots from a fresh IronWorks volume under the HARDENED capability set.
#
#   dev/prove-boot.sh
#
# Costs no model call and needs no credentials. It exists because 1.4.0 introduced a boot
# failure that the three behavioural proofs cannot see: they only ever run against a runtime
# that already came up.
#
# ── IT DESTROYS THE PROBE. Do not run it against a stack you are using. ──────────────────────
#
# Proving a FRESH volume boots requires a fresh volume, so the first thing below is
# `dev/probe-up.sh --down`: the volume goes, the tmpfs Postgres goes, and every thread on that
# instance goes with it. Bring-up then mints new secrets, so an IRONWORKS_TOKEN already exported
# in another shell stops working and the next turn there reports `the credential was rejected` —
# which reads like a defect in the seam and is not.
#
# This is easy to miss because the script reports 4 PASS while having deleted the conversation you
# were about to continue. Observed exactly that way on 2026-09-02, mid-audit. Re-export from
# `docker inspect multiprobe-ic-1` afterwards, and budget ~30s for the rebuild.
#
# ── the drift this pins ──────────────────────────────────────────────────────────────────────
#
# From 1.4.0 the entrypoint chowns $IRONCLAW_REBORN_HOME to `ironclaw` and THEN, still running as
# root, `mkdir -p`s $HOME/workspace. The image bakes /data/ironclaw-reborn as 1000:1000 and a
# fresh named volume is seeded from it, so root — which under `cap_drop: [ALL]` has no
# DAC_OVERRIDE — cannot write there. Boot died with:
#
#   mkdir: cannot create directory '/data/ironclaw-reborn/workspace': Permission denied
#
# The fix is a one-shot `ic-init` that pre-creates the directory AS `ironclaw`, which owns it,
# and therefore needs no root and no capabilities. `mkdir -p` on an existing directory is a no-op
# that never writes to the parent, so the failing operation stops happening.
#
# WHAT THIS ASSERTS, and why each line is here:
#   1. the runtime holds exactly the hardened capability set — no DAC_OVERRIDE crept back
#   2. the init holds strictly fewer capabilities than the runtime it precedes
#   3. a FRESH volume boots (the failing case)
#   4. an EXISTING volume boots again (idempotence — the fix must not be a first-run trick)
#
# Verdicts: PASS / FAIL / BLOCKED, exiting 0 / 2 / 3.
# shellcheck source=dev/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v docker >/dev/null || { give_up "docker" "not installed"; }

caps() { docker inspect "$1" --format '{{.HostConfig.CapAdd}}|{{.HostConfig.CapDrop}}' 2>/dev/null; }

# ── 1 & 3. a fresh volume ────────────────────────────────────────────────────────────────
bash dev/probe-up.sh --down >/dev/null 2>&1
if ! out="$(bash dev/probe-up.sh 2>&1)"; then
  verdict "boot.fresh_volume" FAIL "$(grep -m1 -iE 'permission|denied|never became' <<<"$out" | cut -c1-70)"
else
  verdict "boot.fresh_volume" PASS "healthy from a volume seeded by the image"
fi

RT="$(caps multiprobe-ic-1)"; IN="$(caps multiprobe-ic-init-1)"
# The hardened set, written out so a widening is a diff in this file rather than a surprise.
if [ "$RT" = "[CAP_CHOWN CAP_SETGID CAP_SETUID]|[ALL]" ]; then
  verdict "caps.runtime_unwidened" PASS "CHOWN,SETGID,SETUID · ALL dropped · no DAC_OVERRIDE"
else
  verdict "caps.runtime_unwidened" FAIL "runtime caps are $RT"
fi
if [ "$IN" = "[]|[ALL]" ]; then
  verdict "caps.init_is_narrower" PASS "no capabilities, runs as uid 1000"
else
  verdict "caps.init_is_narrower" FAIL "init caps are $IN"
fi

# ── 4. an existing volume ────────────────────────────────────────────────────────────────
# Restart WITHOUT removing the volume. A fix that only works on first run would pass above and
# strand every real deployment on its second boot.
# `docker restart`, NOT `docker compose restart`: compose interpolates the compose file for every
# subcommand, so the `${PROBE_IMAGE:?}` guard aborts a restart that needs no image resolution at
# all. Restarting the container alone is also the truer test — it does NOT re-run `ic-init`, so
# this measures the runtime booting against a volume the init touched once, long ago.
if docker restart multiprobe-ic-1 >/dev/null 2>&1; then
  # A container boot, not a process start: two minutes at two-second intervals.
  if wait_for_http "http://127.0.0.1:${PROBE_PORT:-3997}/api/health" 60 2 2; then
    verdict "boot.existing_volume" PASS "restarted onto the volume it already wrote"
  else
    verdict "boot.existing_volume" FAIL "did not become healthy after restart"
  fi
else
  verdict "boot.existing_volume" BLOCKED "compose restart failed"
fi

summary 'the runtime never gains DAC_OVERRIDE; the init that precedes it has no capabilities.'
