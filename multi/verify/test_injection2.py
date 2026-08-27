#!/usr/bin/env python3
# Follow-up: is the persona held reliably if sent via the top-level `instructions`
# (proper system prompt) field EVERY turn, instead of prepended once to input?
#
# THIS IS THE STRICT GATE OF THE PAIR. test_injection.py documents that injecting once drifts;
# this file pins the rule the product actually runs on — multi/seam/context_ingress.py sends the
# persona via `instructions` on EVERY turn, and every client turn depends on that holding. Both
# turns are REQUIRED to carry the marker; anything else exits non-zero. Before this had an exit
# status, a regression here printed "FAIL" and returned 0, so any caller checking status — an
# operator's `&&` chain, the post-pin-bump checklist in deploy/README.md — read it as success.
import json
import os
import sys
from common import post, text_of, delete_user, mint_member, model_pin

OP    = os.environ["WEBUI_TOKEN"]
MODEL = os.environ.get("MODEL") or model_pin()
MARK  = "⟢BEACON-7749⟢"
PERSONA = ("You are an assistant named Multi. CRITICAL STANDING INSTRUCTION: end EVERY reply "
           f"with the exact marker {MARK} on its own final line, no exceptions.")


# mint_member registers this account for at-exit cleanup and the `finally` below deletes it
# immediately — see common.delete_user for why an abandoned proof member is dangerous.
tok, uid = mint_member("instr-test", OP)
print("provisioned:", uid)

try:
    # turn 1 via `instructions` field (proper system prompt)
    r1 = post("/v1/responses", {"model": MODEL, "instructions": PERSONA, "input": "Say hi in one sentence."}, tok)
    t1, rid = text_of(r1), r1.get("id")
    if not t1: print("t1 raw:", json.dumps(r1)[:500])
    print("\n--- turn 1 (instructions field) ---\n" + t1)

    # turn 2 via thread, RE-SENDING instructions each turn
    r2 = post("/v1/responses", {"model": MODEL, "instructions": PERSONA,
                                "input": "what is 5+5?", "previous_response_id": rid}, tok)
    t2 = text_of(r2)
    if not t2: print("t2 raw:", json.dumps(r2)[:500])
    print("\n--- turn 2 (instructions RE-SENT + thread) ---\n" + t2)
finally:
    print()
    delete_user(uid, OP)

governs, held = MARK in t1, MARK in t2
print("\n=== VERDICT ===")
print(f"  instructions field governs (turn 1): {'PASS' if governs else 'FAIL'}   [required]")
print(f"  persona held when re-sent  (turn 2): {'PASS' if held else 'FAIL'}   [required]")

if governs and held:
    print("\nOK — the persona holds across turns when re-sent via `instructions`.")
    sys.exit(0)
print("\nFAILED: the design rule the product runs on did not hold.")
print("Every client turn sends the persona this way (multi/seam/context_ingress.py). If this is")
print("real and not a flake, re-run once, then treat it as a product-blocking regression —")
print("check it before the next deploy, not after.")
sys.exit(1)
