#!/usr/bin/env python3
"""MultiAgency internal Account Service — the ONLY thing that touches Postgres.

Exposes a tiny, read-only, org-scoped BUSINESS contract (find_account / get_account_context)
mapped from DB rows, so the DB schema can evolve independently of the agent-facing contract.
The agent never sees SQL, the schema, or DB credentials — those live here.

Identity implies org: each service credential (token) maps to exactly ONE trusted org,
resolved server-side. The caller presents only its token — it CANNOT assert an org (any
X-Org-Id header is ignored). The model never sees the token or an org selector.
"""
import os, json, datetime
from flask import Flask, request, jsonify
from psycopg.rows import dict_row
from psycopg_pool import ConnectionPool

DSN = os.environ["ACCOUNT_DB_DSN"]                 # DB creds live here only
# {token: org_id} — one credential maps to exactly one org. Two sources, merged:
#   ACCOUNT_IDENTITIES        env JSON — the static (dev/demo) base, fixed at startup
#   ACCOUNT_IDENTITIES_FILE   path to a JSON file, HOT-RELOADED on mtime change — so
#                             provisioning a client org never restarts the service
# Fail closed: refuse to start with neither configured; unknown token -> 401 per request.
ENV_IDENTITIES = json.loads(os.environ.get("ACCOUNT_IDENTITIES") or "{}")
IDENTITIES_FILE = os.environ.get("ACCOUNT_IDENTITIES_FILE", "")
if not ENV_IDENTITIES and not IDENTITIES_FILE:
    raise RuntimeError(
        "no identity source — the Account Service refuses to start without one (fail closed). "
        "Set ACCOUNT_IDENTITIES ({\"<token>\": \"<org_id>\"} JSON) and/or ACCOUNT_IDENTITIES_FILE.")
_file_ident = {"mtime": None, "map": {}}

def _identities():
    """The live {token: org} map. File entries override env entries. A malformed or missing
    file never takes identities down: keep the last good file map and retry next request."""
    if IDENTITIES_FILE:
        try:
            mtime = os.stat(IDENTITIES_FILE).st_mtime_ns
            if mtime != _file_ident["mtime"]:
                with open(IDENTITIES_FILE) as f:
                    _file_ident["map"] = json.load(f)
                _file_ident["mtime"] = mtime
        except FileNotFoundError:
            # keep the last good map — a briefly-absent file (backup restore, editor
            # move+rename, mount race) must never 401 every live client at once
            if _file_ident["mtime"] is not None:
                print("identities file missing, keeping previous map", flush=True)
                _file_ident["mtime"] = None       # re-log once; reload when it reappears
        except (ValueError, OSError) as e:                  # keep last good map
            print(f"identities file unreadable, keeping previous: {e}", flush=True)
    return {**ENV_IDENTITIES, **_file_ident["map"]}

MAX_MATCHES = 10
app = Flask(__name__)

# One pooled set of connections instead of a TCP+auth handshake per request — a single chat
# turn makes 1 + N calls here (list_accounts + one get_account_context per resolved account).
_pool = ConnectionPool(DSN, min_size=1, max_size=8, kwargs={"row_factory": dict_row})

def _now(): return datetime.datetime.now(datetime.timezone.utc).isoformat()
def _conn(): return _pool.connection()

def _auth_org():
    # Identity implies org: resolve the org from the credential, server-side. The caller
    # cannot assert an org (any X-Org-Id header is ignored). Unknown token -> 401.
    org = _identities().get(request.headers.get("X-Service-Token", ""))
    if not org:
        return None, (jsonify({"error": "unauthorized"}), 401)
    return org, None

# Legacy gap list for the sales-shaped columns. Kept ONLY for books that actually use those
# columns; it is a fallback, not the contract. Which fields are genuinely expected differs per
# partner, so the seam computes the real gap list from that client's declared FACT_FIELDS
# (see multi/seam/context_ingress.py). Reported as `missing_legacy` so a caller can tell the
# two apart and neither silently stands in for the other.
BUSINESS_FIELDS = ["budget", "timeline", "decision_process", "economic_buyer", "stated_problem"]

