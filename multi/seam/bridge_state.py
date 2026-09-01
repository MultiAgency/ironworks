"""The bridge's durable state — one store, one transaction, no split brain.

WHY ONE STORE. Two facts have to stay in agreement across a crash:

  1. the conversation pointer for a group (`thread.prev`, `supplied`, `ever_supplied`), and
  2. what happened to the Telegram update that produced it, and how to recover or deliver it.

Keep them in two files and they can disagree in one direction that silently corrupts a live
conversation: the journal says a turn completed while the thread file still points at the turn
before it, so the NEXT turn chains from a stale parent and the group loses a turn with nothing
reporting it. That failure is not detectable after the fact — both files are individually
well-formed. A careful write order plus a startup reconciliation could be made correct, but it
would be correct by argument, and the argument would have to be re-made every time someone
touched either writer.

So: one SQLite database, `sqlite3` from the standard library, and the pair above is written in
a single transaction. The consistency question stops being a design and becomes a property.

WHAT IT DELIBERATELY DOES NOT HOLD. No message text, no response text, no account records, no
persona or guidance content, no credentials, no request headers. It does hold non-secret
compatibility fingerprints so a response chain cannot cross a composition change. Recovery is by
IDENTIFIER — an IronClaw response id, or an idempotency key this process chose. The one judgement call is the
Telegram group id, which is stored in clear: it is already the key of the file this replaces,
it is what `deprovision.sh` looks a tenant up by, and it is operator routing infrastructure
rather than client content. `message_id` is stored for the same reason — it identifies a
message without carrying a word of it.

STATES. An update walks one path and stops:

    RECEIVED ──► TURN_STARTED ──► TURN_COMPLETED ──► DELIVERY_STARTED ──► DELIVERED ──► ACKED
        │             │
        │             └──► RECOVERY_BLOCKED   (a turn MAY have run and cannot be recovered)
        └──► IGNORED                          (not addressed to us, or not a registered group)
                      ├──► DELIVERY_RETRY     (answer retained; first chunk rejected)
                      ├──► DELIVERY_RECONCILE (answer retained; Telegram delivery uncertain)
                      └──► FAILED_TERMINAL    (a stable pre-model failure)

`ACKED` is not decoration: on this protocol an update is only truly finished once Telegram has
been told an offset past it, which happens on a LATER `getUpdates`. Until then Telegram may
redeliver it, so `DELIVERED` and `ACKED` are genuinely different facts and the journal keeps
them apart.
"""
import datetime
import json
import os
import pathlib
import shutil
import sqlite3
import tempfile
import threading
import time

SCHEMA_VERSION = 3

# THE SHAPE EACH VERSION ACTUALLY HAD, because "schema version 2" named two different tables.
# v2 introduced five compatibility columns; `organization_id` and `account_service_base` were
# added later and the version was not bumped with them. A database written by the earlier code
# is therefore a LEGITIMATE v2 that the later code refuses as internally inconsistent — and the
# refusal advised restoring a v1 backup, which a database born at v2 does not have. Measured on
# the operator's own host: schema_version=2, five identity columns, one live conversation
# pointer, and a bridge that will not start.
#
# Split into two tuples so the upgrade path is data rather than a guess: v1 takes both, a
# complete v2 takes only the second, and a v1 body mislabelled "2" matches neither and falls
# through to the consistency check, which is what it deserves.
V2_IDENTITY = (("service", "TEXT"), ("service_version", "INTEGER"),
               ("instructions_sha256", "TEXT"), ("model", "TEXT"),
               ("context_policy_sha256", "TEXT"))
V3_IDENTITY = (("organization_id", "TEXT"), ("account_service_base", "TEXT"))

# The walk above, as data.
RECEIVED = "RECEIVED"
TURN_STARTED = "TURN_STARTED"
TURN_COMPLETED = "TURN_COMPLETED"
DELIVERY_STARTED = "DELIVERY_STARTED"
DELIVERED = "DELIVERED"
ACKED = "ACKED"
IGNORED = "IGNORED"
RECOVERY_BLOCKED = "RECOVERY_BLOCKED"
DELIVERY_RETRY = "DELIVERY_RETRY"
DELIVERY_RECONCILE = "DELIVERY_RECONCILE"
FAILED_TERMINAL = "FAILED_TERMINAL"

# The forward walk, in order. It builds ALL_STATES (the write-time validation set) and names
# the happy path in one place. It is NOT an ordinal scale: nothing compares positions in it, and
# `bridge_core.handle_update` dispatches on equality — deliberately, because the recovery
# branches turn on the exact state, not on "at least as far as".
PROGRESS = (RECEIVED, TURN_STARTED, TURN_COMPLETED, DELIVERY_STARTED, DELIVERED, ACKED)
TERMINAL = (ACKED, IGNORED, RECOVERY_BLOCKED, DELIVERY_RETRY, DELIVERY_RECONCILE,
            FAILED_TERMINAL)
# States where WORK HAS BEGUN and may have been billed: a turn was started, or an answer exists
# and its delivery was started. A row in one of these is never a candidate for IGNORED, whatever
# the routing table currently says — see `bridge_core.handle_update`.
IN_FLIGHT = (TURN_STARTED, TURN_COMPLETED, DELIVERY_STARTED)
ALL_STATES = PROGRESS + (IGNORED, RECOVERY_BLOCKED, DELIVERY_RETRY, DELIVERY_RECONCILE,
                         FAILED_TERMINAL)
# Delivery retry/reconciliation states retain the only operator handle to an answer that may not
# have arrived. They are never age-compacted; explicit successful redelivery moves them to ACKED.
#
# RECOVERY_BLOCKED belongs with them, and did not. `health()` derives "N update(s) are
# RECOVERY_BLOCKED and need operator reconciliation (SECURITY.md)" purely from
# `counts_by_state()`, so compacting the row deleted the alarm: on a bridge doing ~600 updates
# between operator checks, the record that a turn may have been billed and never delivered
# vanished and the gate went green with nobody having acted. The state means UNRESOLVED — a row
# nothing has resolved cannot expire on age.
COMPACTABLE_TERMINAL = (ACKED, IGNORED, FAILED_TERMINAL)

