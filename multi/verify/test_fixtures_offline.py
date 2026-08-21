#!/usr/bin/env python3
# OFFLINE fixtures proof — pins that the committed proof-client fixtures load through the
# REAL registry loader from a clean clone: no instance, no Account Service, no credentials.
#
# What it proves (the clean-clone contract):
#   (a) fixtures/clients/ templates + guidance produce a registry that load_clients()
#       accepts via CLIENTS_DIR (the same override the live suite uses),
#   (b) each proof client's composed persona carries ITS OWN synthetic guidance and not
#       the other's, and no MultiAgency-internal guidance leaks in (the same assertions
#       test_client_guidance_live.py makes live),
#   (c) the committed templates carry placeholders, never real-shaped secrets.
#
# Run: python3 test_fixtures_offline.py       (from multi/verify/; exits non-zero on failure)
import pathlib
import re
import shutil
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures" / "clients"
sys.path.insert(0, str(HERE.parent / "seam"))

from common import Checks   # the tick-list; this file keeps its own verdict line below

checks = Checks()
check = checks.check

print("== committed templates are placeholders, not secrets ==")
for slug in ("proof-a", "proof-b"):
    text = (FIXTURES / f"{slug}.env.template").read_text()
    check(f"{slug}: both required tokens are <REQUIRED …> placeholders",
          text.count("<REQUIRED") == 2)
    check(f"{slug}: no hex-token-shaped value present",
          not re.search(r"=[\"']?[0-9a-f]{24,}", text))
    check(f"{slug}: synthetic group id", re.search(r'TELEGRAM_GROUP_ID="-1009000\d\d"', text))

print("== fixtures load through the real loader (CLIENTS_DIR) ==")
with tempfile.TemporaryDirectory() as td:
    tdp = pathlib.Path(td)
    for slug in ("proof-a", "proof-b"):
        env = (FIXTURES / f"{slug}.env.template").read_text()
        # what an operator does after provisioning: fill the two required tokens
        # Fill each placeholder with a DISTINCT value, as a real operator would: the two
        # credentials are different secrets, and load_clients() rejects a file where they
        # match (cross-wired-credentials guard).
        env = re.sub(r"(ACCOUNT_TOKEN=)<REQUIRED[^>]*>", rf"\1dummy-{slug}-account-token", env)
        env = re.sub(r"(IRONCLAW_TOKEN=)<REQUIRED[^>]*>", rf"\1dummy-{slug}-ironclaw-token", env)
        (tdp / f"{slug}.env").write_text(env)
        shutil.copy(FIXTURES / f"{slug}.guidance.md", tdp / f"{slug}.guidance.md")

    import os
    os.environ["CLIENTS_DIR"] = str(tdp)
    # import-time requirement only; nothing in this test performs a call
    os.environ.setdefault("IRONCLAW_API", "http://127.0.0.1:3020")
    import context_ingress as ing
    clients = ing.load_clients()

    check("both proof clients load", set(clients) == {"proof-a", "proof-b"},
          f"got {sorted(clients)}")
    a, b = clients["proof-a"], clients["proof-b"]
    check("proof-a persona carries its own guidance (Alpine)", "Alpine DevTools" in a.persona)
    check("proof-b persona carries its own guidance (Harbor)", "Harbor Studio Services" in b.persona)
    check("no client persona carries the other's guidance",
          "Harbor" not in a.persona and "Alpine" not in b.persona)
    check("no internal MultiAgency guidance in either persona",
          all(t not in p for p in (a.persona, b.persona) for t in ("MultiAgencyHQ", "service catalog")))
    check("group ids map to the synthetic test ids",
          (a.telegram_group_id, b.telegram_group_id) == ("-100900011", "-100900012"))

print("== committed proof account fixtures carry no MultiAgency-internal framing ==")
# THE REGRESSION THIS PINS. proof-a's book used to be seeded from an operator copy of
# deploy/account-intel/data/candidates/northwind.json — the demo book for MultiAgency's own side
# of the table (invented, like these, but written in the VENDOR's voice) — whose inbound note read
# "reached out after seeing MultiAgency builds custom AI agents". Correct in a book of our own
# prospects; wrong for a synthetic EXTERNAL client, and in direct conflict
# with test_client_guidance_live.py's FORBIDDEN list: its :42 tick demands the model ground in
# A's record while :46 forbids the very string that record contained. The live proof failed
# roughly one run in three on a healthy, correctly-isolated system.
#
# The fix was to commit the proof books instead of leaving them to whatever the operator dropped
# in ~/.agency/account-data/. This check is what keeps them clean: it is offline, so CI runs it,
# and it fails at the FIXTURE rather than waiting for a live proof to flake.
FORBIDDEN_IN_FIXTURES = ("MultiAgencyHQ", "MultiAgency", "Aide", "Multiplex")
for slug in ("proof-a", "proof-b"):
    book = FIXTURES / f"{slug}.account.json"
    check(f"{slug}: committed account fixture exists", book.is_file(), str(book))
    if book.is_file():
        blob = book.read_text()
        hits = [t for t in FORBIDDEN_IN_FIXTURES if t in blob]
        check(f"{slug}: account fixture names no MultiAgency-internal term", not hits, str(hits))

print(f"\n{'PASS' if checks.ok else 'FAIL'}: {checks.passed}/{checks.ran} offline fixture checks")
sys.exit(0 if checks.ok else 1)
