//! Runtime-reported identity, and the binding that ties it to a recorded member.
//!
//! # Why these types exist
//!
//! `POST/GET /api/webchat/v2/settings/tools` resolves the **caller's** permission scope, so a
//! confinement verdict describes whoever the credential authenticates as. Lifecycle records a
//! member id. Nothing compared the two, which meant a verdict could name a subject it had never
//! authenticated — and `dev/prove-confinement.sh` demonstrably did exactly that.
//!
//! `GET /api/webchat/v2/session` is the runtime's own answer to *which user does this bearer
//! authenticate as*. It shares the request-scoped caller value with the settings endpoints
//! (`ironclaw@4cb47cfaf3/crates/product/ironclaw_webui/src/webui_v2/handlers.rs:255-279` and
//! `:3785-3789, 3814-3820, 3855-3862`), so asking it answers precisely *whose catalogue is about
//! to be read and written*.
//!
//! # What is here and what is deliberately not
//!
//! [`RuntimeSubject`] is **measured**: the tenant and user the runtime reported. There is no
//! `RuntimeOrigin`. The base URL is configured provenance with no decision role, and it is the one
//! value that could smuggle a credential into the store through an authority like
//! `https://user:pass@host`. Storing nothing about the address removes that rather than defending
//! it; the address still reaches the operator's terminal, where it is useful and ephemeral.

use crate::OpaqueId;
use std::fmt;

