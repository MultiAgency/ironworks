#!/usr/bin/env python3
"""LIVE proof: two registry clients, each with its OWN synthetic business guidance, get
materially different, guidance-governed answers through the REAL seam — and neither is
steered toward MultiAgency's services (the internal-composition leak this guards against).

Needs: the MT instance + Account Service up, and proof-a / proof-b provisioned with
slug-bound guidance files in ~/.agency/clients (see multi/clients/GUIDANCE.template.md).
Run: IRONCLAW_API=http://127.0.0.1:3020 python3 test_client_guidance_live.py
"""
import os, sys, pathlib

os.environ.setdefault("IRONCLAW_API", "http://127.0.0.1:3020")
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "seam"))
import context_ingress as ing

# Terms that mark MultiAgency's INTERNAL selling guidance. None may appear in an
# external client's answer (their synthetic guidance never mentions them).
FORBIDDEN = ("MultiAgencyHQ", "MultiAgency", "Aide", "Multiplex")

from common import Checks   # the tick-list; this file keeps its own verdict line

checks = Checks()
check = checks.check

clients = ing.load_clients()
a, b = clients["proof-a"], clients["proof-b"]

print("== personas are client-specific before any turn ==")
check("proof-a persona carries its own guidance (Alpine)", "Alpine DevTools" in a.persona)
check("proof-b persona carries its own guidance (Harbor)", "Harbor Studio Services" in b.persona)
check("no client persona carries the other's guidance",
      "Harbor" not in a.persona and "Alpine" not in b.persona)
check("no internal MultiAgency guidance in either persona",
      all(t not in p for p in (a.persona, b.persona) for t in ("MultiAgencyHQ", "service catalog")))

# Per-client PRIVATE tokens: the account name and its (single) contact. Both are org-scoped
# rows in the account store, so either one appearing is evidence of grounding in that client's
# own record — and either one appearing in the OTHER client's answer is a cross-org leak.
#
# WHY THIS IS A SET AND NOT ONE STRING. These two ticks used to grep a single account-name
# fragment each, and they picked DIFFERENT POSITIONAL WORDS: "Northwind" is the LEADING word of
# "Northwind Labs", but "Vireo" is the TRAILING word of "Studio Vireo". A model that shortens a
# two-word org to its leading word satisfies A and fails B, so "B grounds in B's org" failed 4
# runs in 6 on a healthy, correctly-isolated system while every isolation tick passed. That was
# a mis-specified assertion, not model flakiness, and the fix is to assert what the tick is
# NAMED for — grounding — rather than one spelling of it. Deliberately NOT a pass floor: three
# of these ticks are the cross-org boundary, and a floor would let them fail while exiting 0.
#
# Tokens must stay DISTINCTIVE. "Support Ops" / "Creative Ops" are also org-private but read as
# generic prose, and a generic token here would red the isolation checks on an innocent answer.
PRIVATE = {
    "proof-a": ("Northwind", "Dana Reyes"),
    "proof-b": ("Vireo", "Jordan Kim"),
}


def grounded(answer, slug):
    """The answer cites at least one of this client's own private records."""
    return any(t in answer for t in PRIVATE[slug])


def leaked(answer, slug):
    """The answer cites ANY of the other client's private records. Strictly stronger than the
    account-name-only check this replaces, which could not have seen a leaked contact name."""
    return [t for t in PRIVATE[slug] if t in answer]


print("== live turns: each client answers from its OWN book under its OWN guidance ==")
ta, tb_ = ing.Thread(a), ing.Thread(b)
ans_a, _ = ing.turn(ta, "Which accounts should we focus on, and what's the recommended next step?", speaker="Pat")
ans_b, _ = ing.turn(tb_, "Which accounts should we focus on, and what's the recommended next step?", speaker="Kim")

check("answers differ", ans_a.strip() != ans_b.strip())
check("A grounds in A's own record", grounded(ans_a, "proof-a"), ans_a[:200])
check("B grounds in B's own record", grounded(ans_b, "proof-b"), ans_b[:200])
check("A never sees B's book", not leaked(ans_a, "proof-b"), ans_a[:200])
check("B never sees A's book", not leaked(ans_b, "proof-a"), ans_b[:200])
check("A's answer is free of MultiAgency-internal steering",
      all(t not in ans_a for t in FORBIDDEN), ans_a[:200])
check("B's answer is free of MultiAgency-internal steering",
      all(t not in ans_b for t in FORBIDDEN), ans_b[:200])
check("A speaks its own guidance's language (stages/terms)",
      any(t in ans_a for t in ("technical-fit", "site", "Alpine", "pilot", "discovery", "deprioriti")), ans_a[:300])
check("B speaks its own guidance's language (stages/terms)",
      any(t in ans_b for t in ("scoping", "brief", "Harbor", "lead", "retainer", "park")), ans_b[:300])

n = checks.passed
print(f"\nscore: {n}/{checks.ran}")
sys.exit(0 if n == checks.ran else 1)
