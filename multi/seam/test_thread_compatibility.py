#!/usr/bin/env python3
"""Persisted conversations are bound to the composition that created them. Offline only."""
import dataclasses
import hashlib
import io
import json
import pathlib
import sqlite3
import stat
import tempfile
import urllib.error

try:
    from . import account_service as asvc
    from . import bridge_state as bs
    from . import context_ingress as ing
    from . import persona as per
    from . import telegram_bridge as tb
except ImportError:
    import account_service as asvc
    import bridge_state as bs
    import context_ingress as ing
    import persona as per
    import telegram_bridge as tb

ROOT = pathlib.Path(__file__).resolve().parents[2]
GID = "-100900071"
GUIDANCE = """<!-- client-guidance v1 slug: acme -->
# Organization guidance — Acme Synthetic
## Company and offer
Acme keeps an invented account book solely for offline lifecycle tests in this repository.
## Target organization
No real organization. Every name and activity is synthetic and carries no production meaning.
## Qualification
Use only supplied records, distinguish evidence from inference, and identify unknowns explicitly.
## Disqualification
Never recommend outreach when the record contains a do-not-contact instruction.
## Account stages
new -> reviewed -> decided. Explain which recorded evidence supports the current stage.
## Evidence
The Account Service records and facts stated by the team in the current conversation.
## Desired decisions
Identify which invented account needs review and which question would resolve the largest unknown.
## Prohibited actions
Never contact anyone, write data, or describe an invented fixture as a real organization.
"""


def client(**changes):
    base = ing.ClientConfig(
        slug="acme", ironclaw_token="member-fixture", account_token="account-fixture",
        telegram_group_id=GID, service="account-analysis", service_version=1,
        organization_id="org-acme", organization_verified=True,
        persona="FINAL COMPOSED INSTRUCTIONS\n\n## Safety\nRead only.")
    return dataclasses.replace(base, **changes)


def active_thread(c):
    th = ing.Thread(c)
    th.prev = "resp_existing"
    th.supplied = {"A-1": "2026-08-25T00:00:00+00:00"}
    th.ever_supplied = True
    th.last_turn_at = "2026-08-25T00:01:00+00:00"
    th.orphans = {"GHOST-1": ("v1", 1)}
    return th


def persist(st, c):
    tb._save_threads({GID: active_thread(c)}, state=st)


def assert_refuses(st, current, category):
    before = dict(st.thread_row(GID))
    try:
        tb._load_threads({GID: current}, state=st)
    except bs.ThreadCompatibilityError as e:
        assert category in e.categories, (category, e.categories)
        assert f"tenant reset-thread {current.slug} --confirm {current.slug}" in str(e), e
    else:
        raise AssertionError(f"{category} mismatch continued the persisted conversation")
    after = dict(st.thread_row(GID))
    assert after == before, "mismatch refusal mutated the conversation instead of preserving it"


def _tmp_state():
    d = tempfile.TemporaryDirectory()
    return d, bs.BridgeState(pathlib.Path(d.name) / "state.db")


def test_fresh_row_binds_the_full_compatibility_identity():
    d, st = _tmp_state()
    c = client()
    persist(st, c)
    row = st.thread_row(GID)
    assert st.stored_identity(row) == c.thread_identity
    assert len(row["instructions_sha256"]) == len(row["context_policy_sha256"]) == 64
    st.close(); d.cleanup()
    print("  PASS a fresh row binds the full compatibility identity")


def test_plain_restart_with_same_identity_continues_without_reset_or_fork():
    d, st = _tmp_state()
    c = client()
    persist(st, c)
    before = dict(st.thread_row(GID))
    first = tb._load_threads({GID: c}, state=st)[GID]
    second = tb._load_threads({GID: c}, state=st)[GID]
    assert first.prev == second.prev == "resp_existing"
    assert first.supplied == second.supplied == {"A-1": "2026-08-25T00:00:00+00:00"}
    assert dict(st.thread_row(GID)) == before, "ordinary restart reset or forked the row"
    st.close(); d.cleanup()
    print("  PASS an unchanged application restart continues the same conversation")


