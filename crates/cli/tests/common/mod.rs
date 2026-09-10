//! Fixtures shared by this crate's integration tests.
//!
//! The stub and the scratch directory both live in `ironworks-testkit` now. This module is the
//! re-export, kept so the tests read as they did — and so the reason is recorded where the copy
//! used to be: the five-second accept deadline here failed under a peer's concurrent
//! `cargo test --workspace`, reproducibly, with `Connection refused`. Measured 2026-09-08: 24s and
//! failing under load, 1.16s and passing alone. `ironworks_testkit::ACCEPT_TIMEOUT` is the one
//! number now, and it is generous rather than tight.

pub use ironworks_testkit::serve;
