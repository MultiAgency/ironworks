//! Deciding whether a failed request could have executed.
//!
//! This is the most consequential judgement in the system and its costs are asymmetric:
//!
//! - calling a may-have-run failure **unsent** invites a second turn, which bills twice and
//!   delivers a different answer chained on a pointer that may already have moved;
//! - calling a provably-unsent failure **unknown** merely blocks one update needlessly.
//!
//! So [`TurnFailure::Unknown`] is the default and only positive evidence downgrades to
//! [`TurnFailure::NotSent`]. The catch-all arm below yields `Unknown`, which is the rare case
//! where a wildcard is correct — an unrecognised variant costs pessimism, not a duplicate turn.
//! (The update state machine is the exact inverse: there a catch-all would be a defect.)
//!
//! [`TurnFailure`] implements **no `From`**, so `?` can never silently convert one into the
//! other. Every construction site says which it means.

use std::fmt;
use std::io;

/// Why a request failed, in terms a caller can log without leaking anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// DNS did not resolve.
    Resolve,
    /// The connection was never established.
    Connect,
    /// The proxy refused to establish a tunnel.
    Proxy,
    /// TLS did not complete — the handshake precedes the request.
    Tls,
    /// The request was malformed before anything was sent.
    Malformed,
    /// Timed out while writing the request, so bytes may be on the wire.
    SendTimeout,
    /// Timed out waiting for the response, so the turn may be running.
    ReceiveTimeout,
    /// The wall-clock budget ran out while the turn was still going.
    BudgetExhausted,
    /// The runtime answered, but not in a shape we understand.
    Undecodable(String),
    /// The credential was rejected.
    Unauthorized,
    /// The idempotency key was replayed against a different body.
    IdempotencyConflict,
    /// The runtime answered with an error envelope.
    Rejected { code: String, message: String },
    /// The runtime refused the request at admission because the caller is over its rate limit.
    ///
    /// **Nothing ran.** `ProductSurfaceRejectionKind::AdmissionRejected` maps to `429` at the pin
    /// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/approval_interaction/types.rs:76`)
    /// and upstream marks the same status retryable beside `503`
    /// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_assistant/src/error.rs:407`). Its own kind
    /// rather than a `Rejected` envelope, because the two answer opposite questions: a rejection
    /// establishes something about the request, and this establishes only that the request was
    /// never considered. The budget is 30 requests per 60 seconds, per user, shared with the chat
    /// surface (§G-6).
    RateLimited,
    /// Transport trouble with no phase attribution.
    Transport(String),
    /// A local control refused the call before the runtime boundary was reached.
    ///
    /// This is not a transport failure and not an unavailable measurement: the control ran and
    /// established that the request was prevented locally. The payload is deliberately bounded
    /// prose chosen by this code, never an IO error that might carry a sensitive path.
    LocalControl(String),
}

impl Cause {
    /// Every variant, for a check that must not silently skip one.
    ///
    /// The payload-carrying variants take a placeholder: this exists to enumerate *kinds*, and a
    /// caller reasoning about kinds must not be able to add one without the checks below seeing it.
    pub const fn all() -> [Self; 15] {
        [
            Self::Resolve,
            Self::Connect,
            Self::Proxy,
            Self::Tls,
            Self::Malformed,
            Self::SendTimeout,
            Self::ReceiveTimeout,
            Self::BudgetExhausted,
            Self::Undecodable(String::new()),
            Self::Unauthorized,
            Self::IdempotencyConflict,
            Self::Rejected { code: String::new(), message: String::new() },
            Self::RateLimited,
            Self::Transport(String::new()),
            Self::LocalControl(String::new()),
        ]
    }