def test_service_mismatch_refuses():
    d, st = _tmp_state(); original = client(); persist(st, original)
    assert_refuses(st, dataclasses.replace(original, service="relationship-intelligence"), "service")
    st.close(); d.cleanup(); print("  PASS service mismatch refuses")


def test_service_version_mismatch_refuses():
    d, st = _tmp_state(); original = client(); persist(st, original)
    assert_refuses(st, dataclasses.replace(original, service_version=2), "service version")
    st.close(); d.cleanup(); print("  PASS service-version mismatch refuses")


def _composed(guidance_path):
    return per.compose_service_persona("account-analysis", guidance_path, "acme", ROOT)


def test_guidance_body_change_refuses_but_comment_only_change_continues():
    with tempfile.TemporaryDirectory() as tmp:
        g = pathlib.Path(tmp) / "acme.guidance.md"
        g.write_text(GUIDANCE)
        p1 = _composed(str(g))
        c1 = client(persona=p1)
        st = bs.BridgeState(pathlib.Path(tmp) / "state.db")
        persist(st, c1)

        g.write_text(GUIDANCE.replace("<!-- client-guidance v1 slug: acme -->",
                                     "<!-- client-guidance v1 slug: acme -->\n"
                                     "<!-- operator-only comment changed -->"))
        p_comment = _composed(str(g))
        assert p_comment == p1, "a stripped authoring comment changed model-visible instructions"
        assert tb._load_threads({GID: client(persona=p_comment)}, state=st)[GID].prev == "resp_existing"

        g.write_text(GUIDANCE + "\nThe team now requires a weekly evidence review.\n")
        assert_refuses(st, client(persona=_composed(str(g))), "instructions")
        st.close()
    print("  PASS guidance body changes refuse; stripped comment-only changes continue")


def test_persona_part_change_refuses():
    d, st = _tmp_state(); original = client(); persist(st, original)
    assert_refuses(st, dataclasses.replace(original, persona=original.persona + "\npersona part edit"),
                   "instructions")
    st.close(); d.cleanup(); print("  PASS persona-part content change refuses")


def test_safety_tail_change_refuses():
    d, st = _tmp_state(); original = client(); persist(st, original)
    assert_refuses(st, dataclasses.replace(original, persona=original.persona + "\nnew safety tail"),
                   "instructions")
    st.close(); d.cleanup(); print("  PASS safety-tail change refuses")


def test_model_mismatch_refuses():
    d, st = _tmp_state(); original = client(); persist(st, original)
    assert_refuses(st, dataclasses.replace(original, model="Explicit/TestModel"), "model")
    st.close(); d.cleanup(); print("  PASS model mismatch refuses")


def test_organization_scope_mismatch_refuses_but_token_rotation_does_not():
    d, st = _tmp_state(); original = client(account_token="old-token"); persist(st, original)
    rotated = dataclasses.replace(original, account_token="new-token")
    assert tb._load_threads({GID: rotated}, state=st)[GID].prev == "resp_existing"
    assert_refuses(st, dataclasses.replace(rotated, organization_id="org-other"),
                   "organization scope")
    st.close(); d.cleanup()
    print("  PASS org changes refuse; same-org credential rotation continues")


def test_account_service_endpoint_mismatch_refuses_same_apparent_org():
    d, st = _tmp_state(); original = client(account_base="https://accounts.example/a/")
    persist(st, original)
    equivalent = dataclasses.replace(original, account_base="https://ACCOUNTS.example:443/a")
    assert tb._load_threads({GID: equivalent}, state=st)[GID].prev == "resp_existing"
    assert_refuses(st, dataclasses.replace(original, account_base="https://other.example/a"),
                   "Account Service endpoint")
    st.close(); d.cleanup()
    print("  PASS endpoint changes refuse; equivalent normalized bases continue")


def test_startup_scope_resolution_uses_authenticated_org_and_fails_closed():
    original_svc = asvc._svc
    try:
        c = client(organization_id="untrusted-registry-metadata", organization_verified=False)
        asvc._svc = lambda path, current: {"org": "trusted-service-org", "accounts": []}
        resolved = ing.resolve_account_scopes({GID: c})[GID]
        assert resolved.organization_id == "trusted-service-org" and resolved.organization_verified
        assert resolved.account_token == c.account_token
        asvc._svc = lambda path, current: {"accounts": []}
        try:
            ing.resolve_account_scopes({GID: c})
        except ing.AccountScopeError:
            pass
        else:
            raise AssertionError("missing authoritative org was treated as compatible")
    finally:
        asvc._svc = original_svc
    print("  PASS startup trusts authenticated org, not registry metadata, and fails closed")


