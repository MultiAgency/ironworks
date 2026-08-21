#!/usr/bin/env python3
# FRESHNESS LIFECYCLE — the full life of one account's `updated_at` through the REAL bridge.
#
# What this pins, end to end, on the production path:
#
#   pre-versioning state  -> the loader REFUSES it (never coerces silently)
#   documented migration  -> {record_id: None}, ever_supplied preserved
#   first turn after it   -> HEALS in exactly one fetch, recording the real version
#   next turn             -> settles, no re-fetch
#   a genuine edit        -> re-reads, unprompted, and records the new version
#   next turn             -> settles again
#
# WHY THIS EXISTS. `_moved` (context_ingress.turn) once required `sent_v is not None`, so an
# UNKNOWN supplied-version was read as "never re-fetch" rather than "re-fetch once". None is
# written two ways, both routine: a turn served before the Account Service emitted `updated_at`,
# and telegram_bridge's documented list->dict migration, which sets every id to None BY DESIGN.
# Either one pinned an account to its first copy for the LIFE of the thread — and
# bridge-threads.json persists, so no restart cleared it. That is the exact failure the
# `updated_at` design replaced, reintroduced through a null-guard that looks defensive.
#
# A unit test covers the logic (test_ingress_fixes.py); it cannot cover the real loader + the real
# migration + a real store. This does. Verified to FAIL against the pre-fix `_moved`
# (turns 1 and 3 both fetch nothing and the version stays None) and pass after it.
#
# SAFETY. BRIDGE_STATE is redirected to a temp file and the redirect is ASSERTED before anything
# runs — the operator's real ~/.agency/bridge-threads.json is never opened. Leg E writes to the
# store (bumping `updated_at` only, content untouched) and restores the original value in a
# finally: block; it is SKIPPED, not failed, when the DB is not reachable.
#
# NOTE the catalog is cached (CATALOG_TTL_SECONDS, default 60s), so this sets it to 0. In
# production the re-read lands on the first turn AFTER the cache expires — up to 60s later, which
# is a delay, not a miss.
#
# Prereqs: MT instance on :3020, the Account Service, and a registry client with a book
# (default `eval` — never point this at a real client; leg E writes to its store). Run:
#   IRONCLAW_API=http://127.0.0.1:3020 python3 test_freshness_lifecycle.py
import os, sys, json, pathlib, tempfile, subprocess

os.environ.setdefault("IRONCLAW_API", "http://127.0.0.1:3020")
os.environ.setdefault("TELEGRAM_BOT_TOKEN", "unused-by-this-probe")   # bridge reads it at import
os.environ["CATALOG_TTL_SECONDS"] = "0"                               # must precede the import
ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi/seam"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

_state_fd, _state_path = tempfile.mkstemp(prefix="bridge-threads-probe-", suffix=".json")
os.close(_state_fd)
os.environ["BRIDGE_STATE"] = _state_path

import context_ingress as ing          # noqa: E402
import telegram_bridge as tb           # noqa: E402

STATE = pathlib.Path(_state_path)
# Fail closed rather than risk the operator's real state file: if BRIDGE_STATE did not take
# effect (import order changed, env stripped), STOP — do not fall back to the default path.
if tb.STATE_PATH != STATE:
    sys.exit(f"refusing to run: STATE_PATH is {tb.STATE_PATH}, not the probe's temp file")

SLUG = os.environ.get("VERIFY_CLIENT", "eval")
DB_CONTAINER = os.environ.get("ACCOUNT_DB_CONTAINER", "multiagency-data-account-db-1")
DB_NAME = os.environ.get("ACCOUNT_DB_NAME", "accounts")

from common import Checks   # the tick-list; this file keeps its own verdict line

checks = Checks()
check = checks.check
skip = checks.skip


