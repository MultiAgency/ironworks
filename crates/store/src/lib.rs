//! The SQLite open protocol, once.
//!
//! # What was here three times
//!
//! `ironworks-journal`, `ironworks-proposals` and `ironworks-channel` each keep one durable local
//! file, and each opened it by the same eight steps in the same order: `Connection::open`,
//! restrict to owner, `journal_mode = WAL`, `synchronous = FULL`, create the schema, read the
//! recorded version, migrate or refuse, hand back the connection. The `opened` helper that turns a
//! first-statement failure into "this file is not a database" was copied character-for-character,
//! doc comment included.
//!
//! The copies had already drifted on the one question that matters. Two of the three read a
//! damaged `schema_version` as *absent* — and the absent arm stamps the implementation's own
//! version onto a database whose shape was never established. The third refused, but reported
//! `found: -1`, a number that was never on disk.
//!
//! # What stays with each crate, and why
//!
//! The schema, the meta table's name, the version, the migration — and **every message**. A
//! refusal here is read by an operator deciding whether to move a file aside, and the three files
//! are different things to lose: a record of which turns ran and which answers were billed; words
//! someone typed and is waiting to confirm; how far a channel has been acknowledged. Sharing the
//! *mechanism* is the point. Sharing the sentences would be a regression.
//!
//! So this crate's [`OpenFailure`] carries no path and no prose. Each caller maps it into its own
//! error enum, which is a ten-line `match` it can read, and keeps the wording it had.

use ironworks_domain::restrict_to_owner;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// What went wrong opening a store, as a cause rather than a sentence.
///
/// No `path` field: the caller has the path, and the sentence it builds is its own.
#[derive(Debug)]
pub enum OpenFailure {
    /// The file could not be opened at all.
    Open(rusqlite::Error),
    /// It opened, and the first statement says its bytes are not a database.
    NotADatabase,
    /// It was created but could not be restricted to its owner.
    Exposed(std::io::Error),
    /// A `schema_version` row that is not a number.
    ///
    /// **Three-valued deliberately.** Folding this into "no version recorded" is what let two of
    /// the three copies stamp a current version onto an unknown schema.
    UnreadableVersion { raw: String },
    /// A version this implementation does not write and the caller could not migrate.
    Version { found: i64 },
    /// Anything else SQLite refused.
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for OpenFailure {
    fn from(source: rusqlite::Error) -> Self {
        Self::Sqlite(source)
    }
}

/// What one store's file is, as far as this crate needs to know.
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    /// The `CREATE TABLE IF NOT EXISTS …` batch, run on every open.
    pub sql: &'static str,
    /// The key/value table holding the `schema_version` row.
    pub meta_table: &'static str,
    /// The version this implementation writes.
    pub version: i64,
}