# Terminal rows kept behind the acknowledged cursor. Enough to diagnose a duplicate-delivery
# report from a client days later; small enough that the file never becomes a growth problem.
RETAIN_TERMINAL = int(os.environ.get("BRIDGE_RETAIN_TERMINAL", "500"))

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);
CREATE TABLE IF NOT EXISTS threads (
    gid                   TEXT PRIMARY KEY,
    prev                  TEXT,
    supplied              TEXT NOT NULL DEFAULT '{}', -- {account_id: updated_at-as-supplied}
    ever_supplied         INTEGER NOT NULL DEFAULT 0,
    last_turn_at          TEXT,
    orphans               TEXT NOT NULL DEFAULT '{}', -- {account_id: [catalog_version, attempts]}
    service               TEXT,
    service_version       INTEGER,
    instructions_sha256   TEXT,
    model                 TEXT,
    context_policy_sha256 TEXT,
    organization_id       TEXT,
    account_service_base  TEXT
);
CREATE TABLE IF NOT EXISTS updates (
    update_id       INTEGER PRIMARY KEY,
    gid             TEXT,
    state           TEXT NOT NULL,
    idempotency_key TEXT,
    response_id     TEXT,
    prev_before     TEXT,
    prev_after      TEXT,
    attempts        INTEGER NOT NULL DEFAULT 0,
    first_seen      TEXT,
    updated_at      TEXT,
    error_code      TEXT,
    message_id      INTEGER,
    delivered_at    TEXT
);
CREATE INDEX IF NOT EXISTS updates_state ON updates(state);
CREATE TABLE IF NOT EXISTS workers (
    gid         TEXT PRIMARY KEY,
    update_id   INTEGER NOT NULL,
    stage       TEXT NOT NULL,
    started_at  TEXT,
    deadline_at TEXT,
    heartbeat_at TEXT
);
"""


# ── the two operator-facing refusals, each written once ───────────────────────────────
# Both of these are read by a human deciding what to do with a live conversation store, and both
# existed TWICE with the wording duplicated verbatim (`_classify_schema`/`_check_version`, and
# `_open`/`_read_stamp`). Two copies of a paragraph is two paragraphs to keep in step: the next
# person to sharpen the corrupt-database advice would have improved one of them.


def _unusable_db_message(path, error):
    """A database SQLite itself will not open. The 'do NOT delete it blind' half is the point."""
    return (f"{path} is not a usable bridge state database ({error}). If it is corrupt, move it "
            "aside and re-migrate from the JSON backup; do NOT delete it blind — it carries "
            "every group's conversation pointer.")


def _wrong_version_message(path, version):
    """A schema version this bridge does not implement — a downgrade, or a half-run migration."""
    return (f"{path} is schema version {version}, this bridge implements {SCHEMA_VERSION}. "
            "Refusing to run: an unknown version means either a downgrade or an unfinished "
            "migration, and guessing which would risk a live conversation.")


class StateError(RuntimeError):
    """The store is unreadable, unwritable, or of a version this code does not implement."""


class MigrationRequired(StateError):
    """The store needs a schema upgrade and this reader was told not to perform one."""

    def __init__(self, path, got):
        self.got = str(got)
        super().__init__(
            f"{path} is schema version {got} and needs an upgrade to {SCHEMA_VERSION}. This "
            "reader opened it read-only, so nothing was changed. Migration happens when the "
            "bridge starts, or on an explicit operator command — never as a side effect of a "
            "status check.")


class LegacyStateError(StateError):
    """A pre-versioning bridge-threads.json was found and must be migrated deliberately."""


IDENTITY_FIELDS = ("service", "service_version", "instructions_sha256", "model",
                   "context_policy_sha256", "organization_id", "account_service_base")
IDENTITY_LABELS = {"service": "service", "service_version": "service version",
                   "instructions_sha256": "instructions", "model": "model",
                   "context_policy_sha256": "FACT_FIELDS",
                   "organization_id": "organization scope",
                   "account_service_base": "Account Service endpoint"}


class ThreadCompatibilityError(StateError):
    """A stored conversation cannot safely continue under the current tenant composition."""

    def __init__(self, slug, gid, categories):
        self.slug, self.gid, self.categories = slug, str(gid), tuple(categories)
        reset = f"./deploy/ironworks tenant reset-thread {slug} --confirm {slug}"
        super().__init__(
            f"tenant {slug!r} group {gid}: persisted conversation is incompatible "
            f"({', '.join(self.categories)}). Refusing continuation; stop the bridge and run: "
            f"{reset}")


def _now():
    """Whole-second UTC, for every column in this store.

    DELIBERATELY NOT `envelope.now_iso`, which the seam's other two writers now share. This one
    truncates microseconds: these stamps are read by an operator in `ironworks bridge status` and
    in raw SQL during an incident, where sub-second precision is noise on a value whose smallest
    meaningful unit is a poll interval. `envelope.now_iso` keeps full precision because
    `retrieved_at` is model-visible and `thread.last_turn_at` feeds `bridge_core._age`.

    THE TWO SHAPES MEET, and that is safe rather than accidental: `_age` parses whichever it is
    given with `datetime.fromisoformat`, which accepts both (measured). Unifying them would
    change either every row in a live database or a model-visible field, for no gain."""
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()


def _as_offset(v):
    """A Telegram offset out of `meta`, which stores every value as TEXT.

    One coercion rule for the three readers of it (`cursor`, `cursor_acked`, and the console's
    snapshot), because an offset that arrives as a string compares wrong against an int in
    every one of them."""
    return int(v) if v not in (None, "") else None


_TABLE_COLUMNS = {
    "meta": ("key", "value"),
    "threads": ("gid", "prev", "supplied", "ever_supplied", "last_turn_at", "orphans",
                *IDENTITY_FIELDS),
    "updates": ("update_id", "gid", "state", "idempotency_key", "response_id",
                "prev_before", "prev_after", "attempts", "first_seen", "updated_at",
                "error_code", "message_id", "delivered_at"),
    "workers": ("gid", "update_id", "stage", "started_at", "deadline_at", "heartbeat_at"),
}
# HISTORY, WRITTEN OUT — not `_TABLE_COLUMNS["threads"][:6]`. Slicing the current schema defines
# the past as "whatever the present happens to start with", which is true only while every
# migration APPENDS. Insert a column into `_SCHEMA` instead of appending and `[:6]` silently
# denotes a different set, every database on disk stops matching any supported shape, and the
# refusal blames the operator's file. These are the names those versions actually had;
# `test_thread_compatibility` asserts they remain prefixes of the current schema, so a reordering
# fails in CI rather than on a host at open time.
_V1_THREAD_COLUMNS = ("gid", "prev", "supplied", "ever_supplied", "last_turn_at", "orphans")
_V2_THREAD_COLUMNS = _V1_THREAD_COLUMNS + (
    "service", "service_version", "instructions_sha256", "model", "context_policy_sha256")


def _source_signature(path):
    """Facts cheap enough to compare around a snapshot copy without touching the source."""
    try:
        st = pathlib.Path(path).stat()
        return st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns
    except FileNotFoundError:
        return None


def _snapshot_database(path):
    """Copy a stable main-db/WAL pair without opening the operator's SQLite files.

    SQLite's nominal read-only connection may create ``-wal``/``-shm`` files and writes the
    shared-memory index while reading.  Conversely, ``immutable=1`` ignores a live WAL.  Neither
    satisfies an observational command.  A byte snapshot does: copy the main database and WAL
    without opening either, reject a concurrently moving source, then let SQLite rebuild any
    required SHM state beside the disposable copy.
    """
    source = pathlib.Path(path)
    if not source.exists():
        raise MigrationRequired(source, "absent")
    holder = tempfile.TemporaryDirectory(prefix="ironworks-bridge-read-")
    target = pathlib.Path(holder.name) / source.name
    wal = pathlib.Path(str(source) + "-wal")
    before = (_source_signature(source), _source_signature(wal))
    try:
        shutil.copyfile(source, target)
        if before[1] is not None:
            shutil.copyfile(wal, pathlib.Path(str(target) + "-wal"))
        after = (_source_signature(source), _source_signature(wal))
        if before != after:
            raise StateError(
                f"bridge state at {source} changed while it was being inspected; refusing an "
                "inconsistent observational snapshot")
        return holder, target
    except BaseException:
        holder.cleanup()
        raise


def _columns_on(db, table):
    return tuple(r[1] for r in db.execute(f"PRAGMA table_info({table})"))


def _signature_on(db, table):
    return tuple((r[1], (r[2] or "").upper(), r[3], r[4], r[5])
                 for r in db.execute(f"PRAGMA table_info({table})"))


def _expected_signatures():
    db = sqlite3.connect(":memory:")
    try:
        db.executescript(_SCHEMA)
        return {table: _signature_on(db, table) for table in _TABLE_COLUMNS}
    finally:
        db.close()


def _thread_shapes(version, expected, path):
    if version == "1":
        return (expected[:len(_V1_THREAD_COLUMNS)],)
    if version == "2":
        return (expected[:len(_V2_THREAD_COLUMNS)], expected)
    if version == str(SCHEMA_VERSION):
        return (expected,)
    raise StateError(_wrong_version_message(path, version))


def _validate_auxiliary_schema(db, path, version, objects, tables, expected_signatures):
    known_tables = set(_TABLE_COLUMNS)
    if not tables <= known_tables:
        raise StateError(f"{path} contains unknown bridge tables: "
                         f"{', '.join(sorted(tables - known_tables))}")
    # AUXILIARY TABLES GET THE SAME APPEND-ONLY RULE AS `threads`, and did not. They were held to
    # the CURRENT signature at every version, so a legitimate v1 whose `updates` predates a later
    # column was rejected as "malformed" — a word that describes corruption — with no route
    # forward, because migration only ever ALTERed `threads`. The database became unopenable and
    # unrecoverable through this module. That is the same drift that made "schema version 2" name
    # two shapes; it just landed on a different table.
    #
    # A historical version may therefore present a PREFIX of the current signature: exactly the
    # columns it had, with the ones appended since still missing. `_migrate_schema` adds them.
    # Anything that is not a prefix is genuinely unrecognised, and says so in those words.
    for table in tables - {"threads", "meta"}:
        actual, expected = _signature_on(db, table), expected_signatures[table]
        if actual == expected:
            continue
        if version != str(SCHEMA_VERSION) and actual == expected[:len(actual)]:
            continue
        raise StateError(
            f"{path} declares schema version {version} but its {table} table matches no shape "
            "this bridge recognises — not the current one, and not a prefix of it. It is either "
            "corrupt or from a fork; refusing to repair it in place. Restore its recorded backup "
            "or re-migrate from the JSON source.")
    if version != str(SCHEMA_VERSION):
        return
    missing = known_tables - tables
    if missing:
        raise StateError(f"{path} claims schema version {version} but is missing tables: "
                         f"{', '.join(sorted(missing))}; refusing to repair it")
    indexes = {name for name, typ in objects if typ == "index"}
    if "updates_state" not in indexes:
        raise StateError(f"{path} claims schema version {version} but is missing the "
                         "updates_state index; refusing to repair it")
    index_columns = tuple(r[2] for r in db.execute("PRAGMA index_info(updates_state)"))
    if index_columns != ("state",):
        raise StateError(f"{path} has a malformed updates_state index; refusing to repair it")


def _classify_schema(db, path):
    """Return the proven schema version/shape, or reject without repairing anything."""
    rows = db.execute("SELECT name, type FROM sqlite_master "
                      "WHERE name NOT LIKE 'sqlite_%'").fetchall()
    objects = {(r[0], r[1]) for r in rows}
    tables = {name for name, typ in objects if typ == "table"}
    if "meta" not in tables or "threads" not in tables:
        raise StateError(f"{path} is malformed: an existing bridge database must contain both "
                         "meta and threads; refusing to create or repair them")
    expected_signatures = _expected_signatures()
    if _signature_on(db, "meta") != expected_signatures["meta"]:
        raise StateError(f"{path} has a malformed meta table; refusing to repair it")
    versions = db.execute("SELECT value FROM meta WHERE key='schema_version'").fetchall()
    if len(versions) != 1 or versions[0][0] in (None, ""):
        raise StateError(f"{path} has no single authoritative schema version; refusing to stamp "
                         "or repair an existing database")
    version = str(versions[0][0])
    columns = _columns_on(db, "threads")
    expected = _thread_shapes(version, expected_signatures["threads"], path)
    signature = _signature_on(db, "threads")
    if signature not in expected:
        missing = [name for name in _TABLE_COLUMNS["threads"] if name not in columns]
        raise StateError(f"{path} claims schema version {version} but its threads columns do "
                         "not match any supported shape"
                         + (f" (missing: {', '.join(missing)})" if missing else "")
                         + "; refusing to repair it")

    _validate_auxiliary_schema(db, path, version, objects, tables, expected_signatures)
    return version


def _classify_or_state_error(db, path):
    try:
        return _classify_schema(db, path)
    except sqlite3.DatabaseError as e:
        raise StateError(_unusable_db_message(path, e)) from e


def _classify_source(path):
    if not pathlib.Path(path).exists():
        return None
    holder, snapshot = _snapshot_database(path)
    try:
        probe = sqlite3.connect(str(snapshot))
        try:
            return _classify_or_state_error(probe, path)
        finally:
            probe.close()
    finally:
        holder.cleanup()


def inspect_thread_exists(path, gid):
    """Observe one route from any supported schema without opening the source through SQLite."""
    holder, snapshot = _snapshot_database(path)
    try:
        db = sqlite3.connect(str(snapshot))
        try:
            _classify_or_state_error(db, path)
            return db.execute("SELECT 1 FROM threads WHERE gid = ?", (str(gid),)).fetchone() \
                is not None
        finally:
            db.close()
    finally:
        holder.cleanup()


class BridgeState:
    """Durable bridge state. Every mutation that spans two facts is one transaction.

    Opened with `check_same_thread=False`. A re-entrant process lock serializes use of this
    connection; SQLite's WAL still coordinates independent watchdog/operator connections.
    Transactions stay short, so model and Telegram I/O remain concurrent across tenants.
    """

    def __init__(self, path=None, migrate=True):
        """`migrate=False` for READERS: report a pending upgrade, never perform one.

        A DIAGNOSTIC MUST NOT WRITE. `ironworks doctor` — including `--offline doctor`, which
        promises to measure nothing live — opened the store through `_bridge_read`, and opening
        is what runs `_check_version`. So a read-only status command silently performed a schema
        migration on the operator's live bridge database, wrote a backup beside it, and reported
        nothing. Observed here, on this machine, from a verification run. It is the same trap
        `telegram_bridge.state_json_path()` documents for the JSON store ("opening it MIGRATES
        the operator's real thread file — a side effect on a live host rather than a stale
        read"), reached through the other door.

        Migration stays the business of the bridge and of an operator running an explicit
        command, both of which construct with the default.
        """
        # NO DEFAULT, and no environment read. This class used to resolve
        # `BRIDGE_STATE_DB or agency_dir("bridge-state.db")` — a SECOND answer to "where is the
        # store?", and a divergent one twice over: it ignored BRIDGE_STATE (which the product
        # derives the db path from) and it named a file the product never opens. Measured:
        #
        #     neither var set   -> product bridge-threads.db, this class bridge-state.db
        #     BRIDGE_STATE set  -> product <that>.db,         this class bridge-state.db
        #
        # Reached, it would silently CREATE an empty database and report an untouched bridge:
        # no thread rows, so every group's `prev` reset — the exact outcome this module's
        # docstring exists to prevent, arriving as a clean start rather than an error.
        # `deploy/ironworks._bridge_paths` already refuses to re-derive this precedence for the
        # same reason and calls `telegram_bridge.state_db_path()` instead. So does everything
        # else: every caller in the tree passes an explicit path. Ask the one resolver; a store
        # this class had to guess at is not one it should open.
        if path is None:
            raise StateError(
                "BridgeState requires an explicit database path. There is exactly one resolver "
                "for it — telegram_bridge.state_db_path(), which honours BRIDGE_STATE_DB and "
                "BRIDGE_STATE — and a second answer here would open a DIFFERENT file, silently, "
                "with no thread rows. Call BridgeState(telegram_bridge.state_db_path()), or "
                "telegram_bridge.open_state().")
        self._may_migrate = migrate
        self._lock = threading.RLock()
        self.path = pathlib.Path(path)
        self._snapshot_holder = None
        self.db = None
        if not migrate:
            # READ-ONLY MEANS THE SOURCE IS NEVER OPENED BY SQLITE. Refusing to migrate was not
            # enough: everything below this point writes before `_check_version` is ever
            # reached — `mkdir`, creating the file, `executescript` adding any missing table or
            # index, and `PRAGMA journal_mode=WAL` rewriting the header. A SQLite `mode=ro`
            # handle still creates/updates WAL shared-memory sidecars, while `immutable=1`
            # silently ignores committed WAL. Inspect a stable disposable copy instead.
            self._initialize_observational_reader()
            return
        classification = _classify_source(self.path)
        newly_created = classification is None
        if newly_created:
            self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            self.db = sqlite3.connect(str(self.path), timeout=30, isolation_level=None,
                                      check_same_thread=False)
        except sqlite3.Error as e:
            raise StateError(f"cannot open bridge state at {self.path}: {e}") from e
        if newly_created:
            # Mode BEFORE content, the same rule the files this replaces already follow: a
            # write-then-chmod publishes every group's response ids at the process umask for
            # the window in between.
            os.chmod(self.path, 0o600)
        self.db.row_factory = sqlite3.Row
        self._initialize_writer(newly_created, classification)
        if newly_created or classification == str(SCHEMA_VERSION):
            self._check_version()

    def _initialize_observational_reader(self):
        try:
            self._snapshot_holder, snapshot = _snapshot_database(self.path)
            self.db = sqlite3.connect(str(snapshot), timeout=30, isolation_level=None,
                                      check_same_thread=False)
            self.db.row_factory = sqlite3.Row
            _classify_or_state_error(self.db, self.path)
            self._check_version()
        except sqlite3.Error as e:
            self.close()
            raise StateError(f"cannot open bridge state at {self.path}: {e}") from e
        except BaseException:
            self.close()
            raise

    def _initialize_writer(self, newly_created, classification):
        try:
            if newly_created:
                self.db.executescript(_SCHEMA)
                self.meta_set("schema_version", str(SCHEMA_VERSION))
            elif classification in ("1", "2"):
                # Back up and migrate before general schema creation, so the backup remains a
                # faithful historical database rather than a partly repaired current one.
                self._check_version()
                self.db.executescript(_SCHEMA)
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("PRAGMA synchronous=FULL")   # a crash test that survives fsync
        except sqlite3.DatabaseError as e:
            raise StateError(_unusable_db_message(self.path, e)) from e

    # ── schema ────────────────────────────────────────────────────────────────────────
    def _columns(self):
        return {r["name"] for r in self.db.execute("PRAGMA table_info(threads)")}

    def _check_version(self):
        got = self.meta_get("schema_version")
        if got is None:
            if not self._may_migrate:
                # A fresh file is not a migration, but stamping it is still a WRITE, and a
                # reader that creates the store it was asked to inspect has answered its own
                # question. `_bridge_read` treats an absent store as "no bridge here".
                raise MigrationRequired(self.path, "unstamped")
            self.meta_set("schema_version", str(SCHEMA_VERSION))
        elif str(got) != str(SCHEMA_VERSION) and not self._may_migrate:
            raise MigrationRequired(self.path, got)
        elif str(got) == "1":
            self._migrate_schema("1", V2_IDENTITY + V3_IDENTITY)
        elif str(got) == "2":
            # ONLY A COMPLETE HISTORICAL v2 IS UPGRADEABLE. A database carrying all five of v2's
            # identity columns is exactly what the earlier code wrote, and the two v3 columns are
            # additive — so it migrates. A body with fewer than that is not a v2 at all, whatever
            # its stamp says, and falls through to the consistency check below rather than being
            # repaired into a shape nothing ever wrote.
            if all(name in self._columns() for name, _ in V2_IDENTITY):
                self._migrate_schema("2", V3_IDENTITY)
        elif str(got) != str(SCHEMA_VERSION):
            raise StateError(_wrong_version_message(self.path, got))
        missing = [name for name in IDENTITY_FIELDS if name not in self._columns()]
        if missing:
            # `got`, not SCHEMA_VERSION: naming the version this database CLAIMS is the whole
            # diagnosis, and reporting the version we implement instead described the wrong file.
            recorded = self.meta_get(f"schema_v{got}_backup") or self.meta_get("schema_v1_backup")
            where = (f" Its recorded backup is {recorded}."
                     if recorded else
                     " It records no backup, so there is nothing to restore: move it aside and "
                     "re-migrate from the JSON backup rather than deleting it — it carries every "
                     "group's conversation pointer.")
            raise StateError(
                f"{self.path} claims schema version {got} but is missing thread compatibility "
                f"columns: {', '.join(missing)}. Refusing to repair an internally inconsistent "
                f"schema in place.{where}")

    def _migrate_schema(self, from_version, additions):
        """One recognized SQLite upgrade step: additive identity columns, no state guesses.

        BACKED UP FIRST, by SQLite itself so committed WAL content is included, and named for the
        version being LEFT — `.v1.bak-…` or `.v2.bak-…` — so an operator can tell which shape a
        given file holds without opening it.

        Existing rows keep NULL identity, which is the point: `identity_mismatches` reports
        "legacy compatibility identity missing" for any row with a NULL field, so `_load_threads`
        binds only rows carrying no state at all and refuses every active conversation until an
        operator explicitly resets it. Nothing here invents an organization or an endpoint for a
        conversation that predates them.

        The ALTERs and the version stamp share ONE transaction, so an interruption leaves the
        database at `from_version` with its columns unchanged and the next open migrates again.
        A backup written before an interrupted attempt is simply an extra file, never a lost one.
        """
        stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
        base = self.path.name + f".v{from_version}.bak-{stamp}"
        backup = self.path.with_name(base)
        suffix = 1
        while True:
            try:
                # Mode before content: the backup carries every group's response pointer.
                fd = os.open(backup, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                os.close(fd)
                break
            except FileExistsError:
                backup = self.path.with_name(f"{base}-{suffix}")
                suffix += 1
        out = sqlite3.connect(str(backup))
        try:
            self.db.backup(out)
        finally:
            out.close()
        os.chmod(backup, 0o600)

        existing = self._columns()
        with self._tx():
            for name, typ in additions:
                if name not in existing:
                    self.db.execute(f"ALTER TABLE threads ADD COLUMN {name} {typ}")
            self._append_missing_auxiliary_columns()
            self.meta_set("schema_version", str(SCHEMA_VERSION))
            self.meta_set(f"schema_v{from_version}_backup", str(backup))

    def _append_missing_auxiliary_columns(self):
        """Bring `updates`/`workers` forward the same way `threads` is brought forward.

        `_validate_auxiliary_schema` now admits a historical database whose auxiliary tables are
        a PREFIX of the current shape; this is the other half of that bargain. Without it such a
        database would be stamped current while still missing columns, and the next open — which
        demands an exact signature at the current version — would reject it. Accepting a shape
        without being able to complete it would trade an unopenable database for one that opens
        exactly once.

        DDL is rebuilt from `_SCHEMA` rather than written out again, so there is one statement of
        each column's type. A NOT NULL column with no default cannot be added to a populated
        table, and SQLite says so; that is left to fail loudly rather than be guessed at, because
        inventing a default is inventing data.
        """
        expected = _expected_signatures()
        for table in ("updates", "workers"):
            signature = _signature_on(self.db, table)
            if not signature:
                # ABSENT, not incomplete — an empty signature is SQLite's answer for "no such
                # table". A v1 predates `workers` entirely, and `executescript(_SCHEMA)` creates
                # it whole immediately after this runs. ALTERing it here is what that ordering
                # made impossible, not something to work around.
                continue
            have = {name for name, *_ in signature}
            for name, typ, notnull, default, _pk in expected[table]:
                if name in have:
                    continue
                clause = f"{name} {typ}"
                if default is not None:
                    clause += f" DEFAULT {default}"
                if notnull:
                    clause += " NOT NULL"
                self.db.execute(f"ALTER TABLE {table} ADD COLUMN {clause}")

    def close(self):
        try:
            with self._lock:
                if self.db is not None:
                    self.db.close()
        except sqlite3.Error:
            pass
        if self._snapshot_holder is not None:
            self._snapshot_holder.cleanup()
            self._snapshot_holder = None

    # ── meta / cursor ─────────────────────────────────────────────────────────────────
    def meta_get(self, key, default=None):
        with self._lock:
            row = self.db.execute("SELECT value FROM meta WHERE key = ?", (key,)).fetchone()
        return row["value"] if row else default

    def meta_set(self, key, value):
        with self._lock:
            self.db.execute("INSERT INTO meta(key, value) VALUES(?, ?) "
                            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                            (key, None if value is None else str(value)))

    @property
    def cursor(self):
        """The next Telegram offset to request, or None before the first update.

        Persisted per UPDATE rather than per batch. The old loop advanced an in-memory offset
        and only communicated it on the next poll, so nothing in a batch was confirmed until
        the whole batch finished — and a crash replayed all of it."""
        return _as_offset(self.meta_get("cursor"))

    def mark_cursor_acked(self):
        """Telegram has been told an offset past everything below the cursor, so it will not
        redeliver those updates. This is what makes compaction safe and what turns DELIVERED
        into ACKED."""
        cur = self.cursor
        if cur is None:
            return 0
        with self._tx():
            self.meta_set("cursor_acked", cur)
            n = self.db.execute(
                "UPDATE updates SET state = ?, updated_at = ? "
                "WHERE update_id < ? AND state = ?",
                (ACKED, _now(), cur, DELIVERED)).rowcount
        return n

    @property
    def cursor_acked(self):
        return _as_offset(self.meta_get("cursor_acked"))

    # ── transactions ──────────────────────────────────────────────────────────────────
    def _tx(self):
        state = self

        class _Tx:
            def __enter__(self):
                state._lock.acquire()
                try:
                    state.db.execute("BEGIN IMMEDIATE")
                    return state
                except BaseException:
                    state._lock.release()
                    raise

            def __exit__(self, exc_type, *_):
                try:
                    state.db.execute("ROLLBACK" if exc_type else "COMMIT")
                finally:
                    state._lock.release()
                return False
        return _Tx()

    # ── threads ───────────────────────────────────────────────────────────────────────
    def thread_row(self, gid):
        with self._lock:
            return self.db.execute(
                "SELECT * FROM threads WHERE gid = ?", (str(gid),)).fetchone()

    def all_thread_rows(self):
        with self._lock:
            return self.db.execute("SELECT * FROM threads ORDER BY gid").fetchall()

    @staticmethod
    def stored_identity(row):
        return {k: row[k] for k in IDENTITY_FIELDS}

    @staticmethod
    def intended_identity(client):
        return dict(client.thread_identity)

    @staticmethod
    def thread_has_state(row):
        """Whether a row carries anything that a silent rebind could discard or reinterpret."""
        return bool(row["prev"] or json.loads(row["supplied"] or "{}")
                    or row["ever_supplied"] or row["last_turn_at"]
                    or json.loads(row["orphans"] or "{}"))

    @staticmethod
    def identity_mismatches(row, client):
        stored, intended = BridgeState.stored_identity(row), client.thread_identity
        missing = [k for k in IDENTITY_FIELDS if stored[k] is None]
        if missing:
            return ["legacy compatibility identity missing"]
        return [IDENTITY_LABELS[k] for k in IDENTITY_FIELDS if stored[k] != intended[k]]

    def bind_thread_identity(self, gid, client):
        """Bind an existing empty legacy row without inventing prior conversation identity."""
        ident = self.intended_identity(client)
        with self._tx():
            self.db.execute(
                "UPDATE threads SET service=?, service_version=?, instructions_sha256=?, "
                "model=?, context_policy_sha256=?, organization_id=?, "
                "account_service_base=? WHERE gid=?",
                (*(ident[k] for k in IDENTITY_FIELDS), str(gid)))

    def _write_thread(self, gid, thread):
        ident = self.intended_identity(thread.client)
        self.db.execute(
            "INSERT INTO threads(gid, prev, supplied, ever_supplied, last_turn_at, orphans, "
            "service, service_version, instructions_sha256, model, context_policy_sha256, "
            "organization_id, account_service_base) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(gid) DO UPDATE SET "
            "prev=excluded.prev, supplied=excluded.supplied, "
            "ever_supplied=excluded.ever_supplied, last_turn_at=excluded.last_turn_at, "
            "orphans=excluded.orphans, service=excluded.service, "
            "service_version=excluded.service_version, "
            "instructions_sha256=excluded.instructions_sha256, model=excluded.model, "
            "context_policy_sha256=excluded.context_policy_sha256, "
            "organization_id=excluded.organization_id, "
            "account_service_base=excluded.account_service_base",
            (str(gid), thread.prev,
             json.dumps({k: thread.supplied[k] for k in sorted(thread.supplied)}),
             1 if thread.ever_supplied else 0, thread.last_turn_at,
             json.dumps({k: list(v) for k, v in sorted(thread.orphans.items())}),
             *(ident[k] for k in IDENTITY_FIELDS)))

    def drop_thread(self, gid):
        """Deprovisioning deletes a tenant's conversation state. Explicit, never implied by
        absence from the registry — an operator moving an env aside must not lose a group.

        Deletes the delivery journal too, because deprovisioning removes the tenant entirely.
        When you want a fresh conversation but need the journal kept as evidence, that is
        `reset_thread` — reaching for this one instead destroys the record of what was
        delivered, which is the thing an operator is most likely to need afterwards."""
        with self._tx():
            n = self.db.execute("DELETE FROM threads WHERE gid = ?", (str(gid),)).rowcount
            self.db.execute("DELETE FROM updates WHERE gid = ?", (str(gid),))
        return n

    def reset_thread(self, gid):
        """Start this group's conversation over, KEEPING the delivery journal.

        The narrow half of `drop_thread`: forget the continuity pointer and what context was
        supplied, so the next turn begins as a first contact with records freshly injected —
        while every `updates` row survives as evidence of what was actually delivered.

        This exists because the two were only available together. Testing a persona change
        against a clean thread meant either destroying the delivery record or editing the
        store by hand, and both were done by hand here before this method existed.

        NOT SAFE WHILE THE BRIDGE IS RUNNING: it caches `Thread` objects in memory and would
        write its stale copy back on the next turn. Stop the bridge, reset, then start it —
        and wait for the old process to exit first, because one bot token allows one poller
        and two overlapping pollers steal each other's updates.

        Returns the number of thread rows removed (0 if the group had never spoken)."""
        with self._tx():
            return self.db.execute("DELETE FROM threads WHERE gid = ?", (str(gid),)).rowcount

    # ── updates ───────────────────────────────────────────────────────────────────────
    def update_row(self, update_id):
        with self._lock:
            return self.db.execute("SELECT * FROM updates WHERE update_id = ?",
                                   (int(update_id),)).fetchone()

    def advance_safe_cursor(self):
        """Acknowledge only rows before the earliest unfinished update.

        Workers may finish out of order. All updates in a fetched batch are inserted before
        any worker starts, so the earliest non-final row is a durable acknowledgement barrier.
        """
        with self._tx():
            return self._advance_safe_cursor_locked()

    def _advance_safe_cursor_locked(self):
        safe = (*TERMINAL, DELIVERED)
        pending = self.db.execute(
            "SELECT MIN(update_id) AS update_id FROM updates WHERE state NOT IN ("
            + ",".join("?" * len(safe)) + ")", safe).fetchone()["update_id"]
        if pending is not None:
            candidate = int(pending)
        else:
            row = self.db.execute("SELECT MAX(update_id) AS update_id FROM updates").fetchone()
            if row["update_id"] is None:
                return self.cursor
            candidate = int(row["update_id"]) + 1
        current = self.cursor
        if current is None or candidate > current:
            self.meta_set("cursor", candidate)
        return self.cursor

    def note_received(self, update_id, gid, message_id=None):
        """First durable fact about an update, written BEFORE any work. Idempotent: a
        redelivered update keeps its original row and its original state."""
        with self._tx():
            self.db.execute(
                "INSERT INTO updates(update_id, gid, state, first_seen, updated_at, message_id) "
                "VALUES(?,?,?,?,?,?) ON CONFLICT(update_id) DO NOTHING",
                (int(update_id), None if gid is None else str(gid), RECEIVED,
                 _now(), _now(), message_id))
        return self.update_row(update_id)

    def note_turn_started(self, update_id, gid, idempotency_key, prev_before):
        """The key is recorded BEFORE the request leaves the process. It is the only handle
        that exists before a reply we might never receive."""
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, gid=?, idempotency_key=?, prev_before=?, "
                "attempts=attempts+1, updated_at=? WHERE update_id=?",
                (TURN_STARTED, str(gid), idempotency_key, prev_before, _now(), int(update_id)))

    def commit_turn(self, update_id, gid, thread):
        """THE CRITICAL TRANSACTION.

        A model response completed; the thread pointer it produced became authoritative; and
        the bridge recorded how to deliver that exact result. All three, or none. This is the
        single write that two separate files could not make atomic, and the reason this store
        exists.

        NO SEPARATE `response_id` ARGUMENT. There was one, and the only production caller passed
        `thread.prev` into it — `self.state.commit_turn(uid, thread.prev, gid, thread)` — so the
        two columns this writes were guaranteed to hold the same string on every real turn, while
        the signature offered a caller the chance to make them differ. They cannot differ and mean
        anything: the thread pointer after a turn IS the id of the response that turn produced.
        One fact, taken once.

        `prev_after` stays in the schema rather than being dropped with the parameter. Nothing
        reads it (nor `prev_before`, written by `note_turn_started`) — they are a forensic record
        of where the pointer stood either side of a turn, which is exactly what an operator wants
        after a crash and exactly what no code should branch on. Removing a column is a
        SCHEMA_VERSION bump and a migration; removing a misleading parameter is neither."""
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, response_id=?, prev_after=?, updated_at=? "
                "WHERE update_id=?",
                (TURN_COMPLETED, thread.prev, thread.prev, _now(), int(update_id)))
            self._write_thread(gid, thread)

    def note_state(self, update_id, state, error_code=None):
        """Move a row's state. `error_code=None` KEEPS whatever is recorded; a value REPLACES it.

        The replace half is sharp: for a DELIVERY_RETRY / DELIVERY_RECONCILE row the error_code
        is the delivery evidence (`delivery_partial` vs `delivery_uncertain`), which says whether
        a retry will duplicate content in a client group. Overwriting it with the reason for a
        FAILED retry destroys the input to the next one. Pass None unless the new value is
        strictly more informative than the old."""
        if state not in ALL_STATES:
            raise StateError(f"unknown update state {state!r}")
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, updated_at=?, "
                "error_code=COALESCE(?, error_code) WHERE update_id=?",
                (state, _now(), error_code, int(update_id)))

    def note_delivered(self, update_id):
        """Delivery succeeded and the next safe offset is durable — one transaction, because a
        crash between them is exactly how an already-answered update gets replayed.

        NO `next_offset` PARAMETER, and there used to be one. Both this and `note_terminal` took
        the caller's idea of the next offset and neither body ever read it: the durable cursor is
        computed here by `_advance_safe_cursor_locked`, from the rows. `bridge_core` computed
        `nxt = uid + 1` for the sole purpose of passing it, through three method signatures, to
        eleven call sites. A reader of `_deliver` reasonably concluded the caller decided the
        cursor — on the most load-bearing transaction in the tree — and the signature was the
        only reason to think so."""
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, delivered_at=?, updated_at=?, error_code=NULL "
                "WHERE update_id=?",
                (DELIVERED, _now(), _now(), int(update_id)))
            self._advance_safe_cursor_locked()

    def note_reconciled_delivered(self, update_id):
        """Record explicit redelivery without moving the global cursor backward."""
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, delivered_at=?, updated_at=?, error_code=NULL "
                "WHERE update_id=? AND state IN (?, ?)",
                (ACKED, _now(), _now(), int(update_id), DELIVERY_RETRY, DELIVERY_RECONCILE))

    def note_terminal(self, update_id, state, error_code=None):
        """A terminal outcome that still lets the loop move on: the client was told, the
        operator can see it, and the offset advances so it is never replayed."""
        if state not in TERMINAL:
            raise StateError(f"{state!r} is not terminal")
        with self._tx():
            self.db.execute(
                "UPDATE updates SET state=?, updated_at=?, "
                "error_code=COALESCE(?, error_code) WHERE update_id=?",
                (state, _now(), error_code, int(update_id)))
            self._advance_safe_cursor_locked()

    def counts_by_state(self):
        with self._lock:
            return {r["state"]: r["n"] for r in self.db.execute(
                "SELECT state, count(*) AS n FROM updates GROUP BY state")}

    def blocked_updates(self, limit=50):
        with self._lock:
            return self.db.execute(
                "SELECT update_id, gid, state, response_id, error_code, updated_at FROM updates "
                "WHERE state IN (?, ?, ?, ?) ORDER BY update_id DESC LIMIT ?",
                (RECOVERY_BLOCKED, DELIVERY_RETRY, DELIVERY_RECONCILE,
                 FAILED_TERMINAL, limit)).fetchall()

    def note_worker(self, gid, update_id, stage, started_at=None, deadline_at=None,
                    heartbeat_at=None):
        with self._tx():
            self.db.execute(
                "INSERT INTO workers(gid, update_id, stage, started_at, deadline_at, heartbeat_at) "
                "VALUES(?,?,?,?,?,?) ON CONFLICT(gid) DO UPDATE SET "
                "update_id=excluded.update_id, stage=excluded.stage, "
                "started_at=COALESCE(excluded.started_at, workers.started_at), "
                "deadline_at=COALESCE(excluded.deadline_at, workers.deadline_at), "
                "heartbeat_at=excluded.heartbeat_at",
                (str(gid), int(update_id), stage, started_at, deadline_at,
                 heartbeat_at or _now()))

    def clear_worker(self, gid):
        with self._tx():
            self.db.execute("DELETE FROM workers WHERE gid = ?", (str(gid),))

    def clear_workers(self):
        """Discard progress rows left by a prior process; journal rows remain authoritative."""
        with self._tx():
            self.db.execute("DELETE FROM workers")

    def active_workers(self):
        with self._lock:
            return [dict(r) for r in self.db.execute(
                "SELECT * FROM workers ORDER BY update_id").fetchall()]

    # ── compaction ────────────────────────────────────────────────────────────────────
    def compact(self, retain=RETAIN_TERMINAL):
        """Drop terminal rows Telegram can no longer redeliver.

        Bounded by `cursor_acked`, not by `cursor`: an update below the cursor has been
        answered locally, but until a `getUpdates` carrying that offset has actually succeeded,
        Telegram may still hand it back — and a compacted row is one the bridge would not
        recognise, which is the duplicate-delivery defect reintroduced by tidiness."""
        acked = self.cursor_acked
        if acked is None:
            return 0
        with self._tx():
            keep_above = self.db.execute(
                "SELECT update_id FROM updates WHERE update_id < ? ORDER BY update_id DESC "
                "LIMIT 1 OFFSET ?", (acked, retain)).fetchone()
            if keep_above is None:
                return 0
            n = self.db.execute(
                "DELETE FROM updates WHERE update_id <= ? AND state IN "
                "(" + ",".join("?" * len(COMPACTABLE_TERMINAL)) + ")",
                (keep_above["update_id"], *COMPACTABLE_TERMINAL)).rowcount
        return n

    # ── operator view ─────────────────────────────────────────────────────────────────
    def progress_snapshot(self):
        """Non-secret liveness + PROGRESS facts for the watchdog and the console.

        Deliberately not a heartbeat alone: a heartbeat proves a process exists, which is what
        `systemctl is-active` already proved, and which stayed true while the old loop sat
        wedged inside a fifteen-minute turn."""
        # ONE read of `meta`, not one per key. This was sixteen separate SELECTs, each taking
        # the process-wide lock on its own, against a table with a handful of rows — and the
        # watchdog now calls this on every tick, beside every `doctor` and `bridge status`.
        with self._lock:
            meta = {r["key"]: r["value"] for r in
                    self.db.execute("SELECT key, value FROM meta")}
        snap = {k: meta.get(k) for k in (
            "heartbeat_at", "last_poll_ok_at", "last_update_at", "last_delivered_at",
            "inflight_update_id", "inflight_gid", "inflight_stage", "inflight_started_at",
            "inflight_deadline_at", "last_batch_size", "started_at", "pid", "last_outcome")}
        # Coerced exactly as the `cursor` / `cursor_acked` properties do — the console prints
        # these and an offset that arrived as a string would compare wrong against an int.
        snap["cursor"] = _as_offset(meta.get("cursor"))
        snap["cursor_acked"] = _as_offset(meta.get("cursor_acked"))
        snap["counts"] = self.counts_by_state()
        snap["workers"] = self.active_workers()
        snap["schema_version"] = meta.get("schema_version")
        return snap

    def note_progress(self, **fields):
        """Record one or more progress facts. Cheap and frequent; never inside a turn's
        critical transaction, so a progress write can never fail a delivery."""
        with self._tx():
            for k, v in fields.items():
                self.meta_set(k, v)


# ── migration from the JSON thread file ───────────────────────────────────────────────

def migrate_from_json(json_path, db_path=None, delete_source=False):
    """One-time, explicit migration of `bridge-threads.json` into the store.

    REFUSES the pre-versioning shape exactly as `_load_threads` did, and for the same reason:
    coercing a list-shaped `supplied` derives `ever_supplied=false` for a thread that HAS had
    context, which trips the starvation recovery and nulls `thread.prev` — silently discarding
    a live group's conversation to save one migration. Failing loudly costs a restart.
    """
    src = pathlib.Path(json_path)
    st = BridgeState(db_path)
    if st.meta_get("migrated_from_json"):
        return st, 0
    try:
        saved = json.loads(src.read_text())
    except FileNotFoundError:
        st.meta_set("migrated_from_json", _now())      # nothing to carry: a fresh install
        return st, 0
    except ValueError as e:
        raise StateError(f"{src} is not readable JSON ({e}). Fix or move it aside; do not "
                         "delete it — it carries every group's conversation pointer.") from e

    migrated = 0
    with st._tx():
        for gid, entry in saved.items():
            if not isinstance(entry, dict):
                continue
            sup = entry.get("supplied", {})
            if not isinstance(sup, dict):
                raise LegacyStateError(
                    f"{src}: group {gid} has a pre-versioning 'supplied' list. Migrate that "
                    "file once with the migration printed here — the operator procedure is in "
                    "deploy/UPGRADE.md, under the bridge step of the bump — then start "
                    "again — this store will pick it up. Do NOT delete the file to clear "
                    "this: that resets every group's `prev`, which is the outcome the "
                    "refusal exists to prevent.")
            orphans = {k: list(v) for k, v in (entry.get("orphans") or {}).items()
                       if isinstance(v, (list, tuple)) and len(v) == 2}
            st.db.execute(
                "INSERT INTO threads(gid, prev, supplied, ever_supplied, last_turn_at, orphans) "
                "VALUES(?,?,?,?,?,?) ON CONFLICT(gid) DO NOTHING",
                (str(gid), entry.get("prev"), json.dumps(sup),
                 1 if entry.get("ever_supplied", bool(sup)) else 0,
                 entry.get("last_turn_at"), json.dumps(orphans)))
            migrated += 1
        st.meta_set("migrated_from_json", _now())
        st.meta_set("migrated_source", str(src))
    # The source is KEPT by default. A migration that deletes its own input leaves an operator
    # who hits a problem five minutes later with nothing to go back to.
    if delete_source and migrated:
        src.rename(src.with_suffix(".json.migrated-" + str(int(time.time()))))
    return st, migrated
