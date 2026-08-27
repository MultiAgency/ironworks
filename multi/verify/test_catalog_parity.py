#!/usr/bin/env python3
# CATALOG PARITY PROBE — the tenant-shared tool/skill surface must be client-agnostic.
#
# Established by reading the source: the operator tool catalog and /tenant-shared/skills are
# tenant-shared — every member sees the SAME surface (verified: operator_tool_catalog.rs
# list_operator_tools "read by any authenticated member"; runtime_mounts.rs tenant-shared skills
# read-only, no writer on production). That is correct for this product (one analyst persona for
# all clients) — the risk is the inverse: a future operator seeding /tenant-shared/skills or a
# registry tool with ONE client's material, which would then be visible to ALL clients.
#
# This probe captures the tool + skill surface visible to client A and client B and asserts it is
# (a) byte-identical across the two clients and (b) free of any client-name marker. A divergence
# or a client name in the shared catalog is the finding.
#
# NOTE these endpoints are the settings/skills READ catalog (member-readable). The admin/write
# side (POST/PUT) is asserted operator-only by test_member_admin_negative.py — a different claim.
#
# Prereqs: MT instance on :3020, two provisioned clients (proof-a / proof-b, or any two). Run:
#   IRONCLAW_API=http://127.0.0.1:3020 python3 test_catalog_parity.py
import sys, json, pathlib
ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi/seam"))
from common import Checks, get  # noqa: E402

CATALOG_ROUTES = ["/api/webchat/v2/settings/tools", "/api/webchat/v2/skills"]

checks = Checks()
check = checks.check
block = checks.block


def two_clients():
    try:
        import context_ingress as ing
        clients = ing.load_clients()
        picks = sorted(clients.values(), key=lambda c: c.slug)
        return picks[:2] if len(picks) >= 2 else None
    except Exception as e:
        print(f"     (client registry unavailable: {e})"); return None

def canon(obj):
    """Order-independent canonical form so a reordered-but-equal catalog still compares equal."""
    return json.dumps(obj, sort_keys=True)

print("== tool/skill catalog is identical across clients and client-agnostic ==")
pair = two_clients()
if pair is None:
    block("catalog parity across clients", "need two provisioned clients / instance unreachable")
else:
    A, B = pair
    names = {A.slug, B.slug, getattr(A, "name", ""), getattr(B, "name", "")}
    names = {n for n in names if n and len(n) > 3}
    for route in CATALOG_ROUTES:
        try:
            a = get(route, A.ironclaw_token)
            b = get(route, B.ironclaw_token)
        except Exception as e:
            block(f"catalog parity {route}", f"fetch failed (member may lack read access?): {e}")
            continue
        check(f"{route}: identical for both clients", canon(a) == canon(b),
              "surfaces differ across clients")
        blob = canon(a) + canon(b)
        stray = sorted(n for n in names if n in blob)
        check(f"{route}: no client-name marker in shared catalog", not stray, str(stray))

checks.finish("shared catalog is uniform and client-agnostic")
