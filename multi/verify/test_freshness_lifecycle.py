#!/usr/bin/env python3
"""Freshness lifecycle on a current, authenticated, compatibility-bound bridge thread.

Legacy migration safety belongs to multi/seam/test_thread_compatibility.py. This proof starts
after that boundary: production scope resolution establishes the organization, production
thread persistence writes the complete compatibility identity, and `_load_threads` verifies it.

Run offline (CI/release):
  python3 multi/verify/test_freshness_lifecycle.py --offline

Run the additional target-host legs:
  IRONCLAW_API=http://127.0.0.1:3020 python3 multi/verify/test_freshness_lifecycle.py
"""
import argparse
import os
import pathlib
import subprocess
import sys
import tempfile

os.environ["CATALOG_TTL_SECONDS"] = "0"       # must precede context_ingress import
ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi/seam"))

_tmp = tempfile.TemporaryDirectory(prefix="ironworks-freshness-")
STATE_JSON = pathlib.Path(_tmp.name) / "bridge-threads.json"
STATE_DB = STATE_JSON.with_suffix(".db")
os.environ["BRIDGE_STATE"] = str(STATE_JSON)
os.environ.pop("BRIDGE_STATE_DB", None)

import context_ingress as ing          # noqa: E402
import account_service as asvc          # noqa: E402
import telegram_bridge as tb           # noqa: E402
from common import Checks              # noqa: E402

parser = argparse.ArgumentParser()
parser.add_argument("--offline", action="store_true",
                    help="run deterministic freshness legs; report target-host legs BLOCKED")
OFFLINE = parser.parse_args().offline

checks = Checks()
check, block = checks.check, checks.block
state = None


def fixture_client():
    """Registry-shaped input: explicitly unverified until production scope resolution runs."""
    return ing.ClientConfig(
        slug="freshness-fixture", ironclaw_token="fixture-member",
        account_token="fixture-account", telegram_group_id="-100900099",
        account_base="http://fixture.invalid", organization_verified=False,
        service="account-analysis", service_version=1,
        persona="CURRENT FIXTURE INSTRUCTIONS\n\n## Safety\nRead only.")


def block_live(reason):
    for label in (
            "live unknown version re-fetches", "live healed version is recorded",
            "live healing settles", "live moved record re-fetches",
            "live new version is recorded", "live edit settles"):
        block(label, reason)


