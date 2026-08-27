#!/usr/bin/env bash
# confine-member.sh — lock a sealed MT client member down to a read-only, NO-EGRESS surface.
#
# WHY: a freshly minted IronClaw member ships the full default tool catalog, including
# `builtin.http` (always_allow) with a COMPILED-IN wildcard egress policy. ironclaw exposes no
# config knob to narrow it (`include_str!` in builtin_capability_policy.rs; no RuntimeProfile
# maps to NetworkMode::Deny) and the product runs ironclaw UNMODIFIED — so per-bearer tool
# state is the ONLY lever. Otherwise an injected turn could POST that client's pipeline to an
# attacker host.
#
# WHY THE CLIENT CANNOT UNDO IT:
#   - tool state is set via POST /settings/tools/{id}, which needs the MEMBER TOKEN. That token
#     lives only in the trusted seam, never reaches the client, and is never in the model's
#     request — so neither can call the route.
#   - the model's in-turn toolset excludes operator_config_set_tool_permission and
#     admin_configuration_replace, so an injected turn cannot re-grant from inside either. And
#     re-enabling would itself need egress it no longer has.
#
# CONFINE BY ALLOWLIST, not deny-list: disable every tool NOT in KEEP. The tool set drifts
# between releases, and an allowlist denies any NEW tool by default where a deny-list would let
# a newly-added egress tool through until someone remembers to list it.
#
# SCOPE — READ THIS BEFORE QUOTING THE GUARANTEE. "Every tool not in KEEP is disabled" is true
# of the CATALOG this reads (/settings/tools). The catalog is NOT the whole surface the model is
# offered. Measured on a confined member at the pinned rev: 50 catalog entries (37 disabled) vs
# 17 tools offered to the model, five of which were in no catalog at all (result_read,
# outbound_delivery_targets_list, project_create, skill_activate, capability_info). This script
# cannot disable what nothing lists, so its success line means "every CATALOGUED tool outside
# KEEP is disabled", not "the member holds exactly len(KEEP) capabilities".
#
# What keeps that from being load-bearing: `disabled` is enforced at DISPATCH, not just
# advertised — a disabled tool still appears on the model's surface but returns `policy_denied`
# when called (reproduced twice live) — and the egress guarantee is proven by OUTCOME in
# multi/verify/test_egress_closed.py, which does not consult the catalog at all. That is why
# UPGRADE.md step 6 requires both this script AND that probe, and says neither implies the other.
# If you ever need "what can this member actually call", ask the model to enumerate its surface;
# do not infer it from the catalog.
#
# It does not degrade the product — measured, not assumed: with this KEEP list applied, the
# headline suite still passes (test_two_clients, test_adversarial_cross_org,
# test_client_guidance_live, test_product_loop). Re-run those four after any change to KEEP;
# a shrunken allowlist that breaks the product is not a security win.
#
# Applied WITH THE MEMBER'S OWN TOKEN. Then PROBED and FAIL-CLOSED: re-reads the live surface
# and exits non-zero if ANY non-KEEP tool is still callable, or if the surface could not be
# read (no silent pass on an error body).
#
# Usage:
#   IRONCLAW_API=http://127.0.0.1:3020 IRONCLAW_MEMBER_TOKEN=<member token> ./confine-member.sh
#   (token via env, never argv, so it stays out of the process table)
set -euo pipefail
API="${IRONCLAW_API:?set IRONCLAW_API (the multi-tenant instance base URL)}"
API="${API%/}"
: "${IRONCLAW_MEMBER_TOKEN:?set IRONCLAW_MEMBER_TOKEN (the sealed member token; via env, not argv)}"

# Minimal read-only, no-egress analyst surface (settings tool ids). Everything else is disabled.
KEEP_DEFAULT="builtin.echo builtin.time builtin.json builtin.read_file builtin.list_dir \
builtin.glob builtin.grep builtin.skill_list \
ironclaw.memory.read ironclaw.memory.search ironclaw.memory.tree"
KEEP="${CONFINE_KEEP:-$KEEP_DEFAULT}"