    /// Whether this cause means **the measurement did not happen**.
    ///
    /// `BLOCKED`, not `FAIL` — the distinction this repository draws everywhere else. A dependency
    /// that was never reached, or a wait that ran out, establishes nothing about the thing under
    /// test; reporting it as a failure blames the product for its environment.
    ///
    /// Two causes, kept apart because they need different words: a dependency that is gone
    /// ([`Cause::dependency_unavailable`]) and a turn that outlasted the runtime's wait and is still
    /// running ([`Cause::outran_wait`]). Reporting the second as the first once sent a session
    /// looking for a provider incident that was not happening.
    ///
    /// `dev/lib.sh::unsettled` is the shell spelling of this disjunction, as `degraded` and
    /// `outran_wait` are of its halves, and `the_shell_agrees_about_which_causes_block_a_measurement`
    /// holds them together — the one mechanism this repository allows for a rule that has to exist
    /// in two languages.
    pub fn blocks_measurement(&self) -> bool {
        self.dependency_unavailable() || self.outran_wait()
    }

    /// Whether something this request depended on was **unavailable**, so nothing was learned.
    ///
    /// The line is drawn at *what was established*, not at whose fault it is. A rejected
    /// credential, a malformed request, an undecodable answer and a replayed idempotency key are
    /// all determinate answers about the request that was made, so they stay measurements. The
    /// transport and timeout causes are not.
    ///
    /// **Nor are words that relay an unavailable dependency, whatever kind carries them.** The
    /// runtime passes on what is upstream of it — a provider's rate limit, a provider that is down
    /// — inside a refusal; a proxy's page arrives as a body read verbatim; a decoder can quote the
    /// answer it could not read. None of that is about the request, and no kind can see it, so the
    /// rendered words are read as well, the way `dev/lib.sh::degraded` reads a run's output. The
    /// shell cannot tell whose words they are and neither does this: prose this code authors,
    /// `LocalControl`'s included, must not borrow that vocabulary.
    ///
    /// `dev/lib.sh::degraded` is the shell spelling of this predicate — its first half the kinds,
    /// its second the relayed words.
    pub fn dependency_unavailable(&self) -> bool {
        let by_kind = match self {
            Self::Resolve
            | Self::Connect
            | Self::Proxy
            | Self::Tls
            | Self::SendTimeout
            | Self::ReceiveTimeout
            | Self::BudgetExhausted
            | Self::RateLimited
            | Self::Transport(_) => true,
            Self::Malformed
            | Self::Undecodable(_)
            | Self::Unauthorized
            | Self::IdempotencyConflict
            | Self::Rejected { .. }
            | Self::LocalControl(_) => false,
        };
        by_kind || relays_an_unavailable_dependency(&self.to_string())
    }

    /// Whether the runtime's create wait **ran out while the turn kept running**.
    ///
    /// IronClaw waits `DEFAULT_RESPONSES_WAIT_TIMEOUT` (30s) for a turn to complete and refuses a
    /// slower one with `503 service_unavailable` (§G-5). Nothing is unavailable: the submission
    /// happened, the turn runs to completion server-side, and the answer is recovered by replaying
    /// the idempotency key (ADR 0005). So it blocks a measurement without being an outage.
    ///
    /// `dev/lib.sh::outran_wait` is the shell spelling, and this reads the rendered words the way
    /// its `grep` does: the literal `service_unavailable`, which holds no newline, so containing it
    /// anywhere is the same as some line containing it.
    pub fn outran_wait(&self) -> bool {
        self.to_string().contains("service_unavailable")
    }
}