def run_offline_freshness(st):
    gid, aid, name, org = "-100900099", "FX-101", "Fixture Account", "fixture-org"
    version = {"value": "v1"}
    fetches, model_calls = [], []
    saved = asvc._svc, ing._get_context, ing._post_ironclaw

    def service(path, client=None):
        if path == "/list_accounts":
            row = {"account_id": aid, "name": name}
            if version["value"] is not None:
                row["updated_at"] = version["value"]
            return {"org": org, "accounts": [row]}
        raise AssertionError(f"unexpected Account Service path: {path}")

    def context(account_id, client=None):
        fetches.append(account_id)
        return {"record_id": account_id,
                "account": {"name": name, "updated_at": version["value"]},
                "contacts": [], "activities": [], "missing": []}

    def model(body, client=None, attempts=4):
        model_calls.append(body)
        return {"id": f"resp_{len(model_calls)}", "status": "completed", "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}

    try:
        asvc._svc, ing._get_context, ing._post_ironclaw = service, context, model
        resolved = ing.resolve_account_scopes({gid: fixture_client()})
        client = resolved[gid]
        check("authenticated organization scope is established",
              client.organization_verified and client.organization_id == org)

        # `_save_threads` is the production identity constructor. No identity column is authored
        # by this proof, and `_load_threads` remains the production compatibility gate.
        tb._save_threads({gid: ing.Thread(client)}, state=st)
        thread = tb._load_threads(resolved, state=st)[gid]
        check("freshness fixture is compatibility-bound by production persistence",
              st.stored_identity(st.thread_row(gid)) == client.thread_identity)

        def turn(text):
            fetches.clear()
            ing.turn(thread, text)
            return list(fetches)

        got = turn(f"Tell me about {name}")
        check("initial context is supplied", got == [aid] and thread.supplied == {aid: "v1"},
              f"fetched={got} supplied={thread.supplied}")

        got = turn(f"Anything else on {name}?")
        check("unchanged context is not redundantly supplied", got == [], f"fetched={got}")

        version["value"] = "v2"
        got = turn(f"Anything else on {name}?")
        check("changed context is re-fetched and re-supplied",
              got == [aid] and thread.supplied == {aid: "v2"},
              f"fetched={got} supplied={thread.supplied}")
        got = turn(f"And again on {name}?")
        check("changed context settles after re-supply", got == [], f"fetched={got}")

        # None is a legitimate current freshness value: a store may not have emitted updated_at
        # on an earlier turn. Persist and reload it through production code, then let v3 appear.
        thread.supplied = {aid: None}
        thread.ever_supplied = True
        tb._save_threads({gid: thread}, state=st)
        thread = tb._load_threads(resolved, state=st)[gid]
        check("unknown freshness state survives a compatible restart",
              thread.supplied == {aid: None} and thread.ever_supplied)
        version["value"] = "v3"
        got = turn(f"Anything else on {name}?")
        check("unknown freshness heals when a newer version appears",
              got == [aid] and thread.supplied == {aid: "v3"},
              f"fetched={got} supplied={thread.supplied}")
        got = turn(f"One more time on {name}?")
        check("healed freshness does not create a fetch storm", got == [], f"fetched={got}")
    finally:
        asvc._svc, ing._get_context, ing._post_ironclaw = saved


def run_live_freshness(st):
    """The target-host continuation: real tenant, model runtime, Account Service and database."""
    slug = os.environ.get("VERIFY_CLIENT", "eval")
    try:
        clients = ing.load_clients()
        if slug not in clients:
            raise RuntimeError(f"client {slug!r} is not provisioned")
        unresolved = clients[slug]
        gid = str(unresolved.telegram_group_id or "")
        if not gid:
            raise RuntimeError(f"client {slug!r} has no Telegram group")
        resolved = ing.resolve_account_scopes({gid: unresolved})
        client = resolved[gid]
        catalog = ing._catalog(client)
        account = (catalog.get("accounts") or [])[0]
    except Exception as e:
        block_live(f"target-host prerequisites unavailable ({type(e).__name__})")
        return

    aid, name, org = account["account_id"], account["name"], catalog["org"]
    st.reset_thread(gid)
    thread = ing.Thread(client)
    thread.supplied, thread.ever_supplied = {aid: None}, True
    tb._save_threads({gid: thread}, state=st)
    thread = tb._load_threads(resolved, state=st)[gid]
    fetches, saved_get = [], ing._get_context
    ing._get_context = lambda account_id, c=None: (fetches.append(account_id)
                                                    or saved_get(account_id, c))

    def turn(text):
        fetches.clear()
        ing.turn(thread, text)
        return list(fetches)

    try:
        try:
            got = turn(f"update on {name}")
            healed = thread.supplied.get(aid)
            check("live unknown version re-fetches", got == [aid], f"fetched={got}")
            check("live healed version is recorded", healed is not None, str(thread.supplied))
            check("live healing settles", turn(f"again on {name}?") == [])
        except Exception as e:
            block_live(f"model/Account Service leg unavailable ({type(e).__name__})")
            return

        container = os.environ.get("ACCOUNT_DB_CONTAINER", "multiagency-data-account-db-1")
        db_name = os.environ.get("ACCOUNT_DB_NAME", "accounts")

        def sql(query):
            p = subprocess.run(["docker", "exec", container, "psql", "-U", "postgres",
                                "-d", db_name, "-tAc", query], capture_output=True, text=True)
            return p.stdout.strip() if p.returncode == 0 else None

        original = sql(f"SELECT updated_at FROM accounts WHERE org_id='{org}' AND account_id='{aid}';")
        if original is None:
            for label in ("live moved record re-fetches", "live new version is recorded",
                          "live edit settles"):
                block(label, f"Account database unavailable via {container}")
            return
        try:
            sql(f"UPDATE accounts SET updated_at=now() WHERE org_id='{org}' AND account_id='{aid}';")
            got = turn(f"anything new on {name}?")
            check("live moved record re-fetches", got == [aid], f"fetched={got}")
            check("live new version is recorded", thread.supplied.get(aid) not in (None, healed))
            check("live edit settles", turn(f"again on {name}?") == [])
        finally:
            sql(f"UPDATE accounts SET updated_at='{original}' "
                f"WHERE org_id='{org}' AND account_id='{aid}';")
    finally:
        ing._get_context = saved_get


try:
    if tb.state_json_path() != STATE_JSON or tb.state_db_path() != STATE_DB:
        raise SystemExit("refusing to run: bridge state did not resolve to the proof directory")
    state = tb.open_state()
    run_offline_freshness(state)
    if OFFLINE:
        block_live("--offline: needs a provisioned target host")
    else:
        run_live_freshness(state)
finally:
    if state is not None:
        state.close()
    _tmp.cleanup()

checks.finish("identity-bound freshness advances exactly when account versions advance")