# RE-RUNNING IS FREE. This used to POST a disable for every denied tool on every run, which cost
# one request per tool per client even when the surface was already correct, and tripped the
# API's rate limiter on a back-to-back re-run (observed: HTTP 429). That failed
# closed and read like a confinement failure when nothing was wrong — and deploy/README.md
# step 6a makes exactly that re-run MANDATORY after every pin bump, so the documented remedy
# was to re-run into the same wall. Now only tools not ALREADY disabled are POSTed, so a re-run
# against an unchanged surface sends zero writes. This is what a CONFINE_VERIFY_ONLY flag was
# added for; the flag is gone, because callers no longer have to remember it.
#
# The guarantee is unchanged: the probe below re-reads the surface and checks EVERY denied tool,
# not just the ones written this run. And a pin bump that introduces new or renamed tools still
# confines them — a new tool is not already `disabled`, so it lands in `pending`.
API="$API" TOKEN="$IRONCLAW_MEMBER_TOKEN" KEEP="$KEEP" \
LIB_DIR="$(cd "$(dirname "$0")/../../deploy/lib" && pwd)" \
SEAM_DIR="$(cd "$(dirname "$0")/../seam" && pwd)" \
python3 - <<'PY'
import json, os, sys, urllib.request, urllib.error

sys.path.insert(0, os.environ["LIB_DIR"])
from tool_surface import parse_catalog, egress_observed_off   # ONE fail-closed catalog reader
# ...and the seam's User-Agent, for the same reason: this script talks to the same instance the
# product does, so it must not be a third spelling of the header. It was one — the full string
# minus the `AppleWebKit`/`KHTML` tokens — guarded by nothing. `responses` is the zero-import-
# weight module that exists so the product and the tooling cannot diverge here; importing a
# product module from operator tooling is the permitted direction (CLAUDE.md).
sys.path.insert(0, os.environ["SEAM_DIR"])
from responses import BROWSER_UA

API = os.environ["API"]
TOKEN = os.environ["TOKEN"]
KEEP = set(os.environ["KEEP"].split())

def _req(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(API + path, data=data, method=method,
                               headers={"Authorization": "Bearer " + TOKEN,
                                        "User-Agent": BROWSER_UA, "content-type": "application/json"})
    with urllib.request.urlopen(r, timeout=20) as x:
        raw = x.read()
    return json.loads(raw) if raw else {}

def catalog():
    return parse_catalog(_req("GET", "/api/webchat/v2/settings/tools"), "confine-member")

# Operator-tier config tools that STRUCTURALLY reject `disabled` (HTTP 400 invalid_value) and are
# NOT exposed in a member turn's model toolset (verified against the live member surface). They
# cannot be removed, but they cannot auto-run either: confinement requires only that they never sit
# at `always_allow` (their default is `ask_each_time`, which cannot fire in the headless seam).
NON_DISABLEABLE = {"builtin.operator_config_set_tool_permission", "builtin.operator_config_set_auto_approve"}

before = catalog()
targets = sorted(t for t in before if t not in KEEP and t not in NON_DISABLEABLE)
kept = sorted(t for t in before if t in KEEP)
# Only what actually needs changing. `targets` is still the full denied set and is what the
# probe checks at the end; `pending` is just the write list.
pending = [t for t in targets if before.get(t) != "disabled"]
print(f"== confine member: {len(before)} tools live; keeping {len(kept)}, "
      f"denying {len(targets)} ({len(pending)} to write, {len(targets) - len(pending)} already disabled) ==")

for t in pending:
    try:
        _req("POST", f"/api/webchat/v2/settings/tools/{t}", {"state": "disabled"})
    except urllib.error.HTTPError as e:
        # 400 = the settings store will not take an override for this id. Tolerated because the
        # probe below is the truth. Do NOT read it as "so the tool is uncallable": measured, a
        # capability id the catalog does not carry returns 400 `unknown_key` while remaining
        # fully callable by the model. Absent from the catalog means UNGATEABLE here, not absent
        # from the surface. See the SCOPE note at the top.
        if e.code != 400:
            raise SystemExit(f"!! confine-member: disabling {t} -> HTTP {e.code} (fail closed)")

# --- PROBE, don't trust: re-read the live surface and fail closed on any survivor ---
after = catalog()
still = sorted(t for t, st in after.items() if t not in KEEP and t not in NON_DISABLEABLE and st != "disabled")
if still:
    raise SystemExit("!! confine-member: CONFINEMENT INCOMPLETE — still callable: " + ", ".join(still))
# the non-disableable operator tools must never be auto-run (always_allow); ask_each_time/default are safe headless
hot = sorted(t for t in NON_DISABLEABLE if after.get(t) == "always_allow")
if hot:
    raise SystemExit("!! confine-member: operator-config tool at always_allow (can auto-run): " + ", ".join(hot))

# Non-vacuous check: the load-bearing egress tools MUST be observed disabled (proves we actually
# read a real surface and acted, not that they were merely absent).
seen_off = egress_observed_off(after, "confine-member")

print(f"   verified: every non-allowlisted tool disabled (egress off: {', '.join(s.split('.')[-1] for s in seen_off)})")
print(f"   member surface now read-only: {', '.join(kept)}")
PY