/// Whether any line of `words` relays that something upstream of the runtime was unavailable.
///
/// The hand-written half of `dev/lib.sh::degraded`, read the way `grep -E` reads it, because the
/// agreement test compares the two by running that grep: case-sensitive, and one line at a time, so
/// nothing matches across a newline. Its three alternatives:
///
/// - `rate.?limit` — `rate_limit_error`, `rate limited`, `ratelimit`;
/// - `429` outside a hex run, so a proxy's bare `429 Too Many Requests` page counts and
///   `request 4297bd26b3b174a1` does not — each half cost a measurement to learn (`0044`);
/// - `provider .* unavailable` — "provider", a space, anything, a space, "unavailable".
fn relays_an_unavailable_dependency(words: &str) -> bool {
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    words.split('\n').any(|line| {
        let rate_limit = line.match_indices("rate").any(|(i, _)| {
            let rest = &line[i + "rate".len()..];
            rest.starts_with("limit")
                || rest.chars().next().is_some_and(|c| rest[c.len_utf8()..].starts_with("limit"))
        });
        let bare_429 = line.match_indices("429").any(|(i, _)| {
            !line[..i].chars().next_back().is_some_and(hex)
                && !line[i + "429".len()..].chars().next().is_some_and(hex)
        });
        let provider = line
            .find("provider ")
            .is_some_and(|i| line[i + "provider ".len()..].contains(" unavailable"));
        rate_limit || bare_429 || provider
    })
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve => f.write_str("the runtime host did not resolve"),
            Self::Connect => f.write_str("the connection was refused or unreachable"),
            Self::Proxy => f.write_str("the egress proxy refused to tunnel"),
            Self::Tls => f.write_str("the TLS handshake failed"),
            Self::Malformed => f.write_str("the request was malformed before it was sent"),
            Self::SendTimeout => f.write_str("timed out writing the request"),
            Self::ReceiveTimeout => f.write_str("timed out awaiting the response"),
            Self::BudgetExhausted => f.write_str("the turn budget ran out while it was running"),
            Self::Undecodable(w) => write!(f, "could not decode the runtime's answer: {w}"),
            Self::Unauthorized => f.write_str("the credential was rejected"),
            Self::IdempotencyConflict => {
                f.write_str("this idempotency key was used with a different body")
            }
            Self::Rejected { code, message } => {
                write!(f, "the runtime refused it: {code} — {message}")
            }
            Self::RateLimited => f.write_str("the runtime rate limited the request; nothing ran"),
            Self::Transport(w) => write!(f, "transport failure: {w}"),
            Self::LocalControl(w) => write!(f, "local control failure: {w}"),
        }
    }
}

/// A request that did not produce an outcome, and whether it could have executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnFailure {
    /// The request provably never left this process. Safe to retry — it cannot have run.
    NotSent(Cause),
    /// It may have executed. **Never retry.** The turn's outcome is unknown, not failed, and a
    /// second attempt would bill an unknowable first one twice.
    Unknown(Cause),
}

impl TurnFailure {
    pub fn cause(&self) -> &Cause {
        match self {
            Self::NotSent(c) | Self::Unknown(c) => c,
        }
    }

    /// Whether it is safe to send this request again.
    ///
    /// True only for `NotSent`, by construction. There is deliberately no way to ask this of an
    /// `Unknown` and get `true`.
    pub fn provably_unsent(&self) -> bool {
        matches!(self, Self::NotSent(_))
    }
}

impl fmt::Display for TurnFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSent(c) => write!(f, "{c} (the request never left, so no turn ran)"),
            Self::Unknown(c) => write!(f, "{c} (a turn MAY have run; it must not be repeated)"),
        }
    }
}

impl std::error::Error for TurnFailure {}

