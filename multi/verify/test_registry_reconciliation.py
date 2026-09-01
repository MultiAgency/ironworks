#!/usr/bin/env python3
"""Registry <-> instance reconciliation: do the two agree about who exists?

WHY THIS EXISTS. Every tool that certifies something about "the members" derives its member set
from the CLIENT REGISTRY:

    multi/provision/confine-existing.sh:25   envs=("$CLIENTS_DIR"/*.env)
    multi/verify/test_egress_closed.py       clients = ing.load_clients()

So "egress is closed" has never meant what its name implies at the INSTANCE level. It means
"every member with a registry .env is confined" — a scope neither the name nor the output states,
and one a reader will not infer. Anything that mints a member outside the registry is invisible to
both. That is not hypothetical: an instance was found holding 43 members, 41 of which the
registry had never heard of, accumulated over weeks by proof scripts that minted and never
cleaned up. Nothing reported it, because nothing was looking.

This closes that by asserting the invariant the other proofs quietly assume:

    A. NO UNKNOWN MEMBERS   — every member on the instance is one the registry knows about.
                              Without this, "every registry member is confined" and "every member
                              is confined" are different sentences, and only the weaker one is
                              ever proven.
    B. NO DANGLING ENTRIES  — every registry entry resolves to a member that actually exists.
                              A dangling entry means the seam is holding credentials for an
                              account that is gone.

WHY NOT PROBE EACH MEMBER'S CONFINEMENT INSTEAD. Because you cannot. Tool state is read via
GET /settings/tools and is CALLER-scoped — you get the catalog of whoever holds the bearer, and
confining requires the member's own token. The operator cannot scope that read to another user;
measured on a live instance:

    GET /settings/tools                                  -> 200, sha 3f0dbc627129d87a
    GET /settings/tools?user_id=<a real member uid>      -> 200, sha 3f0dbc627129d87a
    GET /settings/tools?user_id=totally-made-up-nonsense -> 200, sha 3f0dbc627129d87a
    GET /admin/users/<id>/settings/tools                 -> 404

Identical bodies including for a garbage id: the parameter is SILENTLY IGNORED, not honoured. A
proof that enumerated members and "checked" each one this way would read the operator's own
catalog N times and certify every abandoned account as confined. Do not build that. Set equality
is the checkable property; per-member confinement is not, once the tokens are gone.

RUN IT ON THE BOX WHOSE REGISTRY IS AUTHORITATIVE FOR THAT INSTANCE. The registry lives outside
the repo (`~/.agency/clients`), it is per-machine, and it does not sync — so pointing a laptop
registry at a shared instance reports the instance's real members as "unknown" and is a false
positive, not a finding. Observed: after a client's `.env` was removed from the laptop
as a stale divergent copy (the serve VM's is authoritative), this check reported the live
production member as unknown on the laptop while being entirely correct on the VM. Both sides are
per-box; compare like with like.

Needs: WEBUI_TOKEN (operator) + the MT instance. CLIENTS_DIR default ~/.agency/clients.
Run:   WEBUI_TOKEN=... python3 multi/verify/test_registry_reconciliation.py
"""
import os
import pathlib
import sys
import urllib.error

from common import DEFAULT_API, get

OP = os.environ.get("WEBUI_TOKEN")
if not OP:
    sys.exit("!! reconciliation: WEBUI_TOKEN (operator) is required — refusing to guess")
CLIENTS_DIR = pathlib.Path(os.environ.get("CLIENTS_DIR")
                           or pathlib.Path(os.environ.get("AGENCY_DIR")
                                           or pathlib.Path.home() / ".agency") / "clients")


