//! Who, besides this process, can read a durable store.
//!
//! # Why this is domain code and not an implementation detail of each store
//!
//! Three crates create SQLite files that outlive the process: the turn journal, the channel
//! journal, and the lifecycle store. None of them holds message text, answer text, records, or
//! credentials — that is enforced elsewhere and tested by grepping a populated store for fixture
//! strings. What they *do* hold is the shape of the operation: which organizations exist, which
//! channels they are reached on, which turns are outstanding, and which idempotency keys were
//! minted. A reader who cannot see a single sentence of a conversation can still learn the whole
//! client list from a world-readable file.
//!
//! So the mode belongs with the other confidentiality decisions in this crate rather than
//! duplicated three times as a filesystem chore. The reference implementation asserts `0600` on
//! its own store and this is the same guarantee, stated once.
//!
//! # What this does not promise
//!
//! It restricts **the file it is given**. SQLite in WAL mode also creates `-wal` and `-shm`
//! siblings, and pages that have not been checkpointed live in the `-wal` file — so a claim about
//! the main database alone would be a claim about the wrong file. Measured rather than assumed:
//! SQLite creates both siblings with the mode of the database it opened, so restricting one
//! restricts all three. `ironworks-channel` pins that, because it is a property of the database
//! library rather than of this function, and it would go unnoticed if it changed.
//!
//! It also cannot close the window before it runs. A store is created by the database library and
//! restricted immediately afterwards, so a file briefly exists at the process umask — empty, with
//! no schema and no rows. Narrowing that further would mean owning file creation, which would
//! mean not using the library's opener.

use std::io;
use std::path::Path;

/// Restrict a durable store to its owner. Call it immediately after creation and before anything
/// is written, so the window in which the file is both readable and non-empty does not exist.
///
/// A failure here is a real failure and must not be swallowed. A store that could not be
/// restricted is a store whose confidentiality was not established, and reporting success would
/// be a claim about something never measured.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).expect("stat").permissions().mode() & 0o777
    }

    /// The positive case, written first: a file the umask left group- and world-readable comes
    /// back owner-only. Without this, a test asserting `0600` could pass on a machine whose umask
    /// already produced it, and would be measuring the umask rather than this function.
    #[cfg(unix)]
    #[test]
    fn an_exposed_file_becomes_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("custody-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let p = dir.join("store.db");
        std::fs::write(&p, b"x").expect("write");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(mode_of(&p), 0o644, "the control did not take");

        restrict_to_owner(&p).expect("restrict");
        assert_eq!(mode_of(&p), 0o600);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path that does not exist cannot be restricted, and saying so is the point: the caller
    /// creates the store and then secures it, so a missing file means the creation it believes
    /// happened did not.
    #[test]
    fn a_missing_file_is_an_error_rather_than_a_silent_success() {
        let missing = std::env::temp_dir().join("custody-no-such-file-ever.db");
        std::fs::remove_file(&missing).ok();
        assert!(restrict_to_owner(&missing).is_err());
    }
}