def test_two_tenants_resolving_to_one_org_is_refused_at_startup():
    """D-091's one-to-one invariant, at the only place it can be measured.

    `registry.load_clients` dedupes ACCOUNT_TOKEN, and its own comment says that catches only
    ONE token reused. The case it cannot see is two DIFFERENT tokens minted against one org —
    an operator running register-identity.sh a second time for the same org id — because the
    tokens differ and the mapping is server-side. The Account Service reports that as a warning
    and serves it anyway (`duplicate_orgs_are_reported_not_refused`). So nothing refused it, and
    two Telegram groups — two audiences — were served one org's book.

    A tenant resolving to its OWN org must still load, or this check would be satisfied by
    refusing everything."""
    original_svc = asvc._svc
    try:
        a = client(slug="acme", account_token="token-a")
        b = client(slug="beta", account_token="token-b", telegram_group_id="-100222")

        asvc._svc = lambda path, current: {"org": "one-org", "accounts": []}
        try:
            ing.resolve_account_scopes({GID: a, "-100222": b})
        except ing.AccountScopeError as e:
            assert "acme" in str(e) and "beta" in str(e), str(e)
            assert "one-org" in str(e), str(e)
        else:
            raise AssertionError(
                "two tenants with different tokens resolved to one org and both were served")

        # THE POSITIVE CONTROL. Distinct orgs must resolve normally.
        orgs = {"token-a": "org-acme", "token-b": "org-beta"}
        asvc._svc = lambda path, current: {"org": orgs[current.account_token], "accounts": []}
        resolved = ing.resolve_account_scopes({GID: a, "-100222": b})
        assert resolved[GID].organization_id == "org-acme"
        assert resolved["-100222"].organization_id == "org-beta"
        assert all(c.organization_verified for c in resolved.values())
    finally:
        asvc._svc = original_svc
    print("  PASS two tenants cannot resolve to one org; distinct orgs still load")


def test_hot_identity_repoint_refuses_instead_of_degrading_or_continuing():
    original_svc, original_post = asvc._svc, ing._post_ironclaw
    model_calls = []
    try:
        c = client()
        asvc._svc = lambda path, current: {"org": "org-other", "accounts": []}
        ing._post_ironclaw = lambda *args, **kwargs: model_calls.append((args, kwargs))
        try:
            ing.turn(ing.Thread(c), "continue the conversation")
        except ing.AccountScopeChanged as e:
            assert "reset-thread acme --confirm acme" in str(e)
        else:
            raise AssertionError("a hot token->org remap degraded into continued conversation")
        assert model_calls == [], "the model was called after the organization scope changed"
    finally:
        asvc._svc, ing._post_ironclaw = original_svc, original_post
    print("  PASS hot identity-map repoint refuses before any subsequent model turn")


def test_hot_repoint_between_catalog_and_context_404_refuses_before_model():
    """The 404 is authenticated output too: org A catalog -> org B 404 must not disappear."""
    original_svc, original_post = asvc._svc, ing._post_ironclaw
    model_calls = []
    calls = []

    def remapping_svc(path, current):
        calls.append(path)
        if path == "/list_accounts":
            return {"org": "org-acme", "accounts": [
                {"account_id": "A-1", "name": "Acme One", "updated_at": "v1"}]}
        body = json.dumps({"org": "org-other", "account": None,
                           "found": False, "account_id": "A-1"}).encode()
        raise urllib.error.HTTPError(
            current.account_base + path, 404, "not found", {}, io.BytesIO(body))

    try:
        asvc._svc = remapping_svc
        ing._post_ironclaw = lambda *args, **kwargs: model_calls.append((args, kwargs))
        try:
            ing.turn(ing.Thread(client()), "review the book")
        except ing.AccountScopeChanged as e:
            assert "org-acme" in str(e) and "org-other" in str(e), e
        else:
            raise AssertionError("an authenticated cross-org 404 was treated as missing data")
        assert calls == ["/list_accounts", "/get_account_context?account_id=A-1"], calls
        assert model_calls == [], "the model was called after a 404 proved the token remapped"
    finally:
        asvc._svc, ing._post_ironclaw = original_svc, original_post
    print("  PASS org-A catalog -> org-B context 404 refuses before any model call")