/// Classify a `ureq` failure.
///
/// The phase information lives in `ureq::Error::Timeout(Timeout)`, which is why the client
/// configures **per-phase** timeouts and never a global one — `Timeout::Global` is
/// phase-agnostic and would force everything into `Unknown` (ADR 0004).
pub(crate) fn classify(err: &ureq::Error) -> TurnFailure {
    use ureq::Error as E;
    use ureq::Timeout as T;

    match err {
        // ── provably unsent: nothing reached the socket ──────────────────────────────
        E::HostNotFound => TurnFailure::NotSent(Cause::Resolve),
        E::ConnectionFailed => TurnFailure::NotSent(Cause::Connect),
        E::ConnectProxyFailed(_) => TurnFailure::NotSent(Cause::Proxy),
        E::Tls(_) => TurnFailure::NotSent(Cause::Tls),
        E::BadUri(_) | E::InvalidProxyUrl | E::RequireHttpsOnly(_) | E::Http(_) => {
            TurnFailure::NotSent(Cause::Malformed)
        }
        E::Timeout(T::Resolve) => TurnFailure::NotSent(Cause::Resolve),
        E::Timeout(T::Connect) => TurnFailure::NotSent(Cause::Connect),
        E::Io(e) if is_connect_phase(e.kind()) => TurnFailure::NotSent(Cause::Connect),

        // ── ambiguous: bytes may have reached the runtime ────────────────────────────
        E::Timeout(T::SendRequest | T::SendBody) => TurnFailure::Unknown(Cause::SendTimeout),
        E::Timeout(_) => TurnFailure::Unknown(Cause::ReceiveTimeout),

        // Anything unrecognised is ambiguous. Pessimism is the safe direction here.
        other => TurnFailure::Unknown(Cause::Transport(other.to_string())),
    }
}

