#!/usr/bin/env python3
# END-TO-END PRODUCT PROOF (hosted multi-tenant, verified model):
#   sealed account  +  persona via `instructions`  +  client context via `input`
#   -> does the analyst deliver grounded account intelligence over THIS client's data?
#
# EXERCISES THE SHIPPED PATH. The persona here is the CLIENT composition
# (compose_client_persona: the generic analyst + that client's own guidance), which is what
# every registry client actually receives — `load_clients()` composes nothing else. This proof
# used to run the INTERNAL composition instead, so the repo's headline end-to-end evidence
# validated a persona no client is ever served. The fixtures are the committed clean-clone kit
# (`fixtures/clients/proof-a.*`), the same guidance+book pairing test_two_clients.py provisions.
#
# EXIT STATUS. Two required checks: the reply must be non-empty, and the context-tell score must
# clear a FLOOR of 5 of 7. verify/README.md records the measured result as 7/7 — the gate sits
# below it deliberately, because these tells are heuristics over model prose and a gate pinned to
# a best-ever run fails a healthy system on ordinary variation. A score under 7 is still printed
# loudly: it is the signal to read the transcript, which is what this proof is for. Same stance
# as multi/eval/: a check that cannot fail manufactures confidence, and one that fails on noise
# gets ignored.
import os, sys, json, pathlib
from common import post, text_of, delete_user, mint_member, model_pin

FLOOR = 5

API   = "http://127.0.0.1:3020"
OP    = os.environ["WEBUI_TOKEN"]


MODEL = os.environ.get("MODEL") or model_pin()
ROOT  = pathlib.Path(__file__).resolve().parents[2]

# 1) the injected persona = what the CHANNEL supplies (instance bakes none on hosted-MT).
# compose_client_persona is the function load_clients() calls per client, so proof and product
# cannot diverge. Reads a committed guidance file — no provisioned registry needed, so this
# stays an instance-tier proof.
sys.path.insert(0, str(ROOT / "multi/seam"))
from persona import compose_client_persona  # noqa: E402
FIX = pathlib.Path(__file__).resolve().parent / "fixtures" / "clients"
persona = compose_client_persona(str(FIX / "proof-a.guidance.md"), "proof-a", ROOT)

# 2) trusted-context envelope built by the REAL seam code — same reason compose_persona is
# imported: the proof must exercise the product's envelope, not a hand-maintained replica.
import context_ingress as ing  # noqa: E402  (needs env + sys.path set first)

nw = json.loads((FIX / "proof-a.account.json").read_text())
acc = nw["account"]
nw["missing"] = [k for k in ("budget", "timeline", "decision_process", "economic_buyer")
                 if acc.get(k) is None]
QUESTION = "What's the state of the Northwind Labs account — is it worth pursuing, and what should I do next?"
inp = ing.build_envelope(QUESTION, [nw], "proof-a")


# mint_member registers this account for at-exit cleanup and the `finally` below deletes it
# immediately — see common.delete_user for why neither confine-existing.sh nor
# test_egress_closed.py would ever find an abandoned one.
tok, uid = mint_member("product-loop", OP)
print("sealed account:", uid, "| persona injected:", len(persona), "chars")
try:
    r = post("/v1/responses", {"model": MODEL, "instructions": persona, "input": inp}, tok, timeout=180)
    reply = text_of(r)
    print("\n===== MULTI'S REPLY =====\n" + reply)
finally:
    print()
    delete_user(uid, OP)

# heuristic 'context tells' — did it USE the injected facts vs. give generic advice?
low = reply.lower()
tells = {
    "cites the 500+ ticket pain":        "500" in reply,
    "names the contact (Dana Reyes)":    "dana" in low,
    "flags the genuinely-missing fields": any(w in low for w in ["budget", "timeline", "helpdesk", "decision process"]),
    # Guidance-relative, NOT offering-specific. The old form looked for a match to MultiAgency's
    # own offering, which only made sense under the internal composition. Under a client's
    # guidance the correct answer may well be "deprioritize — this is not our ICP", and that is
    # a GOOD answer the old tell scored as a miss. What must hold is that the analyst reasons in
    # the vocabulary its guidance defines (proof-a: Alpine's stages) rather than a generic one.
    "reasons in its guidance's vocabulary": any(w in low for w in ["technical-fit", "pilot", "discovery", "deprioriti", "alpine", "subscription", "renewal"]),
    "uses evidence tags (FACT/UNKNOWN)": any(t in reply for t in ["FACT", "UNKNOWN", "INFERENCE"]),
    "asks a discovery question":         "?" in reply,
    "recommends a next step":            any(w in low for w in ["next step", "discovery", "advance", "intake", "recommend"]),
}
print("\n===== CONTEXT TELLS =====")
for k, v in tells.items():
    print(f"  [{'x' if v else ' '}] {k}")
score = sum(tells.values())
print(f"\nscore: {score}/{len(tells)} — grounded-in-context if most are checked (floor: {FLOOR})")

if not reply.strip():
    print("\nFAILED: empty reply — the loop did not produce an answer at all.")
    sys.exit(1)
if score < FLOOR:
    print(f"\nFAILED: {score}/{len(tells)} is below the {FLOOR}/7 floor — the reply is not")
    print("visibly working from the injected context. Read the transcript above: the failure is")
    print("either the persona composition, the envelope, or the model, and only reading tells you")
    print("which. A number here is not a grade; it is a prompt to look.")
    sys.exit(1)
if score < len(tells):
    print(f"\nPASS (above floor), but {len(tells) - score} tell(s) missed — verify/README.md")
    print("records this proof at 7/7. Worth a look before it drifts further.")
print("\nOK — the product loop delivered grounded, context-derived analysis.")
sys.exit(0)