def sql(query):
    """One-shot psql against the account store. Returns stripped stdout, or None if unreachable."""
    p = subprocess.run(["docker", "exec", DB_CONTAINER, "psql", "-U", "postgres",
                        "-d", DB_NAME, "-tAc", query], capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else None


try:
    clients = ing.load_clients()
    if SLUG not in clients:
        sys.exit(f"client {SLUG!r} not in the registry — provision it first (see multi/eval/README.md)")
    client = clients[SLUG]
    if not client.telegram_group_id:
        sys.exit(f"client {SLUG!r} has no TELEGRAM_GROUP_ID — the bridge loader keys on it")
    gid = str(client.telegram_group_id)

    catalog = ing._catalog(client)
    book = catalog.get("accounts") or []
    if not book:
        sys.exit(f"client {SLUG!r} has an empty book — seed it first (see multi/eval/README.md)")
    acct, org = book[0], catalog["org"]
    aid, aname = acct["account_id"], acct["name"]
    print(f"client={SLUG} org={org} group={gid} account={aid} ({aname})\n")

    groups = {gid: client}
    fetches = []
    _real_get = ing._get_context
    ing._get_context = lambda a, c=None: (fetches.append(a) or _real_get(a, c))

    def turn(msg):
        fetches.clear()
        ing.turn(thread, msg)
        return list(fetches)

    print("A. a pre-versioning state file is REFUSED, not coerced")
    STATE.write_text(json.dumps({gid: {"prev": None, "supplied": [aid], "ever_supplied": True}}))
    try:
        tb._load_threads(groups)
        check("loader refuses a pre-versioning 'supplied' list", False, "it was accepted")
    except ValueError as e:
        check("loader refuses a pre-versioning 'supplied' list", "Migrate once" in str(e),
              "refusal must tell the operator how to fix it")

    print("\nB. the documented migration (telegram_bridge.py) leaves version-unknown entries")
    d = json.loads(STATE.read_text())
    for st in d.values():
        if isinstance(st.get("supplied"), list):
            st["ever_supplied"] = st.get("ever_supplied", bool(st["supplied"]))
            st["supplied"] = {a: None for a in st["supplied"]}
    STATE.write_text(json.dumps(d))
    thread = tb._load_threads(groups)[gid]
    check("migrated state loads", thread.supplied == {aid: None}, str(thread.supplied))
    check("ever_supplied survives the migration", thread.ever_supplied is True)

    print("\nC. an unknown version re-reads ONCE, then settles")
    got = turn(f"update on {aname}")
    check("first turn after migration re-fetches", got == [aid], f"fetched {got}")
    healed = thread.supplied.get(aid)
    check("the real version is recorded", healed is not None, str(thread.supplied))
    got = turn(f"anything new on {aname}?")
    check("healing does not repeat (no per-turn storm)", got == [], f"fetched {got}")

    print("\nD. a GENUINE edit re-reads, unprompted")
    original = sql(f"SELECT updated_at FROM accounts WHERE org_id='{org}' AND account_id='{aid}';")
    if original is None:
        skip("a moved record is re-fetched", f"account store not reachable via docker exec {DB_CONTAINER}")
        skip("the new version is recorded", "same")
        skip("it settles again after the edit", "same")
    else:
        try:
            sql(f"UPDATE accounts SET updated_at = now() WHERE org_id='{org}' AND account_id='{aid}';")
            got = turn(f"anything new on {aname}?")
            check("a moved record is re-fetched, with no prompting", got == [aid], f"fetched {got}")
            check("the new version is recorded", thread.supplied.get(aid) not in (None, healed),
                  str(thread.supplied))
            got = turn(f"and again on {aname}?")
            check("it settles again after the edit", got == [], f"fetched {got}")
        finally:
            # restore even if a leg raised: the book must be left exactly as it was found
            sql(f"UPDATE accounts SET updated_at = '{original}' "
                f"WHERE org_id='{org}' AND account_id='{aid}';")
            back = sql(f"SELECT updated_at FROM accounts WHERE org_id='{org}' AND account_id='{aid}';")
            print(f"  restored updated_at -> {back}"
                  + ("" if back == original else f"  !! EXPECTED {original} — RESTORE FAILED"))
finally:
    STATE.unlink(missing_ok=True)

ok = checks.ok if checks.ran else False
print(f"\nscore: {checks.passed}/{checks.ran}" + (f", {len(checks.blocked)} BLOCKED" if checks.blocked else "")
      + (" — an unknown version heals once and a real edit is never missed" if ok and checks.ran else ""))
if checks.blocked and not checks.results:
    print("ALL LEGS BLOCKED — no assertions ran; not a pass.")
    sys.exit(2)
sys.exit(0 if ok else 1)