def test_fact_fields_mismatch_refuses_all_three_states_are_distinct():
    d, st = _tmp_state(); undeclared = client(fact_fields=None); persist(st, undeclared)
    assert_refuses(st, dataclasses.replace(undeclared, fact_fields=()), "FACT_FIELDS")
    st.reset_thread(GID)
    declared_empty = client(fact_fields=())
    persist(st, declared_empty)
    assert_refuses(st, dataclasses.replace(declared_empty, fact_fields=("budget",)), "FACT_FIELDS")
    st.close(); d.cleanup(); print("  PASS FACT_FIELDS mismatch refuses and tri-state is preserved")


V1_SCHEMA = """
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE threads (
  gid TEXT PRIMARY KEY, prev TEXT, supplied TEXT NOT NULL DEFAULT '{}',
  ever_supplied INTEGER NOT NULL DEFAULT 0, last_turn_at TEXT,
  orphans TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO meta(key, value) VALUES('schema_version', '1');
"""


def make_v1(path, active):
    db = sqlite3.connect(str(path))
    db.executescript(V1_SCHEMA)
    db.execute("INSERT INTO threads(gid, prev, supplied, ever_supplied, last_turn_at, orphans) "
               "VALUES(?,?,?,?,?,?)",
               (GID, "resp_legacy" if active else None,
                json.dumps({"A-1": "v1"}) if active else "{}", 1 if active else 0,
                "2026-08-24T00:00:00+00:00" if active else None, "{}"))
    db.commit(); db.close()


V2_SCHEMA = """
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE threads (
  gid TEXT PRIMARY KEY, prev TEXT, supplied TEXT NOT NULL DEFAULT '{}',
  ever_supplied INTEGER NOT NULL DEFAULT 0, last_turn_at TEXT,
  orphans TEXT NOT NULL DEFAULT '{}',
  service TEXT, service_version INTEGER, instructions_sha256 TEXT,
  model TEXT, context_policy_sha256 TEXT
);
INSERT INTO meta(key, value) VALUES('schema_version', '2');
"""


def make_v2(path, active, identity=True):
    """A GENUINE HISTORICAL v2: the five compatibility columns v2 actually shipped with, and
    neither of the two that were added later without a version bump. This is the shape on the
    operator's own host — and the shape the later code refused to open."""
    db = sqlite3.connect(str(path))
    db.executescript(V2_SCHEMA)
    db.execute("INSERT INTO threads(gid, prev, supplied, ever_supplied, last_turn_at, orphans, "
               "service, service_version, instructions_sha256, model, context_policy_sha256) "
               "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
               (GID, "resp_v2" if active else None,
                json.dumps({"A-1": "v2"}) if active else "{}", 1 if active else 0,
                "2026-08-26T00:00:00+00:00" if active else None, "{}",
                *(("account-analysis", 1, "sha-i", "model-x", "sha-c") if identity
                  else (None, None, None, None, None))))
    db.commit(); db.close()


def filesystem_snapshot(root):
    """Names, bytes and modes: the observational boundary, not only SQL contents."""
    return {p.name: (hashlib.sha256(p.read_bytes()).hexdigest(),
                     stat.S_IMODE(p.stat().st_mode))
            for p in pathlib.Path(root).iterdir() if p.is_file()}


