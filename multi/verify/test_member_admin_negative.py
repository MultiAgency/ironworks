#!/usr/bin/env python3
# MEMBER-vs-ADMIN NEGATIVE PROBE — the tenant-wide admin surface needs the operator bearer,
# and the project surface — which IS member-reachable — is per-user isolated.
#
# The tenant-wide surfaces (admin user directory, operator config, project records + membership)
# must not be reachable/writable by a sealed member. This generalizes the lesson that an
# operator-scoped control does not bind a member path, applied to the admin API.
#
# Routes enumerated read-only from ironclaw's webui route table
# (crates/product/ironclaw_webui/src/webui_v2/descriptors.rs). Two shapes of check:
#
#   1. ADMIN / OPERATOR routes — a sealed member MUST be denied (401/403). Bodies are
#      well-formed on purpose: admin authz is enforced in the SERVICE layer, AFTER body
#      deserialization (handlers.rs: "the service enforces admin authorization (operator token
#      or admin/owner role)"), so a malformed body returns 422 BEFORE authz runs and would mask
#      the real check. Verified live: a well-formed member POST /admin/users returns
#      403 participant_denied; role/status likewise.
#
#   2. PROJECT routes — CORRECTION to the earlier INFERRED claim that "no product route exposes
#      projects to member tokens." It is FALSE: GET/POST /projects return 200 for a sealed member. The route
#      is member-reachable but CALLER-SCOPED (handlers.rs list_projects/create_project bind to
#      the caller's own (tenant,user); CONTRACT.md keys projects per tenant with a per-user ACL).
#      The real guarantee is per-user isolation, so this probe PROVES that directly: client A
#      creates a project; client B's list must not show it and B GET/DELETE by exact id must 404;
#      then A deletes it (cleanup runs even on failure). Verified live: B 404s on A's
#      id; A delete -> 204. So the guarantee holds — via ACL scoping on a reachable route, not via route
#      inaccessibility. (Note: projects are therefore a member-WRITABLE surface the seam never
#      uses; flagged for the record, not a leak.)
#
# Prereqs: MT instance on :3020; one provisioned client for the admin leg, TWO (e.g. proof-a /
# proof-b) for the project-isolation leg. Optional IRONCLAW_OPERATOR_TOKEN adds a positive control.
# Run:  IRONCLAW_API=http://127.0.0.1:3020 python3 test_member_admin_negative.py
import os, sys, json, urllib.request, urllib.error, pathlib

os.environ.setdefault("IRONCLAW_API", "http://127.0.0.1:3020")
ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi/seam"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from common import DEFAULT_API, Checks  # noqa: E402

# (method, path, body) with WELL-FORMED bodies so service-layer authz actually runs. Member MUST
# be denied on every one (401/403). Owner id `reborn-cli` is the instance default_owner.
ADMIN_ROUTES = [
    ("GET",  "/api/webchat/v2/admin/users", None),
    ("POST", "/api/webchat/v2/admin/users",
     {"email": "probe@example.test", "display_name": "probe", "role": "member"}),
    ("GET",  "/api/webchat/v2/admin/users/reborn-cli", None),
    ("POST", "/api/webchat/v2/admin/users/reborn-cli/role", {"role": "member"}),
    ("POST", "/api/webchat/v2/admin/users/reborn-cli/status", {"status": "active"}),
    ("GET",  "/api/webchat/v2/admin/users/reborn-cli/secrets", None),
    ("GET",  "/api/webchat/v2/operator/config", None),
    ("GET",  "/api/webchat/v2/operator/status", None),
]
# GET /settings/tools is the member-readable catalog (see test_catalog_parity.py) — 200 is fine.
READ_ROUTES = [("GET", "/api/webchat/v2/settings/tools")]

checks = Checks()
check = checks.check
block = checks.block