/// Open a store owner-only and durable, create its schema, and settle its version.
///
/// `migrate` is asked about a version that is neither absent nor current. It returns `Ok(true)`
/// when it has brought the file up to `schema.version`, and `Ok(false)` when it does not know that
/// version — which becomes [`OpenFailure::Version`]. **A store with nothing to migrate passes
/// `|_, _| Ok(false)`**, which reads as "no version but mine is acceptable" rather than as an
/// omission.
///
/// **The migration and the version it earns are one transaction, and this function owns it.**
/// `migrate` is lent the [`Transaction`](rusqlite::Transaction) and cannot commit it — `commit`
/// takes it by value — so its DDL and the new version land together or not at all. Both callers
/// used to commit their own `ALTER TABLE` and leave the version to a second statement here, and a
/// crash between the two left the column added and the old version recorded: every later open
/// re-ran the `ALTER` into `duplicate column name`, for good. The type stops `commit`; it does not
/// stop a literal `COMMIT` in the migration's own SQL.
pub fn open_durable(
    path: &Path,
    schema: Schema,
    migrate: impl FnOnce(&rusqlite::Transaction<'_>, i64) -> Result<bool, rusqlite::Error>,
) -> Result<Connection, OpenFailure> {
    let mut conn = Connection::open(path).map_err(OpenFailure::Open)?;
    // Before the schema, so a file that is both readable and non-empty never exists.
    restrict_to_owner(path).map_err(OpenFailure::Exposed)?;
    // The first statement is where a file that is not a database says so: `open` itself is lazy
    // and succeeds against any path it can touch.
    conn.pragma_update(None, "journal_mode", "WAL").map_err(not_a_database)?;
    // A crash test that survives fsync is the only kind worth writing. The whole point of these
    // files is to be true after a power cut rather than merely after a clean exit, and `FULL` is
    // the setting that buys that.
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.execute_batch(schema.sql)?;

    // Deferred: opening a current store only reads, and must not take a write lock it does not
    // need. The version is read inside it, so the one a migration is offered is the one its commit
    // replaces. Every early return drops `tx`, which rolls back.
    let tx = conn.transaction()?;
    match recorded_version(&tx, schema.meta_table)? {
        None => {
            tx.execute(
                &format!(
                    "INSERT INTO {}(key,value) VALUES('schema_version',?1)",
                    schema.meta_table
                ),
                [schema.version.to_string()],
            )?;
        }
        Some(found) if found == schema.version => {}
        Some(found) => {
            if !migrate(&tx, found)? {
                return Err(OpenFailure::Version { found });
            }
            tx.execute(
                &format!("UPDATE {} SET value=?1 WHERE key='schema_version'", schema.meta_table),
                [schema.version.to_string()],
            )?;
        }
    }
    tx.commit()?;
    Ok(conn)
}

/// Open a store read-only and refuse anything but the current version.
///
/// Inspection must reject a file this binary would refuse to serve. Without the version check an
/// old schema that happens to retain the columns being read could look healthy while carrying
/// semantics this binary no longer understands. An *absent* version is refused here rather than
/// written, because a read-only open writes nothing — including a version.
pub fn open_readonly(path: &Path, schema: Schema) -> Result<Connection, OpenFailure> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(OpenFailure::Open)?;
    match recorded_version(&conn, schema.meta_table)? {
        Some(found) if found == schema.version => Ok(conn),
        Some(found) => Err(OpenFailure::Version { found }),
        // This used to report `found: -1`, a number that was never on disk. `UnreadableVersion`
        // with an empty raw says what actually happened: nothing recorded a version at all.
        None => Err(OpenFailure::UnreadableVersion { raw: String::new() }),
    }
}

/// Copy a transactionally consistent view into a new owner-only file.
///
/// `create_new` is the reservation: a snapshot never overwrites, so an operator cannot lose the
/// previous one by repeating a command.
pub fn snapshot_to(source: &Connection, destination: &Path) -> Result<(), SnapshotFailure> {
    let reservation = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(SnapshotFailure::Reserve)?;
    drop(reservation);
    restrict_to_owner(destination).map_err(SnapshotFailure::Reserve)?;
    let mut target = Connection::open(destination).map_err(SnapshotFailure::Sqlite)?;
    let backup =
        rusqlite::backup::Backup::new(source, &mut target).map_err(SnapshotFailure::Sqlite)?;
    backup
        .run_to_completion(128, std::time::Duration::from_millis(1), None)
        .map_err(SnapshotFailure::Sqlite)?;
    drop(backup);
    target.close().map_err(|(_, source)| SnapshotFailure::Sqlite(source))
}

/// What went wrong taking a snapshot, as a cause rather than a sentence.
#[derive(Debug)]
pub enum SnapshotFailure {
    /// The destination could not be created, or could not be restricted to its owner.
    Reserve(std::io::Error),
    /// SQLite refused the copy.
    Sqlite(rusqlite::Error),
}

/// The recorded schema version: absent, the number, or a refusal.
///
/// `crates/channel/src/lib.rs::parse_acked` refuses for exactly this reason one field over: *"a
/// damaged file is precisely the case a crate running `synchronous = FULL` exists to survive, so
/// it is reported rather than absorbed."*
fn recorded_version(conn: &Connection, meta_table: &str) -> Result<Option<i64>, OpenFailure> {
    let raw: Option<String> = conn
        .query_row(&format!("SELECT value FROM {meta_table} WHERE key='schema_version'"), [], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(not_a_database)?;
    match raw {
        None => Ok(None),
        Some(raw) => raw.parse().map(Some).map_err(|_| OpenFailure::UnreadableVersion { raw }),
    }
}

/// A file that is not a database says so on the first statement, and nowhere earlier.
fn not_a_database(source: rusqlite::Error) -> OpenFailure {
    if matches!(&source, rusqlite::Error::SqliteFailure(e, _)
        if e.code == rusqlite::ErrorCode::NotADatabase)
    {
        return OpenFailure::NotADatabase;
    }
    OpenFailure::Open(source)
}

#[cfg(test)]
mod tests;