def test_schema_v1_migrates_to_current_additive_backed_up_preserving_active_state():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v1(path, active=True)
        st = bs.BridgeState(path)
        row = st.thread_row(GID)
        assert st.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
        assert row["prev"] == "resp_legacy" and json.loads(row["supplied"]) == {"A-1": "v1"}
        assert all(row[k] is None for k in bs.IDENTITY_FIELDS)
        backups = list(path.parent.glob(path.name + ".v1.bak-*"))
        assert len(backups) == 1 and stat.S_IMODE(backups[0].stat().st_mode) == 0o600
        assert st.meta_get("schema_v1_backup") == str(backups[0])
        backup_db = sqlite3.connect(str(backups[0]))
        assert backup_db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert backup_db.execute(
            "SELECT value FROM meta WHERE key='schema_version'").fetchone()[0] == "1"
        assert backup_db.execute(
            "SELECT prev FROM threads WHERE gid=?", (GID,)).fetchone()[0] == "resp_legacy"
        backup_db.close()
        st.close()
        reopened = bs.BridgeState(path)
        assert reopened.thread_row(GID)["prev"] == "resp_legacy"
        assert list(path.parent.glob(path.name + ".v1.bak-*")) == backups
        assert reopened.meta_get("schema_v1_backup") == str(backups[0])
        reopened.close()
    print("  PASS v1->v2 creates one usable backup; repeated opens create no more")


def test_genuine_historical_v2_migrates_instead_of_being_refused():
    """THE COLLISION. `organization_id` and `account_service_base` were added to the v2 shape
    without bumping the version, so "schema version 2" named two different tables. A database
    written by the earlier code carries five identity columns and a stamp of `2`; the later code
    read the stamp, skipped migration, then failed the completeness check and told the operator
    to restore a v1 backup — which a database born at v2 does not have. Measured on the
    operator's own host: five identity columns, one live conversation pointer, bridge dead."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        st = bs.BridgeState(path)
        row = st.thread_row(GID)
        assert st.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
        # ROWS PRESERVED, including the live pointer and the identity v2 legitimately held.
        assert row["prev"] == "resp_v2" and json.loads(row["supplied"]) == {"A-1": "v2"}
        assert row["service"] == "account-analysis" and row["model"] == "model-x"
        # ...and NOTHING invented for the two columns v2 never had.
        assert row["organization_id"] is None and row["account_service_base"] is None
        backups = list(path.parent.glob(path.name + ".v2.bak-*"))
        assert len(backups) == 1, backups
        assert stat.S_IMODE(backups[0].stat().st_mode) == 0o600
        assert st.meta_get("schema_v2_backup") == str(backups[0])
        b = sqlite3.connect(str(backups[0]))
        assert b.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert b.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0] == "2"
        assert b.execute("SELECT prev FROM threads WHERE gid=?", (GID,)).fetchone()[0] == "resp_v2"
        b.close()
        st.close()
    print("  PASS a genuine historical v2 migrates, backed up, with rows preserved")


def test_a_seven_column_intermediate_v2_migrates_rather_than_being_refused():
    """"Schema version 2" named THREE shapes, not two. Between the five-column v2 and the
    version bump, a database could be written with `organization_id` and `account_service_base`
    already present and still stamped `2` — the intermediate state of the very drift that made
    v3 necessary. It is a legitimate candidate and must converge, not be rejected as malformed.

    It works because `_migrate_schema` adds only columns that are absent, so the step degenerates
    to re-stamping. Pinned here because nothing else distinguishes it from the five-column case,
    and because the fix for it must not weaken the rejection asserted by
    `test_incomplete_schema_v2_fails_closed_instead_of_self_repairing` — a v1 BODY stamped `2`
    still has none of v2's identity columns and still fails closed."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        db = sqlite3.connect(str(path))       # the two later columns, before the version moved
        db.execute("ALTER TABLE threads ADD COLUMN organization_id TEXT")
        db.execute("ALTER TABLE threads ADD COLUMN account_service_base TEXT")
        db.execute("UPDATE threads SET organization_id='org-legacy' WHERE gid=?", (GID,))
        db.commit(); db.close()

        st = bs.BridgeState(path)
        row = st.thread_row(GID)
        assert st.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
        assert row["prev"] == "resp_v2", "the conversation pointer was lost"
        assert row["service"] == "account-analysis"
        assert row["organization_id"] == "org-legacy", \
            "a value the database already held was overwritten by the migration"
        st.close()
    print("  PASS a seven-column intermediate v2 converges without losing what it held")