def registry():
    """slug -> (user_id, field_present). Parsed straight from the .env files: load_clients()
    deliberately does not expose IRONCLAW_USER_ID, and adding a field to ClientConfig just to
    run a check would put instance identity into the seam's hot path for no product reason."""
    out = {}
    for f in sorted(CLIENTS_DIR.glob("*.env")):
        kv = {}
        for line in f.read_text().splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                k, _, v = line.partition("=")
                kv[k.strip()] = v.strip().strip('"')
        slug = kv.get("CLIENT_SLUG") or f.stem
        uid, present = kv.get("IRONCLAW_USER_ID", ""), True
        if not uid:
            # Fall back to asking the instance who this token is. Recorded as MISSING either way:
            # deprovision.sh:57 reads this field and :144 takes a SKIPPED branch without it, so an
            # entry lacking it will NOT have its sealed account deleted at teardown. Found live on
            # a production entry — entries predating the provision.sh:87 guard lack it.
            present = False
            tok = kv.get("IRONCLAW_TOKEN", "")
            if tok:
                try:
                    uid = get("/api/webchat/v2/session", tok).get("user_id", "")
                except (urllib.error.HTTPError, OSError):
                    uid = ""
        out[slug] = (uid, present)
    return out


def instance_members():
    d = get("/api/webchat/v2/admin/users", OP)
    us = d.get("users", d if isinstance(d, list) else [])
    if not us:
        # Fail closed, same stance as test_egress_closed.py: an empty catalog is far more likely
        # to be a broken read than a genuinely empty instance, and "0 unknown members" would be a
        # perfect score derived from no evidence.
        sys.exit("!! reconciliation: instance returned no members — refusing to certify (fail closed)")
    return {u.get("user_id"): (u.get("display_name") or "?") for u in us}


reg = registry()
if not reg:
    sys.exit(f"!! reconciliation: no client envs in {CLIENTS_DIR} — nothing to reconcile (fail closed)")
members = instance_members()

print(f"== registry <-> instance reconciliation — {DEFAULT_API} ==")
print(f"   registry: {len(reg)} client(s) in {CLIENTS_DIR}")
print(f"   instance: {len(members)} member account(s)")

missing_field = [s for s, (_, present) in reg.items() if not present]
unresolved = [s for s, (uid, _) in reg.items() if not uid]
known = {uid for uid, _ in reg.values() if uid}

unknown = {uid: name for uid, name in members.items() if uid not in known}
dangling = {s: uid for s, (uid, _) in reg.items() if uid and uid not in members}

print("\n-- A. members the registry does not know about --")
if unknown:
    import collections
    for name, n in collections.Counter(unknown.values()).most_common():
        print(f"   {n:>3}  {name}")
    print(f"   TOTAL {len(unknown)} — each is a member the confinement back-fill and the egress")
    print("   proof cannot see, because both enumerate the registry.")
else:
    print("   none — the instance holds no member outside the registry")

print("\n-- B. registry entries pointing at members that no longer exist --")
if dangling:
    for slug, uid in sorted(dangling.items()):
        print(f"   {slug:<14} -> {uid}  (no such member)")
    print("   The seam still holds credentials for these. NOTE: a deleted account's token keeps")
    print("   authenticating until it expires — deletion is not revocation (deprovision.sh:133).")
else:
    print("   none — every registry entry resolves to a live member")

if missing_field:
    print(f"\n-- C. registry entries with no IRONCLAW_USER_ID: {', '.join(sorted(missing_field))}")
    print("   deprovision.sh cannot delete their sealed account (it takes the SKIPPED branch at")
    print("   :144), so teardown revokes the org token and silently leaves the member alive.")
if unresolved:
    print(f"   UNRESOLVABLE (no id, and the token did not answer): {', '.join(sorted(unresolved))}")

fails = []
if unknown:
    fails.append(f"{len(unknown)} unknown member(s)")
if dangling:
    fails.append(f"{len(dangling)} dangling registry entry/entries")
if missing_field:
    fails.append(f"{len(missing_field)} entry/entries without IRONCLAW_USER_ID")

print()
if fails:
    print("FAILED: " + "; ".join(fails))
    sys.exit(1)
print("OK — registry and instance agree on exactly who exists.")
sys.exit(0)
