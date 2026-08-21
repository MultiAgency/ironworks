#!/usr/bin/env python3
# TWO-CLIENT SERVED-PRODUCT PROOF — the multi-tenant analogue of test_product_loop.py's 7/7.
# Drives the REAL seam (context_ingress, not a re-implementation) for two provisioned clients
# with disjoint orgs (provision.sh proof-a / proof-b, seeded Northwind vs Studio Vireo) and
# proves, end-to-end through sealed accounts on the live MT instance:
#   (a) client A sees only A's accounts, with the persona governing (evidence tags)
#   (b) client B likewise
#   (c) A asking about B's account by name gets NO context (isolation surfaces end-to-end)
#   (d) no client's tokens appear in any request body sent to IronClaw
#
# Prereqs: MT instance on :3020, Account Service on :8443, and both proof clients provisioned:
#   IRONCLAW_API=http://127.0.0.1:3020 IRONCLAW_OPERATOR_TOKEN=... \
#     ../provision/provision.sh proof-a "Proof Client A" -100900011   (data: northwind.json)
#     ../provision/provision.sh proof-b "Proof Client B" -100900012   (data: vireo.json)
# Run:  IRONCLAW_API=http://127.0.0.1:3020 python3 test_two_clients.py
import os, sys, json, pathlib

os.environ.setdefault("IRONCLAW_API", "http://127.0.0.1:3020")
ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi/seam"))
import context_ingress as ing

clients = ing.load_clients()
missing = [s for s in ("proof-a", "proof-b") if s not in clients]
if missing:
    sys.exit(f"!! provision first (see header): missing {missing} in the client registry")
A, B = clients["proof-a"], clients["proof-b"]

# capture every body the seam sends to IronClaw (for the token-leak check), pass through live
sent = []
_orig_post = ing._post_ironclaw
def _recording_post(body, client=None, attempts=4):
    sent.append(json.dumps(body))
    return _orig_post(body, client, attempts)
ing._post_ironclaw = _recording_post

TELLS = ("FACT", "UNKNOWN", "INFERENCE")   # the persona's evidence-discipline tags
from common import Checks   # the tick-list; this file keeps its own verdict line

checks = Checks()
check = checks.check


print("== (a) client A: own-org view, persona governs ==")
tha = ing.Thread(A)
text_a, supplied_a = ing.turn(tha, "Which of these prospects should we focus on?")
check("A was supplied only A's accounts", supplied_a == ["NW-001"], str(supplied_a))
check("A's reply names Northwind", "Northwind" in text_a, text_a[:200])
check("A's reply never mentions B's account", "Vireo" not in text_a and "SV-004" not in text_a)
check("persona governs A (evidence tags)", any(t in text_a for t in TELLS))

print("== (b) client B: own-org view, persona governs ==")
thb = ing.Thread(B)
text_b, supplied_b = ing.turn(thb, "Which of these prospects should we focus on?")
check("B was supplied only B's accounts", supplied_b == ["SV-004"], str(supplied_b))
check("B's reply names Studio Vireo", "Vireo" in text_b, text_b[:200])
check("B's reply never mentions A's account", "Northwind" not in text_b and "NW-001" not in text_b)
check("persona governs B (evidence tags)", any(t in text_b for t in TELLS))

print("== (c) A asks about B's account by name ==")
text_x, supplied_x = ing.turn(tha, "What's the latest on Studio Vireo?")
check("seam supplied A no context for B's account", supplied_x == [], str(supplied_x))
check("B's record id never reaches A", "SV-004" not in text_x)

print("== (d) no client token in any IronClaw request body ==")
blob = "".join(sent)
leaks = [f"{c.slug}.{k}" for c in (A, B)
         for k, v in (("ironclaw_token", c.ironclaw_token), ("account_token", c.account_token))
         if v in blob]
check("no tokens in any request body", not leaks, str(leaks))

print(f"\nscore: {checks.passed}/{checks.ran} — two clients served, sealed, and governed"
      if checks.ok else f"\nFAIL: {checks.ran - checks.passed} of {checks.ran} checks failed")
sys.exit(0 if checks.ok else 1)
