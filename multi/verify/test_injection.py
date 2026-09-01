#!/usr/bin/env python3
# Empirical test: does a CHANNEL-injected persona govern the agent on a hosted
# multi-tenant IronClaw instance (whose baked identity source is Empty)?
# Replicates the onboarding bot's method: persona prepended to /v1/responses `input`
# on turn 1, then carried by previous_response_id (persona NOT re-sent on turn 2).
#
# EXIT STATUS — read this before changing an assertion. Turn 2 here is EXPECTED TO DRIFT: the
# whole point of this file is that injecting the persona once and leaning on
# `previous_response_id` was not observed to hold it. So "exit non-zero if anything says FAIL"
# would report a HEALTHY system as broken. Each verdict is checked against its EXPECTED value
# instead. (See the MEASURED note below before treating the drift as current.)
#
# Only turn 1 is REQUIRED: if a channel-injected persona cannot govern even the turn it was
# injected into, the instance or this harness is broken and that is a real failure. Turn 2 is
# recorded as an OBSERVATION — one model reply is not a deterministic measurement, and a
# single non-drifting reply is news to investigate (the design rule this pair justifies might
# no longer need to be what it is), not a build break. test_injection2.py is the strict gate:
# it pins the rule the product actually depends on.
#
# MEASURED 2026-08-26, pinned rev + MODEL_PIN, live MT instance: turn 2 did NOT drift, four runs
# out of four. The "expected: drifts" above no longer reproduces here — read it as the HISTORY
# that motivated the design, not as a current prediction. (README.md's table no longer states a
# per-turn expectation either; the citation above to "turn 1 PASS, turn 2 FAIL" outlived the
# wording it quoted, which is its own small lesson about quoting a sibling doc.)
#
# WHAT THAT DOES AND DOES NOT CHANGE. It does not change the product: `context_ingress` sends the
# persona via `instructions` EVERY turn, and that stays, because it is correct whether or not the
# thread happens to carry it — a rule that holds by construction beats one that holds because a
# model currently behaves well, and test_injection2.py pins it either way. What it does change is
# the JUSTIFICATION: "otherwise it drifts" is no longer something this file demonstrates. Anyone
# citing that as evidence should re-measure first. Four runs on one model is a signal, not a
# retirement notice for the observation — a different model, or a longer thread, may drift again.
import json
import os
import sys
from common import (post, text_of, delete_user, mint_member, model_pin,
                    INJECTION_MARK as MARK, INJECTION_PERSONA as PERSONA)

OP    = os.environ["WEBUI_TOKEN"]            # operator token (sourced from env; never printed)
MODEL = os.environ.get("MODEL") or model_pin()


# 1. fresh sealed test account. mint_member registers it for at-exit cleanup; the `finally`
# below deletes it immediately, which is the same discipline stated once instead of per proof.
tok, uid = mint_member("injection-test", OP)
print("provisioned sealed test account:", uid)

try:
    # 2. turn 1 — inject persona
    r1 = post("/v1/responses", {"model": MODEL, "input": PERSONA + "\n\nSay hello in one short sentence."}, tok)
    t1, rid = text_of(r1), r1.get("id")
    if not t1: print("turn1 raw (truncated):", json.dumps(r1)[:600])
    print("\n--- turn 1 reply (persona injected) ---\n" + t1)

    # 3. turn 2 — follow-up via thread; persona NOT re-sent
    r2 = post("/v1/responses", {"model": MODEL, "input": "THEM: what is 2+2?", "previous_response_id": rid}, tok)
    t2 = text_of(r2)
    if not t2: print("turn2 raw (truncated):", json.dumps(r2)[:600])
    print("\n--- turn 2 reply (persona NOT re-sent; carried by thread) ---\n" + t2)
finally:
    print()
    delete_user(uid, OP)

# verdict — each checked against its EXPECTED value (see the header)
governs, carried = MARK in t1, MARK in t2
print("\n=== VERDICT (hosted multi-tenant, baked identity = Empty) ===")
print(f"  injection governs behavior  (turn 1): {'PASS' if governs else 'FAIL'}   [required: governs]")
print(f"  thread carries the persona  (turn 2): {'PASS' if carried else 'FAIL'}   [expected: drifts]")

if not governs:
    print("\nFAILED: a channel-injected persona did not govern the turn it was injected into.")
    print("That is the instance or this harness, not the design rule — check the model and token.")
    sys.exit(1)
if carried:
    print("\nNOTE: turn 2 did NOT drift this run. Expected, documented behaviour is that it does")
    print("(verify/README.md). One reply is not a measurement — re-run before concluding anything.")
    print("If it holds consistently, the premise behind 'persona via instructions EVERY turn' has")
    print("changed upstream and the design rule deserves a fresh look. Not a build break.")
print("\nOK — injection governs on the turn it is injected into.")
sys.exit(0)