/// `io::ErrorKind`s that can only arise before a request is written.
fn is_connect_phase(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::AddrNotAvailable
            | io::ErrorKind::NetworkDown
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_and_connect_failures_are_provably_unsent() {
        for e in [
            ureq::Error::HostNotFound,
            ureq::Error::ConnectionFailed,
            ureq::Error::Timeout(ureq::Timeout::Resolve),
            ureq::Error::Timeout(ureq::Timeout::Connect),
        ] {
            let c = classify(&e);
            assert!(c.provably_unsent(), "{e:?} should be safe to retry");
        }
    }

    #[test]
    fn a_tls_failure_is_unsent_because_the_handshake_precedes_the_request() {
        assert!(classify(&ureq::Error::Tls("handshake")).provably_unsent());
    }

    #[test]
    fn send_and_receive_timeouts_are_ambiguous() {
        for t in [
            ureq::Timeout::SendRequest,
            ureq::Timeout::SendBody,
            ureq::Timeout::RecvResponse,
            ureq::Timeout::RecvBody,
        ] {
            let c = classify(&ureq::Error::Timeout(t));
            assert!(!c.provably_unsent(), "{t:?} must never be retried");
        }
    }

    /// The rule ADR 0004 exists to protect: a phase-agnostic timeout tells us nothing, so it
    /// must fall to `Unknown` — which is precisely why the client never configures one.
    #[test]
    fn a_global_timeout_is_ambiguous_because_it_names_no_phase() {
        for t in [ureq::Timeout::Global, ureq::Timeout::PerCall] {
            assert!(!classify(&ureq::Error::Timeout(t)).provably_unsent());
        }
    }

    #[test]
    fn connect_phase_io_kinds_are_unsent_and_others_are_not() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert!(classify(&ureq::Error::Io(refused)).provably_unsent());
        // A reset mid-stream could have happened after the request was written.
        let reset = io::Error::from(io::ErrorKind::ConnectionReset);
        assert!(!classify(&ureq::Error::Io(reset)).provably_unsent());
    }

    #[test]
    fn an_unrecognised_error_defaults_to_ambiguous() {
        // The safe direction: pessimism costs an unnecessary block, optimism costs a second
        // billed turn.
        assert!(!classify(&ureq::Error::TooManyRedirects).provably_unsent());
        assert!(!classify(&ureq::Error::BodyStalled).provably_unsent());
    }

    #[test]
    fn the_two_variants_read_differently_to_an_operator() {
        let a = TurnFailure::NotSent(Cause::Connect).to_string();
        let b = TurnFailure::Unknown(Cause::ReceiveTimeout).to_string();
        assert!(a.contains("no turn ran"));
        assert!(b.contains("MAY have run"));
    }

    /// A compile-time property stated as a test: there is no `From<...> for TurnFailure`, so `?`
    /// cannot silently decide this for you. If someone adds one, this comment is the reason not
    /// to — and the classification below is the only way in.
    #[test]
    fn classification_is_the_only_construction_path_from_transport_errors() {
        let f = classify(&ureq::Error::HostNotFound);
        assert_eq!(f, TurnFailure::NotSent(Cause::Resolve));
    }

    /// Every `Cause` is classified, and the shell agrees with the classification.
    ///
    /// Two mechanisms for one rule is one mechanism and one thing to keep in sync, so this reads
    /// the other one. `dev/lib.sh::unsettled` decides `BLOCKED` versus `FAIL` for every
    /// `dev/prove*.sh`, and it cannot call this crate — so the agreement is checked here, through
    /// the same `grep` bash will run with the flags and pattern read out of `dev/lib.sh`, rather
    /// than asserted in a comment.
    ///
    /// It caught a real drift: `ReceiveTimeout` renders "timed out awaiting the response" and the
    /// shell matched only `timed out (writing|reading)`, so the runtime's ~30s synchronous limit
    /// scored FAIL. `Connect` and `BudgetExhausted` missed the same way.
    ///
    /// The non-blocking half is the positive control (§K-8): a pattern that matched everything
    /// would satisfy the first assertion and be exactly the defect.
    ///
    /// **And it has to see the words, not only the kinds.** `Cause::all()` carries empty
    /// placeholders, so this once compared the shell's provider clauses against nothing: a
    /// `Rejected` relaying `provider nearai unavailable` or `rate_limit_error` was BLOCKED to the
    /// shell and a measurement here, and the test passed. Every variant now carries every sample
    /// in every position it has words. `blocking` must block and the `measured` samples must not —
    /// `0044`'s digest among them — so agreement cannot come from both sides answering one way.
    ///
    /// **And for both halves, not only the one that says a dependency is gone.** A create that
    /// outlasts IronClaw's 30s wait comes back `service_unavailable` while the turn keeps running
    /// (§G-5). The shell scored it BLOCKED through `unsettled` and this called it a measurement, and
    /// nothing compared the two because only `degraded` was read. So `dependency_unavailable` is
    /// held to `degraded` and `blocks_measurement` to `unsettled`, and the `outran` samples must
    /// block without being called an outage.
    #[test]
    fn the_shell_agrees_about_which_causes_block_a_measurement() {
        let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/lib.sh"))
            .expect("dev/lib.sh is the other half of this rule");
        let degraded = shell_grep(&lib, "degraded");
        let outran_wait = shell_grep(&lib, "outran_wait");
        assert_eq!(
            shell_body(&lib, "unsettled"),
            r#"degraded "$1" || outran_wait "$1""#,
            "dev/lib.sh::unsettled is no longer the disjunction Cause::blocks_measurement spells"
        );

        // Relayed prose: a provider that is down, a rate limit in an envelope's code, the runtime's
        // own log line for one, a proxy's bare page, and the records client's status line.
        let blocking = [
            "provider nearai unavailable",
            "rate_limit_error",
            "Provider nearai_chat rate limited",
            "429 Too Many Requests",
            "HTTP 429: slow down",
        ];
        // `0044`'s digest and a response id, each with 429 inside a hex run; an ordinary refusal;
        // and nothing at all.
        // The 30s cliff, as the envelope's code and inside a message: a turn still running.
        let outran = [
            "service_unavailable",
            "the response did not complete in time (service_unavailable) [server_error]",
        ];
        let measured = ["request 4297bd26b3b174a1", "resp_c0ffee429d", "invalid_request", ""];
        // Neither list, and here only to hold this crate to grep's reading: case-sensitive, and
        // one line at a time.
        let grep_semantics = ["Provider nearai unavailable", "provider nearai\n is unavailable"];

        for kind in Cause::all() {
            for text in blocking.iter().chain(&outran).chain(&measured).chain(&grep_semantics) {
                for cause in carrying(&kind, text) {
                    let words = cause.to_string();
                    let gone = greps(&degraded, &words);
                    assert_eq!(
                        gone,
                        cause.dependency_unavailable(),
                        "dev/lib.sh::degraded and Cause::dependency_unavailable disagree about \
                         {cause:?} ({words:?}); the two report a dependency gone in different words \
                         from a turn still running"
                    );
                    assert_eq!(
                        gone || greps(&outran_wait, &words),
                        cause.blocks_measurement(),
                        "dev/lib.sh::unsettled and Cause::blocks_measurement disagree about \
                         {cause:?} ({words:?}); a blocking cause the shell misses is scored FAIL \
                         by every dev/prove*.sh, and one it over-matches hides a real failure"
                    );
                }
            }
        }

        let refusal = Cause::Rejected { code: String::new(), message: String::new() };
        for text in blocking {
            for cause in carrying(&refusal, text) {
                assert!(
                    cause.dependency_unavailable(),
                    "{cause:?} relays an unavailable dependency"
                );
            }
        }
        for text in outran {
            for cause in carrying(&refusal, text) {
                assert!(
                    cause.blocks_measurement(),
                    "{cause:?} outran the wait; nothing was measured"
                );
                assert!(!cause.dependency_unavailable(), "{cause:?} is a turn still running");
            }
        }
        for text in measured {
            for kind in [&refusal, &Cause::Undecodable(String::new())] {
                for cause in carrying(kind, text) {
                    assert!(!cause.blocks_measurement(), "{cause:?} is a measurement");
                }
            }
        }
    }

    /// The body of the `dev/lib.sh` function `name`: the line after `name() {`.
    fn shell_body<'a>(lib: &'a str, name: &str) -> &'a str {
        let header = format!("{name}() {{");
        lib.lines()
            .skip_while(|line| *line != header)
            .nth(1)
            .unwrap_or_else(|| panic!("dev/lib.sh::{name} is gone"))
            .trim()
    }

    /// The flags and pattern the `dev/lib.sh` function `name` runs `grep` with on its argument.
    fn shell_grep(lib: &str, name: &str) -> (String, String) {
        let body = shell_body(lib, name);
        body.strip_prefix("grep ")
            .and_then(|rest| rest.strip_suffix("\" <<<\"$1\""))
            .and_then(|rest| rest.split_once(" \""))
            .map(|(flags, pattern)| (flags.to_owned(), pattern.to_owned()))
            .unwrap_or_else(|| panic!("dev/lib.sh::{name} no longer greps one pattern: {body}"))
    }

    /// Whether `grep`, run with the shell's own flags and pattern, matches `text`.
    fn greps((flags, pattern): &(String, String), text: &str) -> bool {
        std::process::Command::new("grep")
            .arg(flags)
            .arg("-e")
            .arg(pattern)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().expect("stdin").write_all(text.as_bytes())?;
                child.wait()
            })
            .expect("grep")
            .success()
    }

    /// `cause` carrying `text` in every position it has words. Exhaustive rather than a wildcard,
    /// so a variant that gains a payload cannot be left out of the test above.
    fn carrying(cause: &Cause, text: &str) -> Vec<Cause> {
        let text = || text.to_owned();
        match cause {
            Cause::Undecodable(_) => vec![Cause::Undecodable(text())],
            Cause::Transport(_) => vec![Cause::Transport(text())],
            Cause::LocalControl(_) => vec![Cause::LocalControl(text())],
            Cause::Rejected { .. } => vec![
                Cause::Rejected { code: text(), message: String::new() },
                Cause::Rejected { code: String::new(), message: text() },
            ],
            Cause::Resolve
            | Cause::Connect
            | Cause::Proxy
            | Cause::Tls
            | Cause::Malformed
            | Cause::SendTimeout
            | Cause::ReceiveTimeout
            | Cause::BudgetExhausted
            | Cause::Unauthorized
            | Cause::IdempotencyConflict
            | Cause::RateLimited => vec![cause.clone()],
        }
    }
}
