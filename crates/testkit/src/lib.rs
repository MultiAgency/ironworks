//! Fixtures shared by this workspace's tests.
//!
//! # Two things existed six times each, and both had cost real time
//!
//! **A single-shot HTTP stub** — `TcpListener::bind("127.0.0.1:0")`, a thread, a header-draining
//! read, one canned response — appeared in `records`, `reconcile` (three times), `channel-http`,
//! `telegram` and `crates/cli/tests/common`. The copies were not equivalent, and the difference
//! was operational rather than cosmetic:
//!
//! - Without deadlines, a stub whose client never arrives blocks in `accept` forever. The binary
//!   never exits, so it holds cargo's build lock and nobody can build the repository until someone
//!   finds the process. **Seven were alive at once across two sessions, the oldest for more than a
//!   day.**
//! - With deadlines but a tight one, the same stub goes *red* instead: under a concurrently
//!   running `cargo test --workspace` the spawned child misses a five-second accept window and the
//!   test fails with `Connection refused`, reproducibly, looking exactly like a defect. Measured
//!   2026-09-08: 24s and failing under load, 1.16s and passing alone.
//!
//! So the deadline here is bounded **and generous**, and it is one number rather than six.
//!
//! **A temporary directory** with a `Drop` — six copies, already drifted between `path()` and
//! `.0`, between `expect` and `unwrap`, and one with no `Drop` at all that leaked its directory on
//! any panic.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a stub waits for its one client, and then for that client's request.
///
/// **Generous on purpose.** The old five seconds is enough when the machine is idle and is not
/// when a peer is running the suite, and a fixture that fails under load teaches its reader that
/// red means nothing. Thirty seconds costs nothing on a passing run — the stub returns as soon as
/// the client arrives — and only bounds the wedge.
pub const ACCEPT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a connected client has to finish sending its request head.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A single-shot HTTP stub: one request in, one canned response out.
///
/// The handle yields the request text, or `None` when no client ever arrived. `Option` rather than
/// an empty string, so "nobody connected" cannot pass an assertion that only checks a prefix — a
/// stub that measured nothing must not read as a stub that measured agreement.
///
/// Reads until the request head is complete rather than taking whatever one packet held, so a
/// request split across packets is not a flaky assertion.
pub fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<Option<String>>) {
    serve_many(vec![(status.to_owned(), body.to_owned())])
}

/// A stub that answers `responses` in order, one per connection, then stops accepting.
///
/// `reconcile`'s tests need more than one exchange against one base URL; `records`' need exactly
/// one. Both are this function.
pub fn serve_many(
    responses: Vec<(String, String)>,
) -> (String, std::thread::JoinHandle<Option<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    listener.set_nonblocking(true).expect("nonblocking fixture");
    let address = listener.local_addr().expect("fixture address");
    let thread = std::thread::spawn(move || {
        let mut first = None;
        for (status, body) in responses {
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let Some(mut stream) = accept_within(&listener, ACCEPT_TIMEOUT) else {
                return first;
            };
            // The accepted socket inherits the listener's non-blocking flag on macOS, so this is
            // required rather than tidy: without it the first read returns `WouldBlock` before the
            // client has said anything.
            stream.set_nonblocking(false).expect("blocking fixture stream");
            stream.set_read_timeout(Some(READ_TIMEOUT)).expect("fixture read timeout");
            let request = read_head(&mut stream);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            first.get_or_insert(request);
        }
        first
    });
    (format!("http://{address}"), thread)
}

/// Wait for one client, or give up.
///
/// An `accept` with no deadline blocks forever when the process under test never connects — which
/// an OOM kill of a spawned child is enough to cause — and the test binary then never exits.
fn accept_within(listener: &TcpListener, timeout: Duration) -> Option<std::net::TcpStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

/// Everything up to the blank line, plus exactly `content-length` bytes of body.
fn read_head(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut head = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if let Some((name, value)) = line.split_once(':')
                    && name.trim().eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
                let blank = line == "\r\n" || line == "\n";
                head.push_str(&line);
                if blank {
                    break;
                }
            }
        }
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_ok() {
            head.push_str(&String::from_utf8_lossy(&body));
        }
    }
    head
}

/// A directory no other test in this process can be handed, removed when it goes out of scope.
///
/// # A counter, not a clock
///
/// The first version of one of the six copies keyed on `SystemTime` nanoseconds, and `SystemTime`
/// is coarser than its unit: two tests running in parallel landed in one bucket, shared a store,
/// and one found the other's rows. It failed roughly one run in eight, which is worse than failing
/// always — the claim checker reported the suite BLOCKED at random. A counter cannot collide.
pub struct Scratch(PathBuf);

impl Scratch {
    /// A fresh directory, named for the test that asked.
    pub fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ironworks-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside it. The directory exists; the file need not.
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    /// **On drop, not at the end of the test body.** One of the six copies ended with
    /// `let _ = fs::remove_dir_all(root);` as its last statement, which never runs on a panic — so
    /// every failing run leaked a directory.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests;