def test_the_backup_carries_committed_WAL_content():
    """`db.backup()` is SQLite's own API, so it copies committed pages still living in the -wal
    file. A plain file copy would not, and the rows most likely to be there are the most recent
    ones — the conversation pointer a crash would otherwise strand."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        db = sqlite3.connect(str(path))
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("INSERT INTO threads(gid, prev) VALUES('-100wal','resp_in_wal')")
        db.commit(); db.close()               # committed, not necessarily checkpointed

        bs.BridgeState(path).close()
        backup = sorted(path.parent.glob(path.name + ".v2.bak-*"))[0]
        b = sqlite3.connect(str(backup))
        rows = dict(b.execute("SELECT gid, prev FROM threads"))
        b.close()
        assert rows.get("-100wal") == "resp_in_wal", \
            f"the backup lost committed WAL content: {rows}"
        assert rows.get(GID) == "resp_v2"
    print("  PASS the pre-migration backup includes committed WAL content")


def test_migrating_v2_is_idempotent_and_makes_exactly_one_backup():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        first = bs.BridgeState(path); first.close()
        backups = list(path.parent.glob(path.name + ".v2.bak-*"))
        for _ in range(3):
            again = bs.BridgeState(path)
            assert again.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
            assert again.thread_row(GID)["prev"] == "resp_v2"
            again.close()
        assert list(path.parent.glob(path.name + ".v2.bak-*")) == backups, \
            "reopening a migrated database made another backup"
    print("  PASS v2 migration is idempotent; repeated opens create no further backups")


def test_an_interrupted_v2_migration_leaves_the_database_openable_and_retryable():
    """The ALTERs and the version stamp share one transaction, so a crash mid-migration must
    leave a v2 that still opens — and migrates — on the next attempt. An extra backup file is
    the acceptable cost; a half-migrated database that opens as neither version is not."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        boom = RuntimeError("power cut mid-migration")
        real = bs.BridgeState._tx

        def explode(self):
            raise boom

        bs.BridgeState._tx = explode
        try:
            bs.BridgeState(path)
        except RuntimeError as e:
            assert e is boom
        else:
            raise AssertionError("the injected failure did not fire")
        finally:
            bs.BridgeState._tx = real

        db = sqlite3.connect(str(path))
        assert db.execute(
            "SELECT value FROM meta WHERE key='schema_version'").fetchone()[0] == "2", \
            "an interrupted migration left a version stamp it had not earned"
        assert "organization_id" not in {r[1] for r in db.execute("PRAGMA table_info(threads)")}
        db.close()

        st = bs.BridgeState(path)          # the retry
        assert st.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
        assert st.thread_row(GID)["prev"] == "resp_v2", "the retry lost the conversation pointer"
        st.close()
    print("  PASS an interrupted v2 migration is retryable and loses nothing")


def test_a_migrated_v2_row_with_state_still_refuses_rather_than_rebinding():
    """Migration moves the SHAPE, never the meaning. The two new columns are NULL, so a row that
    carries a conversation is refused until an operator resets it — the same rule v1 rows get,
    and the reason nothing here has to guess an organization for a thread that predates them."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        st = bs.BridgeState(path)
        assert_refuses(st, client(), "legacy compatibility identity missing")
        st.reset_thread(GID)
        st.close()
    print("  PASS a populated migrated v2 row refuses continuation instead of rebinding")


def test_an_empty_migrated_v2_row_binds_without_a_reset():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=False, identity=False)
        st = bs.BridgeState(path)
        persist(st, client())
        row = st.thread_row(GID)
        assert row["organization_id"] == client().thread_identity["organization_id"]
        st.close()
    print("  PASS an empty migrated v2 row binds identity with no operator action")


def test_a_read_only_opener_reports_the_upgrade_instead_of_performing_it():
    """A STATUS CHECK THAT WRITES IS NOT A STATUS CHECK. `ironworks doctor` — `--offline doctor`
    included, which promises to measure nothing live — reached the store through the console's
    per-tenant thread view and through `_bridge_read`, and opening is what runs `_check_version`.
    So a read-only diagnostic silently migrated the operator's live database and wrote a backup
    beside it, reporting neither. Observed on a real host during a verification run, which is how
    it was found. Migration belongs to the bridge starting, or to an explicit operator command."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        try:
            bs.BridgeState(path, migrate=False)
        except bs.MigrationRequired as e:
            assert e.got == "2", e.got
            assert "nothing was changed" in str(e), e
        else:
            raise AssertionError("a read-only open migrated the store")
        db = sqlite3.connect(str(path))
        assert db.execute(
            "SELECT value FROM meta WHERE key='schema_version'").fetchone()[0] == "2"
        assert "organization_id" not in {r[1] for r in db.execute("PRAGMA table_info(threads)")}
        db.close()
        assert list(path.parent.glob(path.name + ".v*.bak-*")) == [], \
            "a read-only open wrote a backup, so it had already decided to migrate"
        # ...and the writer still may.
        st = bs.BridgeState(path)
        assert st.meta_get("schema_version") == str(bs.SCHEMA_VERSION)
        st.close()
    print("  PASS a read-only open reports the pending upgrade and changes nothing")


