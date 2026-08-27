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

from service_guards import (duplicate_orgs, insecure_mode, like_contains, new_ref, safe_error,
                            validate_identity_map)
from migrations import status as migration_status

DSN = os.environ["ACCOUNT_DB_DSN"]                 # DB creds live here only
# {token: org_id} — one credential maps to exactly one org. Two sources, merged:
#   ACCOUNT_IDENTITIES        env JSON — the static (dev/demo) base, fixed at startup
#   ACCOUNT_IDENTITIES_FILE   path to a JSON file, HOT-RELOADED on mtime change — so
#                             provisioning a client org never restarts the service
# Fail closed: refuse to start with neither configured; unknown token -> 401 per request.
ENV_IDENTITIES = validate_identity_map(
    json.loads(os.environ.get("ACCOUNT_IDENTITIES") or "{}"), "ACCOUNT_IDENTITIES")
IDENTITIES_FILE = os.environ.get("ACCOUNT_IDENTITIES_FILE", "")
if not ENV_IDENTITIES and not IDENTITIES_FILE:
    raise RuntimeError(
        "no identity source — the Account Service refuses to start without one (fail closed). "
        "Set ACCOUNT_IDENTITIES ({\"<token>\": \"<org_id>\"} JSON) and/or ACCOUNT_IDENTITIES_FILE.")
_file_ident = {"mtime": None, "map": {}, "loaded": False, "error": None}

def _validated(doc, path):
    """Schema-check a freshly read identity map before it replaces the live one, and report
    duplicate orgs. The rules themselves live in service_guards so they are testable without
    flask/psycopg installed; this wrapper is the logging half."""
    doc = validate_identity_map(doc, path)
    dupes = duplicate_orgs(doc)
    if dupes:
        print(f"identities: {len(dupes)} org(s) have MORE THAN ONE live token "
              f"({', '.join(dupes)}) — a mid-rotation state or a failed re-provision left "
              "authority behind. Deregister the stale token.", flush=True)
    return doc


def _identities():
    """The current {token: org} map. File entries override env entries.

    A failed file reload invalidates the file-backed authority immediately. Keeping a stale map
    while merely logging the failure lets removed credentials continue authenticating and makes
    `/ready` claim the source is healthy. Clear it, report not-ready, and retry on every request
    until a valid current file is loaded.
    """
    if IDENTITIES_FILE:
        try:
            st = os.stat(IDENTITIES_FILE)
            mtime = st.st_mtime_ns
            if mtime != _file_ident["mtime"]:
                # The file holds every client's org credential. Group- or world-readable is a
                # finding, not a fatality: refusing to load would take every client down over a
                # mode bit, which trades a confidentiality risk for a certain outage.
                loose = insecure_mode(st.st_mode)
                if loose is not None:
                    print(f"identities: {IDENTITIES_FILE} is mode {loose:o} — it holds every "
                          "client's org token and must be 0600. chmod it.", flush=True)
                with open(IDENTITIES_FILE) as f:
                    _file_ident["map"] = _validated(json.load(f), IDENTITIES_FILE)
                _file_ident["mtime"] = mtime
                _file_ident["loaded"] = True
                _file_ident["error"] = None
        except FileNotFoundError:
            if _file_ident["mtime"] is not None:
                print("identities file missing, invalidating file-backed authority", flush=True)
            _file_ident.update(mtime=None, map={}, loaded=False,
                               error="identity_file_missing")
        except (ValueError, OSError) as e:
            print(f"identities file unreadable, invalidating file-backed authority: {e}",
                  flush=True)
            # `mtime=None` forces a retry even on filesystems whose timestamp did not advance
            # between the malformed write and its correction.
            _file_ident.update(mtime=None, map={}, loaded=False,
                               error="identity_file_invalid")
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
    """Liveness for the trusted seam and the host watchdog. The FAILURE path is the one that
    matters here: `str(e)` on a psycopg error carries the connection string — host, database,
    user, and on some failure modes the password — and /health is the one route reachable with
    no credential at all. So the caller gets a stable code it can act on, and the diagnostic
    goes to the process log beside a correlation id that ties the two together.

    The correlation id is the whole point of not simply dropping the detail: an operator
    reading `{"ok": false, "error": "backend_unavailable", "ref": "…"}` can find the exact
    exception in the log, and nobody else learns anything."""
    try:
        with _conn() as c: c.execute("SELECT 1")
        return jsonify({"ok": True})
    except Exception as e:
        ref = new_ref()
        # repr, not str: the exception TYPE is the fastest signal in a log, and neither form
        # leaves this process.
        print(f"health check failed ref={ref}: {type(e).__name__}: {e!r}", flush=True)
        body, status = safe_error("backend_unavailable", ref)
        return jsonify(body), status


@app.get("/ready")
def ready():
    """Readiness: the service can authenticate callers and serve the committed DB contract."""
    try:
        _identities()
        # If a file source is configured, its CURRENT load must be good. A static env identity
        # cannot turn a failed file reload green: doing so would conceal stale/revoked file-backed
        # authority during mixed-source development or credential rotation.
        identity_ready = (not IDENTITIES_FILE or
                          (_file_ident["loaded"] and _file_ident["error"] is None))
        with _conn() as c:
            schema = migration_status(c)
        problems = list(schema["problems"])
        if not identity_ready:
            problems.append(_file_ident["error"] or "identity_source_not_loaded")
        body = {"ok": not problems, "schema_ready": schema["ready"],
                "identity_ready": identity_ready, "problems": problems,
                "expected_migrations": [m["version"] for m in schema["expected"]],
                "applied_migrations": sorted(schema["applied"])}
        return jsonify(body), 200 if body["ok"] else 503
    except Exception as e:
        ref = new_ref()
        print(f"readiness check failed ref={ref}: {type(e).__name__}: {e!r}", flush=True)
        body, _ = safe_error("not_ready", ref)
        return jsonify(body), 503

@app.get("/find_account")
def find_account():
    org, err = _auth_org()
    if err: return err
    q = (request.args.get("query") or "").strip()
    if not q: return jsonify({"error": "missing_query"}), 400
    with _conn() as c:
        # `like_contains` escapes the caller's `%` and `_`, and ESCAPE names the same character
        # the escaping used rather than relying on the server default. Without it `?query=%`
        # matched every row this org has, up to MAX_MATCHES — a lookup answering as a listing.
        rows = c.execute(
            "SELECT account_id, name, domain FROM accounts "
            "WHERE org_id = %s AND lower(name) LIKE %s ESCAPE '\\' ORDER BY name LIMIT %s",
            (org, like_contains(q.lower()), MAX_MATCHES),
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
                            "org": org, "account": None, "found": False,
                            "account_id": account_id}), 404
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
        "org": org,
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
