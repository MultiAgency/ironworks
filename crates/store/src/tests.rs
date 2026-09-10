use super::*;

const SCHEMA: Schema = Schema {
    sql: "CREATE TABLE IF NOT EXISTS thing_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS thing(id TEXT PRIMARY KEY);",
    meta_table: "thing_meta",
    version: 3,
};

/// A directory no other test in this process can be handed. A counter rather than a clock:
/// `SystemTime` is coarser than its unit, so two tests running in parallel can land in one bucket.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ironworks-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }
    fn db(&self) -> std::path::PathBuf {
        self.0.join("thing.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn refuse_every_migration(_: &rusqlite::Transaction<'_>, _: i64) -> Result<bool, rusqlite::Error> {
    Ok(false)
}

#[test]
fn a_fresh_store_records_the_version_and_reopens() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    // The positive control every refusal below rests on: an untouched store reopens.
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("reopen"));

    let conn = Connection::open(dir.db()).expect("read back");
    let recorded: String = conn
        .query_row("SELECT value FROM thing_meta WHERE key='schema_version'", [], |r| r.get(0))
        .expect("a version was recorded");
    assert_eq!(recorded, "3");
}

/// The defect this crate exists to stop happening in three places at once.
#[test]
fn a_damaged_version_is_refused_rather_than_read_as_absent() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("UPDATE thing_meta SET value='three' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    match open_durable(&dir.db(), SCHEMA, refuse_every_migration) {
        Err(OpenFailure::UnreadableVersion { raw }) => assert_eq!(raw, "three"),
        other => panic!("a damaged version must be refused, got {other:?}"),
    }
}

#[test]
fn an_unknown_version_no_migration_understands_is_refused_with_what_was_found() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("UPDATE thing_meta SET value='99' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    match open_durable(&dir.db(), SCHEMA, refuse_every_migration) {
        Err(OpenFailure::Version { found }) => assert_eq!(found, 99),
        other => panic!("an unknown version must be refused, got {other:?}"),
    }
}

/// A migration that succeeds leaves the version current, and the next open runs no migration at
/// all. This migration does no DDL, so it cannot say what an interrupted one leaves behind — that
/// is `a_migration_that_fails_after_its_ddl_leaves_neither_the_column_nor_a_new_version`'s.
#[test]
fn a_migration_that_succeeds_is_run_once_and_leaves_the_version_current() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("UPDATE thing_meta SET value='2' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    let ran = std::cell::Cell::new(0);
    drop(
        open_durable(&dir.db(), SCHEMA, |_, found| {
            ran.set(ran.get() + 1);
            Ok(found == 2)
        })
        .expect("a migration that reports success must open"),
    );
    assert_eq!(ran.get(), 1);

    let ran_again = std::cell::Cell::new(0);
    drop(
        open_durable(&dir.db(), SCHEMA, |_, _| {
            ran_again.set(ran_again.get() + 1);
            Ok(false)
        })
        .expect("the migrated store is current"),
    );
    assert_eq!(ran_again.get(), 0, "a current version must not be offered to the migration");
}

/// Whether the migration below has left its one visible effect.
fn has_extra_column(path: &std::path::Path) -> bool {
    let conn = Connection::open(path).expect("read back");
    conn.prepare("SELECT 1 FROM pragma_table_info('thing') WHERE name='extra'")
        .expect("prepare")
        .exists([])
        .expect("probe")
}

fn recorded_version_of(path: &std::path::Path) -> String {
    let conn = Connection::open(path).expect("read back");
    conn.query_row("SELECT value FROM thing_meta WHERE key='schema_version'", [], |r| r.get(0))
        .expect("a version was recorded")
}

/// The migration and the version it earns land together or not at all.
///
/// Both callers used to commit their own `ALTER TABLE` and leave the version to a later statement,
/// so a failure between the two left the column added and the old version recorded — and every
/// later open re-ran the `ALTER` into `duplicate column name`, for good.
#[test]
fn a_migration_that_fails_after_its_ddl_leaves_neither_the_column_nor_a_new_version() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("UPDATE thing_meta SET value='2' WHERE key='schema_version'", []).unwrap();
    drop(conn);

    let failed = open_durable(&dir.db(), SCHEMA, |tx, found| {
        assert_eq!(found, 2);
        tx.execute_batch("ALTER TABLE thing ADD COLUMN extra TEXT")?;
        Err(rusqlite::Error::InvalidQuery)
    });
    assert!(matches!(failed, Err(OpenFailure::Sqlite(rusqlite::Error::InvalidQuery))));
    assert!(!has_extra_column(&dir.db()), "the DDL must roll back with the failure");
    assert_eq!(recorded_version_of(&dir.db()), "2");

    // Not wedged: the same migration, succeeding, runs cleanly on the file the failure left.
    drop(
        open_durable(&dir.db(), SCHEMA, |tx, found| {
            tx.execute_batch("ALTER TABLE thing ADD COLUMN extra TEXT")?;
            Ok(found == 2)
        })
        .expect("a clean retry migrates"),
    );
    // The positive control for the absence above: the same probe sees the column once it is there.
    assert!(has_extra_column(&dir.db()));
    assert_eq!(recorded_version_of(&dir.db()), "3");
}

#[test]
fn a_file_that_is_not_a_database_says_so() {
    let dir = Scratch::new();
    std::fs::write(dir.db(), b"this is not a database\n").expect("write");
    match open_durable(&dir.db(), SCHEMA, refuse_every_migration) {
        Err(OpenFailure::NotADatabase) => {}
        other => panic!("expected NotADatabase, got {other:?}"),
    }
}

#[test]
fn a_store_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    let mode = std::fs::metadata(dir.db()).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "a store names organizations and must not be group- or world-readable");
}

#[test]
fn read_only_refuses_an_unversioned_or_stale_file_and_accepts_a_current_one() {
    let dir = Scratch::new();
    drop(open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open"));
    drop(open_readonly(&dir.db(), SCHEMA).expect("a current store inspects"));

    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("UPDATE thing_meta SET value='2' WHERE key='schema_version'", []).unwrap();
    drop(conn);
    match open_readonly(&dir.db(), SCHEMA) {
        Err(OpenFailure::Version { found }) => assert_eq!(found, 2),
        other => panic!("a stale store must not inspect, got {other:?}"),
    }

    let conn = Connection::open(dir.db()).expect("open");
    conn.execute("DELETE FROM thing_meta WHERE key='schema_version'", []).unwrap();
    drop(conn);
    match open_readonly(&dir.db(), SCHEMA) {
        Err(OpenFailure::UnreadableVersion { raw }) => assert!(raw.is_empty()),
        other => panic!("an unversioned store must not inspect, got {other:?}"),
    }
}

#[test]
fn a_snapshot_is_owner_only_and_never_overwrites() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new();
    let live = open_durable(&dir.db(), SCHEMA, refuse_every_migration).expect("fresh open");
    live.execute("INSERT INTO thing(id) VALUES('a')", []).unwrap();

    let copy = dir.0.join("copy.db");
    snapshot_to(&live, &copy).expect("snapshot");
    let mode = std::fs::metadata(&copy).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let read = Connection::open(&copy).expect("open copy");
    let rows: i64 = read.query_row("SELECT count(*) FROM thing", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "the snapshot must carry the rows, not just the schema");

    match snapshot_to(&live, &copy) {
        Err(SnapshotFailure::Reserve(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        }
        other => panic!("a snapshot must never overwrite, got {other:?}"),
    }
}