def _missing(acct: dict):
    return [f for f in BUSINESS_FIELDS if acct.get(f) in (None, "")]

@app.get("/health")
def health():
    try:
        with _conn() as c: c.execute("SELECT 1")
        return jsonify({"ok": True})
    except Exception as e:
        return jsonify({"ok": False, "error": str(e)}), 500

@app.get("/find_account")
def find_account():
    org, err = _auth_org()
    if err: return err
    q = (request.args.get("query") or "").strip()
    if not q: return jsonify({"error": "missing_query"}), 400
    with _conn() as c:
        rows = c.execute(
            "SELECT account_id, name, domain FROM accounts "
            "WHERE org_id = %s AND lower(name) LIKE %s ORDER BY name LIMIT %s",
            (org, f"%{q.lower()}%", MAX_MATCHES),
        ).fetchall()
    return jsonify({"source": "multiagency", "retrieved_at": _now(),
                    "org": org, "query": q, "matches": rows, "match_count": len(rows)})

@app.get("/list_accounts")
def list_accounts():
    # Bounded, read-only candidate set for the org (for prioritization prefetch).
    org, err = _auth_org()
    if err: return err
    with _conn() as c:
        rows = c.execute(
            # updated_at rides the catalog so the seam can tell FRESH from STALE without asking
            # the user to say a magic word: it is the cheap call (cached per client), and the
            # expensive per-account fetch is then made only for accounts that actually moved.
            "SELECT account_id, name, domain, updated_at FROM accounts "
            "WHERE org_id = %s ORDER BY name LIMIT %s",
            (org, 50)).fetchall()
    for r in rows:
        if r.get("updated_at") is not None:
            r["updated_at"] = r["updated_at"].isoformat()
    return jsonify({"source": "multiagency", "retrieved_at": _now(),
                    "org": org, "accounts": rows, "count": len(rows)})

@app.get("/get_account_context")
def get_account_context():
    org, err = _auth_org()
    if err: return err
    account_id = (request.args.get("account_id") or "").strip()
    if not account_id: return jsonify({"error": "missing_account_id"}), 400
    with _conn() as c:
        acct = c.execute(
            "SELECT account_id, name, domain, industry, employees, headquarters, cloud, "
            "stated_problem, current_tooling, budget, timeline, decision_process, economic_buyer, "
            "owner, stage, value_band, facts, "
            "updated_at FROM accounts WHERE org_id = %s AND account_id = %s",
            (org, account_id),
        ).fetchone()
        if not acct:
            # explicit not-found — and never leaks another org's row
            return jsonify({"source": "multiagency", "retrieved_at": _now(),
                            "account": None, "found": False, "account_id": account_id}), 404
        contacts = c.execute(
            "SELECT contact_id, name, title, engaged, notes FROM contacts "
            "WHERE org_id = %s AND account_id = %s ORDER BY name", (org, account_id)).fetchall()
        activities = c.execute(
            "SELECT activity_id, occurred_at, kind, body FROM activities "
            "WHERE org_id = %s AND account_id = %s ORDER BY occurred_at", (org, account_id)).fetchall()
    for a in activities:
        if a.get("occurred_at"): a["occurred_at"] = a["occurred_at"].isoformat()
    if acct.get("updated_at"): acct["updated_at"] = acct["updated_at"].isoformat()
    return jsonify({
        "source": "multiagency",
        "record_id": acct["account_id"],
        "retrieved_at": _now(),
        "found": True,
        "account": acct,               # explicit nulls preserved for unknown business facts
        "contacts": contacts,
        "activities": activities,
        "open_opportunities": [],      # not modeled in V1 (no eval needs it)
        # Gaps in the SALES-shaped columns only. The per-partner gap list (declared
        # FACT_FIELDS vs what `facts` actually holds) is computed in the seam, which is the
        # only layer that knows which client this is.
        "missing_legacy": _missing(acct),
    })

if __name__ == "__main__":
    # Plain HTTP on the private network only (reached by the trusted backend over localhost).
    # TLS scaffolding was removed with the deferred WASM egress path it belonged to.
    app.run(host="0.0.0.0", port=int(os.environ.get("PORT", "8080")))
