//! Synthetic qualification binary. Never built without `synthetic-qualification`.
//!
//! It deliberately shares the production command implementation; the only added surface is the
//! proof-only service and guidance, and this line is the whole of the difference.
//!
//! This used to be `include!("main.rs")`, which compiled all 6,600 lines — the test module
//! included — into a second target, and made every helper in that file structurally impossible to
//! share, move or reach from an integration test.

fn main() -> std::process::ExitCode {
    ironworks_cli::run_cli_for_qualification()
}
