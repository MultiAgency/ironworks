//! The adapter will not start without its own credential.
//!
//! This is measured by starting the real binary rather than by calling a function, for two
//! reasons. The claim is about a *process* — "a build that can serve `/poll` unauthenticated does
//! not exist" is not a claim any in-process test can make. And the workspace forbids `unsafe`,
//! which `std::env::set_var` is in this edition, so there is no way to arrange the environment
//! from inside a test anyway.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStderr, Command, Stdio};

const EXE: &str = env!("CARGO_BIN_EXE_ironworks-telegram");

/// Start the adapter and read the first thing it says.
///
/// The reader is handed back and deliberately kept alive by the caller: dropping it closes the
/// pipe, and the next `eprintln!` in a still-running child then fails and takes it down — which
/// would make the control look like a refusal.
fn start(adapter_token: Option<&str>) -> (String, Child, BufReader<ChildStderr>) {
    let mut cmd = Command::new(EXE);
    cmd.arg("--port")
        .arg("0")
        .env("TELEGRAM_BOT_TOKEN", "123:stub-bot-token")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match adapter_token {
        Some(value) => cmd.env("CHANNEL_ADAPTER_TOKEN", value),
        None => cmd.env_remove("CHANNEL_ADAPTER_TOKEN"),
    };
    let mut child = cmd.spawn().expect("start the adapter");
    let mut reader = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut said = String::new();
    reader.read_line(&mut said).expect("read the adapter's first line");
    (said, child, reader)
}

#[test]
fn the_adapter_refuses_to_start_without_a_credential_of_its_own() {
    // Missing and present-but-blank are the same defect wearing two hats: an environment file
    // that was created and never filled in is the likelier of the two on a real host.
    for absent in [None, Some(""), Some("   ")] {
        let (said, mut child, _stderr) = start(absent);
        assert!(said.contains("CHANNEL_ADAPTER_TOKEN"), "{absent:?} said {said:?}");
        // It has already printed its reason, so this waits on an exit that has happened rather
        // than racing one — no signal can land between the message and the exit code.
        let status = child.wait().expect("reap the adapter");
        assert_eq!(status.code(), Some(64), "{absent:?} said {said:?}");
    }
}

/// The control. Without it the test above passes on a binary that refuses to start for any
/// reason at all — a missing bot token, an unparseable flag, a panic on line one.
#[test]
fn with_both_credentials_it_starts_and_says_it_is_serving() {
    let (said, mut child, _stderr) = start(Some("adapter-secret"));
    assert!(said.contains("serving the channel contract"), "{said:?}");
    assert!(!said.contains("CHANNEL_ADAPTER_TOKEN"), "{said:?}");
    child.kill().expect("stop the adapter");
    let _ = child.wait();
}