/// Why a runtime-reported identifier could not be accepted.
///
/// Every variant refuses rather than truncates or repairs: an identifier this cannot re-encode is
/// one it cannot ask about again, and a repaired id would be a different member.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeIdError {
    #[error("{what} is empty")]
    Empty { what: &'static str },
    #[error("{what} is {len} bytes, over the 256-byte ceiling the runtime itself enforces")]
    TooLong { what: &'static str, len: usize },
    #[error("{what} is a dot segment, which the runtime refuses as an identifier")]
    DotSegment { what: &'static str },
    #[error("{what} contains a path separator")]
    PathSeparator { what: &'static str },
    #[error("{what} contains a control character")]
    Control { what: &'static str },
    #[error("{raw:?} is not the timestamp shape this ledger orders by")]
    Timestamp { raw: String },
}

/// The pin's own identifier rules, applied on our side of the wire.
///
/// Mirrors `validate_scope_id`
/// (`ironclaw@4cb47cfaf3/crates/contracts/ironclaw_host_api/src/ids.rs:18-48`, which `UserId` and
/// `TenantId` are both built from at `:209`). Applying them here is not distrust of the runtime —
/// it is what makes a value read back out of SQLite provably the same shape as the value that went
/// in, so a corrupted row is refused instead of being asked about.
fn scope_id(what: &'static str, raw: &str) -> Result<OpaqueId, RuntimeIdError> {
    if raw.is_empty() {
        return Err(RuntimeIdError::Empty { what });
    }
    if raw.len() > 256 {
        return Err(RuntimeIdError::TooLong { what, len: raw.len() });
    }
    if raw == "." || raw == ".." {
        return Err(RuntimeIdError::DotSegment { what });
    }
    if raw.contains('/') || raw.contains('\\') {
        return Err(RuntimeIdError::PathSeparator { what });
    }
    if raw.chars().any(char::is_control) {
        return Err(RuntimeIdError::Control { what });
    }
    Ok(OpaqueId::new(raw))
}

/// The tenant the runtime reported for an authenticated caller.
///
/// Host-installation configuration on the runtime's side, never influenced by a request body
/// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_webui/src/webui_serve.rs:851-862`). It is the
/// only *measured* answer to "which control plane answered".
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeTenantId(OpaqueId);

/// The user the runtime reported for an authenticated caller.
///
/// A distinct type from [`RuntimeTenantId`] on purpose: the two are both opaque strings, they
/// arrive from adjacent JSON keys, and a positional swap would silently compare the wrong pair.
/// The type makes that a compile error rather than a test someone remembers to write.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeUserId(OpaqueId);

impl RuntimeTenantId {
    pub fn parse(raw: &str) -> Result<Self, RuntimeIdError> {
        scope_id("a runtime tenant id", raw).map(Self)
    }
    pub fn id(&self) -> &OpaqueId {
        &self.0
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl RuntimeUserId {
    pub fn parse(raw: &str) -> Result<Self, RuntimeIdError> {
        scope_id("a runtime user id", raw).map(Self)
    }
    pub fn id(&self) -> &OpaqueId {
        &self.0
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for RuntimeTenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl fmt::Display for RuntimeUserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// Who a credential authenticates as, as the runtime itself reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSubject {
    tenant: RuntimeTenantId,
    user: RuntimeUserId,
}

impl RuntimeSubject {
    pub fn new(tenant: RuntimeTenantId, user: RuntimeUserId) -> Self {
        Self { tenant, user }
    }
    pub fn tenant(&self) -> &RuntimeTenantId {
        &self.tenant
    }
    pub fn user(&self) -> &RuntimeUserId {
        &self.user
    }
}

impl fmt::Display for RuntimeSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} in tenant {}", self.user, self.tenant)
    }
}

/// When an attempt was made. The ledger's ordering key, so a malformed value cannot sort wrong.
///
/// Accepts exactly the shape `now_iso` emits — `YYYY-MM-DDTHH:MM:SS±HH:MM` — because lexicographic
/// order is only chronological order while every value has that shape and a fixed offset. A row
/// whose timestamp does not parse is [`crate::LifecycleError::Corrupt`], never the latest.
///
/// # The offset is checked, and it is the half that makes the ordering claim true
///
/// The instant is only half the value. `12:34:56+00:00` and `13:34:56+01:00` are one moment and
/// sort as two; `12:34:56Z` is the same moment again and sorts after both, because `Z` is `0x5A`
/// and `+` is `0x2B`. So "a fixed offset" was never a remark about the format — it is the
/// precondition the `ORDER BY at DESC` in `confinement_attempts_latest` rests on, and a parser
/// that checked the digits and waved the tail through accepted precisely the values that break
/// it. It also accepted `2026-09-02T12:34:56junkjunk`, because nothing past byte 19 was read.
///
/// Hence the literal suffix. This refuses timestamps that are perfectly correct as timestamps,
/// and that is the intended reading of [`RuntimeIdError::Timestamp`]: *not the shape this ledger
/// orders by*. A correct instant that cannot be ordered against the rest of the ledger is not a
/// key, and a type whose stated job is the key is the right place to say so.
///
/// That every value in this repository comes from `now_iso`, and is therefore always `+00:00`, is
/// not the argument for skipping the check — this type exists to refuse a row read back out of
/// SQLite, which is exactly where a value `now_iso` never wrote can arrive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObservedAt(String);

/// The one offset the ledger orders by. `now_iso` emits UTC and nothing else does.
const LEDGER_OFFSET: &str = "+00:00";

impl ObservedAt {
    pub fn parse(raw: &str) -> Result<Self, RuntimeIdError> {
        let bad = || RuntimeIdError::Timestamp { raw: raw.to_owned() };
        // `YYYY-MM-DDTHH:MM:SS` plus the offset, and nothing on either side of it.
        if raw.len() != 19 + LEDGER_OFFSET.len() {
            return Err(bad());
        }
        let b = raw.as_bytes();
        let punctuation = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
        if punctuation.iter().any(|(i, c)| b[*i] != *c) {
            return Err(bad());
        }
        let digits = [(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)];
        if digits.iter().any(|(from, to)| !b[*from..*to].iter().all(u8::is_ascii_digit)) {
            return Err(bad());
        }
        if &raw[19..] != LEDGER_OFFSET {
            return Err(bad());
        }
        Ok(Self(raw.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObservedAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Proof that a runtime-authenticated subject **is** the member lifecycle recorded.
///
/// The only constructor is [`BoundSubject::bind`], and its two arms are exactly the two inputs the
/// attempt constructors take. A "bound" attempt whose subject is not the expected member, or a
/// "mismatch" attempt whose subject is, therefore cannot be built — the relationship is discharged
/// by the type rather than re-checked at each call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSubject {
    expected: OpaqueId,
    authenticated: RuntimeSubject,
}

/// Proof that it is **not**. Constructible only as [`BoundSubject::bind`]'s error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectMismatch {
    expected: OpaqueId,
    authenticated: RuntimeSubject,
}

impl BoundSubject {
    /// Compare the recorded member with what the runtime reported.
    ///
    /// This is the whole binding. Everything that touches a member's settings takes the `Ok` value,
    /// so the comparison cannot be skipped by forgetting it.
    pub fn bind(
        expected: OpaqueId,
        authenticated: RuntimeSubject,
    ) -> Result<Self, SubjectMismatch> {
        if authenticated.user().id() == &expected {
            Ok(Self { expected, authenticated })
        } else {
            Err(SubjectMismatch { expected, authenticated })
        }
    }
    pub fn expected(&self) -> &OpaqueId {
        &self.expected
    }
    pub fn authenticated(&self) -> &RuntimeSubject {
        &self.authenticated
    }
}

impl SubjectMismatch {
    pub fn expected(&self) -> &OpaqueId {
        &self.expected
    }
    pub fn authenticated(&self) -> &RuntimeSubject {
        &self.authenticated
    }
}

impl fmt::Display for SubjectMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "lifecycle records member {}, but the credential authenticates as {}",
            self.expected, self.authenticated
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(tenant: &str, user: &str) -> RuntimeSubject {
        RuntimeSubject::new(
            RuntimeTenantId::parse(tenant).expect("tenant"),
            RuntimeUserId::parse(user).expect("user"),
        )
    }

    #[test]
    fn a_realistic_runtime_id_parses() {
        // The positive control. Every rejection below is only worth something because this one
        // shows the validator admits the value the runtime actually returns.
        assert_eq!(RuntimeUserId::parse("user_7f3a").expect("user").as_str(), "user_7f3a");
        assert_eq!(RuntimeTenantId::parse("acme").expect("tenant").as_str(), "acme");
    }

    #[test]
    fn the_pins_own_identifier_rules_are_enforced() {
        // Mirrors `validate_scope_id` at the pin. Each of these is a distinct refusal there.
        for (label, raw) in [
            ("empty", ""),
            ("dot", "."),
            ("dot dot", ".."),
            ("forward slash", "a/b"),
            ("back slash", "a\\b"),
            ("nul", "a\0b"),
            ("newline", "a\nb"),
        ] {
            assert!(RuntimeUserId::parse(raw).is_err(), "{label} must be refused");
        }
        let long = "u".repeat(257);
        assert!(RuntimeUserId::parse(&long).is_err(), "257 bytes must be refused");
        assert!(
            RuntimeUserId::parse(&long[..256]).is_ok(),
            "256 bytes is the ceiling, not over it"
        );
    }

    #[test]
    fn binding_succeeds_only_when_the_runtime_names_the_recorded_member() {
        let expected = OpaqueId::new("user_7f3a");
        let bound = BoundSubject::bind(expected.clone(), subject("acme", "user_7f3a"))
            .expect("the same id must bind");
        assert_eq!(bound.expected(), &expected);
        assert_eq!(bound.authenticated().user().as_str(), "user_7f3a");
    }

    #[test]
    fn binding_refuses_a_different_subject_and_carries_both_ids() {
        // The negative half. `reborn-cli` is not a hypothetical: it is what the probe's env-bearer
        // token resolves to, while the seeded store records a minted member id.
        let mismatch =
            BoundSubject::bind(OpaqueId::new("user_7f3a"), subject("acme", "reborn-cli"))
                .expect_err("a different id must not bind");
        assert_eq!(mismatch.expected().as_str(), "user_7f3a");
        assert_eq!(mismatch.authenticated().user().as_str(), "reborn-cli");
        // Both identities must reach the operator; a refusal that named only one would leave them
        // guessing which half is wrong.
        let rendered = mismatch.to_string();
        assert!(rendered.contains("user_7f3a"), "{rendered}");
        assert!(rendered.contains("reborn-cli"), "{rendered}");
    }

    #[test]
    fn binding_compares_the_user_and_never_the_tenant() {
        // A tenant that happens to equal the recorded member must not bind. Without this, a
        // deployment whose tenant and member ids are both the client slug would bind vacuously.
        assert!(BoundSubject::bind(OpaqueId::new("acme"), subject("acme", "user_7f3a")).is_err());
    }

    #[test]
    fn a_timestamp_that_cannot_order_is_refused() {
        assert!(
            ObservedAt::parse("2026-09-02T12:34:56+00:00").is_ok(),
            "the shape `now_iso` emits"
        );
        for raw in [
            "",
            "2026-09-02",
            "2026-09-02 12:34:56+00:00",
            "not-a-time-at-all-really",
            "20260902T123456+00:00",
        ] {
            assert!(ObservedAt::parse(raw).is_err(), "{raw:?} must be refused");
        }
    }

    #[test]
    fn a_timestamp_whose_offset_is_not_the_ledgers_is_refused() {
        // The half the digit checks cannot see, and the one that decides whether the ordering
        // claim is true. Each of these names a real instant and none of them orders against
        // `+00:00`: `13:34:56+01:00` IS `12:34:56+00:00` and sorts after it, and `Z` is that
        // moment a third time and sorts after both, because `Z` is `0x5A` and `+` is `0x2B`. A
        // ledger holding two of these would report the wrong attempt as the latest.
        for raw in [
            "2026-09-02T13:34:56+01:00",
            "2026-09-02T11:34:56-01:00",
            "2026-09-02T12:34:56Z",
            "2026-09-02T12:34:56+0000",
            "2026-09-02T12:34:56",
        ] {
            assert!(
                ObservedAt::parse(raw).is_err(),
                "{raw:?} cannot be ordered against the rest of the ledger"
            );
        }
    }

    #[test]
    fn nothing_may_follow_the_offset() {
        // The previous bound accepted any length from twenty to forty and read nothing past byte
        // nineteen, so every one of these parsed and every one sorted somewhere arbitrary.
        for raw in [
            "2026-09-02T12:34:56junkjunk",
            "2026-09-02T12:34:56+00:00 ",
            "2026-09-02T12:34:56+00:00Z",
            "2026-09-02T12:34:56+00:00.123",
        ] {
            assert!(ObservedAt::parse(raw).is_err(), "{raw:?} must be refused");
        }
    }

    #[test]
    fn timestamps_order_lexicographically() {
        // The ledger's `ORDER BY at DESC` rests on this, so it is asserted rather than assumed.
        let earlier = ObservedAt::parse("2026-09-02T12:34:56+00:00").expect("earlier");
        let later = ObservedAt::parse("2026-09-02T12:34:57+00:00").expect("later");
        assert!(earlier < later);
    }
}