def test_a_read_only_opener_will_not_create_a_store():
    """Creating the file it was asked to inspect is the same defect with a friendlier face: the
    reader answers its own question, and an absent bridge reads as an empty healthy one."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "absent.db"
        try:
            bs.BridgeState(path, migrate=False)
        except bs.MigrationRequired:
            pass
        else:
            raise AssertionError("a read-only open stamped a brand-new store")
    print("  PASS a read-only open does not create the store")


def test_read_only_current_store_creates_no_wal_or_shm_sidecars():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        st = bs.BridgeState(path); st.close()
        before = filesystem_snapshot(tmp)
        reader = bs.BridgeState(path, migrate=False)
        reader.progress_snapshot()
        reader.close()
        assert filesystem_snapshot(tmp) == before, \
            "read-only inspection created or changed SQLite files"
    print("  PASS a current-store inspection is byte-for-byte observational")


def test_read_only_wal_snapshot_sees_committed_state_without_touching_source_sidecars():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        writer = bs.BridgeState(path)
        writer.db.execute("PRAGMA wal_autocheckpoint=0")
        writer.db.execute("INSERT INTO threads(gid, prev) VALUES('-wal-read','committed')")
        before = filesystem_snapshot(tmp)
        reader = bs.BridgeState(path, migrate=False)
        assert reader.thread_row("-wal-read")["prev"] == "committed"
        reader.close()
        assert filesystem_snapshot(tmp) == before, \
            "WAL inspection changed the source DB/WAL/SHM set"
        writer.close()
    print("  PASS committed WAL is read from a non-mutating snapshot")


def test_a_future_schema_version_is_refused_not_migrated():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v2(path, active=True)
        db = sqlite3.connect(str(path))
        db.execute("UPDATE meta SET value=? WHERE key='schema_version'",
                   (str(bs.SCHEMA_VERSION + 1),))
        db.commit(); db.close()
        try:
            bs.BridgeState(path)
        except bs.StateError as e:
            assert f"schema version {bs.SCHEMA_VERSION + 1}" in str(e), e
            assert "downgrade" in str(e), e
        else:
            raise AssertionError("a future schema version was opened")
        assert list(path.parent.glob(path.name + ".v*.bak-*")) == [], \
            "a refused future version was still backed up, implying an attempted migration"
    print("  PASS a future schema version is refused without being touched")


def test_future_schema_rejection_does_not_recreate_a_missing_index_or_sidecars():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        st = bs.BridgeState(path)
        st.db.execute("UPDATE meta SET value='99' WHERE key='schema_version'")
        st.db.execute("DROP INDEX updates_state")
        st.close()
        before = filesystem_snapshot(tmp)
        try:
            bs.BridgeState(path)
        except bs.StateError:
            pass
        else:
            raise AssertionError("a future schema was accepted")
        assert filesystem_snapshot(tmp) == before, \
            "future-schema rejection repaired or otherwise changed the source"
    print("  PASS a future schema is rejected before writer initialization")


def test_existing_active_database_without_meta_is_not_silently_stamped_current():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        st = bs.BridgeState(path)
        st.db.execute("INSERT INTO threads(gid, prev) VALUES('-malformed','resp_live')")
        st.close()
        db = sqlite3.connect(str(path)); db.execute("DROP TABLE meta"); db.commit(); db.close()
        before = filesystem_snapshot(tmp)
        try:
            bs.BridgeState(path)
        except bs.StateError as e:
            assert "malformed" in str(e) and "refusing" in str(e)
        else:
            raise AssertionError("an existing unstamped database was repaired and accepted")
        assert filesystem_snapshot(tmp) == before, \
            "malformed-schema rejection created metadata, sidecars, indexes or backups"
    print("  PASS malformed active state is rejected without repair")


def test_preversioned_json_refuses_then_supported_migration_stays_identityless():
    """JSON migration repairs freshness shape; it never guesses A6 compatibility identity."""
    with tempfile.TemporaryDirectory() as tmp:
        src = pathlib.Path(tmp) / "bridge-threads.json"
        src.write_text(json.dumps({GID: {
            "prev": "resp_legacy", "supplied": ["A-1"], "ever_supplied": True}}))
        try:
            bs.migrate_from_json(src, pathlib.Path(tmp) / "refused.db")
        except bs.LegacyStateError as e:
            assert "UPGRADE.md" in str(e), e
        else:
            raise AssertionError("pre-versioned JSON was silently coerced")

        src.write_text(json.dumps({GID: {
            "prev": "resp_legacy", "supplied": {"A-1": None},
            "ever_supplied": True}}))
        st, migrated = bs.migrate_from_json(src, pathlib.Path(tmp) / "supported.db")
        row = st.thread_row(GID)
        assert migrated == 1 and row["prev"] == "resp_legacy"
        assert json.loads(row["supplied"]) == {"A-1": None}
        assert all(row[k] is None for k in bs.IDENTITY_FIELDS), \
            "migration guessed compatibility identity"
        assert_refuses(st, client(), "legacy compatibility identity missing")
        st.close()
    print("  PASS JSON migration refuses old shape; supported migration remains identityless")


def test_incomplete_schema_v2_fails_closed_instead_of_self_repairing():
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "state.db"
        make_v1(path, active=False)
        db = sqlite3.connect(str(path))
        db.execute("UPDATE meta SET value='2' WHERE key='schema_version'")
        db.commit(); db.close()
        try:
            bs.BridgeState(path)
        except bs.StateError as e:
            assert "claims schema version 2" in str(e) and "organization_id" in str(e)
        else:
            raise AssertionError("an incomplete schema v2 was silently repaired")
    print("  PASS incomplete schema v2 fails closed")


def test_active_legacy_row_refuses_but_empty_legacy_row_auto_binds():
    with tempfile.TemporaryDirectory() as tmp:
        active_path = pathlib.Path(tmp) / "active.db"
        make_v1(active_path, active=True)
        active = bs.BridgeState(active_path)
        assert_refuses(active, client(), "legacy compatibility identity missing")
        active.close()

        empty_path = pathlib.Path(tmp) / "empty.db"
        make_v1(empty_path, active=False)
        empty = bs.BridgeState(empty_path)
        loaded = tb._load_threads({GID: client()}, state=empty)[GID]
        assert loaded.prev is None and loaded.supplied == {}
        assert empty.stored_identity(empty.thread_row(GID)) == client().thread_identity
        empty.close()
    print("  PASS active legacy state refuses; empty legacy state auto-binds")


def test_reset_clears_all_conversation_context_identity_and_preserves_delivery():
    d, st = _tmp_state(); persist(st, client())
    st.note_received(71, GID, 71)
    st.note_terminal(71, bs.FAILED_TERMINAL, 72, "fixture")
    delivery_before = dict(st.update_row(71))
    cursor_before, cursor_acked_before = st.cursor, st.cursor_acked
    assert st.reset_thread(GID) == 1
    assert st.thread_row(GID) is None, "conversation/context/identity row survived reset"
    assert dict(st.update_row(71)) == delivery_before, "reset changed delivery evidence"
    assert (st.cursor, st.cursor_acked) == (cursor_before, cursor_acked_before)
    st.close(); d.cleanup()
    print("  PASS reset clears continuity and preserves delivery evidence exactly")


if __name__ == "__main__":
    for _name, _fn in list(globals().items()):
        if _name.startswith("test_") and callable(_fn):
            _fn()
    print("ALL THREAD-COMPATIBILITY TESTS PASS")
