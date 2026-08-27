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
    /// Transport trouble with no phase attribution.
    Transport(String),
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
            Self::Transport(w) => write!(f, "transport failure: {w}"),
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
}
