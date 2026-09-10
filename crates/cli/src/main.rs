//! The operator binary, `ironworks`.
//!
//! Three lines, because everything it does is a library now — which is what lets an integration
//! test reach a parser or a boot without spawning a process, and what stops
//! `src/deployment_proof.rs` compiling the whole crate a second time.
//!
//! It names `run_cli` and nothing else. The proof-only qualification service is selectable only
//! through `ironworks_cli::run_cli_for_qualification`, which this binary does not call and which
//! does not exist without the `synthetic-qualification` feature.

fn main() -> std::process::ExitCode {
    ironworks_cli::run_cli()
}