def req(method, path, token, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(DEFAULT_API + path, data=data, method=method,
        headers={"Authorization": "Bearer " + token, "User-Agent": "Mozilla/5.0",
                 "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()

def clients_by_slug():
    try:
        import context_ingress as ing
        return dict(sorted(ing.load_clients().items()))
    except Exception as e:
        print(f"     (client registry unavailable: {e})"); return {}

cl = clients_by_slug()
picks = list(cl.values())
member = picks[0] if picks else None

# ---- 1. admin / operator routes deny the member ----
print("== member token vs admin / operator routes (deny 401/403) ==")
if member is None:
    block("admin-deny suite", "no provisioned client / instance unreachable")
else:
    for method, path, body in ADMIN_ROUTES:
        try:
            st, _ = req(method, path, member.ironclaw_token, body)
        except Exception as e:
            block(f"{method} {path}", f"call failed: {e}"); continue
        check(f"{method} {path}: member denied (401/403)", st in (401, 403), f"status {st}")
    for method, path in READ_ROUTES:
        try:
            st, _ = req(method, path, member.ironclaw_token)
        except Exception as e:
            block(f"{method} {path}", f"call failed: {e}"); continue
        check(f"{method} {path}: member read allowed (200/403 both fine)",
              st in (200, 403), f"status {st}")

# ---- 2. projects are member-reachable but per-user isolated (the correction above) ----
print("== projects: member-reachable, per-user isolated ==")
if len(picks) < 2:
    block("project cross-user isolation", "need two provisioned clients (e.g. proof-a/proof-b)")
else:
    A, B = picks[0], picks[1]
    pid = None
    try:
        st, raw = req("POST", "/api/webchat/v2/projects", A.ironclaw_token,
                      {"name": "isolation-probe", "description": "cross-user isolation probe; auto-deleted"})
        check("A (member) can create a project — route IS member-reachable", st == 200, f"status {st}")
        if st == 200:
            pid = (json.loads(raw).get("project") or {}).get("project_id")
        if not pid:
            block("project cross-user isolation", f"no project_id in create response: {raw[:200]!r}")
        else:
            # B must not see A's project in its own list...
            st_b, raw_b = req("GET", "/api/webchat/v2/projects", B.ironclaw_token)
            b_list = json.loads(raw_b).get("projects", []) if st_b == 200 else []
            check("B's project list does not include A's project",
                  all(p.get("project_id") != pid for p in b_list), f"B list: {raw_b[:200]!r}")
            # ...and must 404 (not 200/403-with-content) on A's exact id, for read and delete.
            st_bg, _ = req("GET", f"/api/webchat/v2/projects/{pid}", B.ironclaw_token)
            check("B GET A's project by exact id -> 404 (no cross-user read)", st_bg == 404, f"status {st_bg}")
            st_bd, _ = req("DELETE", f"/api/webchat/v2/projects/{pid}", B.ironclaw_token)
            check("B DELETE A's project by exact id -> 404 (no cross-user delete)", st_bd == 404, f"status {st_bd}")
            # A can see + delete its own (positive control).
            st_ag, _ = req("GET", f"/api/webchat/v2/projects/{pid}", A.ironclaw_token)
            check("A GET own project -> 200 (per-user scope is real, not blanket-deny)", st_ag == 200, f"status {st_ag}")
    except Exception as e:
        block("project cross-user isolation", f"call failed: {e}")
    finally:
        # Cleanup ALWAYS — never leave a probe project on the instance.
        if pid:
            try:
                st_del, _ = req("DELETE", f"/api/webchat/v2/projects/{pid}", A.ironclaw_token)
                print(f"     cleanup: A deleted probe project {pid} (status {st_del})")
            except Exception as e:
                print(f"     !! cleanup FAILED for project {pid}: {e} — delete it manually")

# ---- optional positive control ----
op = os.environ.get("IRONCLAW_OPERATOR_TOKEN")
if op:
    print("== positive control: operator authorized where member was denied ==")
    try:
        st, _ = req("GET", "/api/webchat/v2/admin/users", op)
        check("operator GET /admin/users is not 401/403", st not in (401, 403), f"status {st}")
    except Exception as e:
        block("operator positive control", f"call failed: {e}")
else:
    print("     (set IRONCLAW_OPERATOR_TOKEN to add the operator positive control)")

ok = checks.ok if checks.ran else False
print(f"\nscore: {checks.passed}/{checks.ran} run" + (f", {len(checks.blocked)} BLOCKED" if checks.blocked else "")
      + (" — admin surface operator-only; projects member-reachable but per-user isolated"
         if ok and checks.ran else ""))
if checks.blocked and not checks.results:
    print("ALL LEGS BLOCKED — no assertions ran; not a pass. Operator: run on the VM.")
    sys.exit(2)
sys.exit(0 if ok else 1)
