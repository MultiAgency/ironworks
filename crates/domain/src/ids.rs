//! Identifiers, and the one thing that is deliberately *not* a value.

use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("{what} may not be empty")]
    Empty { what: &'static str },
    #[error("{what} {value:?} must be lowercase letters, digits and '-'")]
    Charset { what: &'static str, value: String },
    /// A rejection that deliberately does **not** echo the value.
    ///
    /// [`Charset`](Self::Charset) names what it refused, which is right for a slug an operator
    /// typed and wrong for anything that must stay out of a log line. [`ActorRef`] is refused
    /// through this variant for that reason alone.
    #[error("{what} {reason}")]
    Shape { what: &'static str, reason: &'static str },
}

macro_rules! slug_like {
    ($name:ident, $what:literal, $doc:literal) => {
        #[doc = $doc]
        // `Deserialize` through the validating constructor, never a bare newtype derive: these
        // parse from a record store's JSON, and a value that skipped `parse` would be an
        // identifier the rest of the program believes was checked.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(v: String) -> Result<Self, Self::Error> {
                Self::parse(v)
            }
        }

        impl $name {
            pub fn parse(raw: impl Into<String>) -> Result<Self, IdError> {
                let v = raw.into();
                if v.is_empty() {
                    return Err(IdError::Empty { what: $what });
                }
                if !v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                    return Err(IdError::Charset { what: $what, value: v });
                }
                Ok(Self(v))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

slug_like!(
    Slug,
    "a tenant slug",
    "The short, stable identifier for a tenant — what an operator types."
);
slug_like!(
    ServiceId,
    "a service id",
    "A service's name. The filename is the id and they must agree."
);
slug_like!(
    ContributorId,
    "a contributor id",
    "Who a promoted record is attributed to, inside one tenant's organization.

An **organizational principal**, and deliberately a different type from [`RuntimeIdentity`]: it
names a person in the tenant's own terms, resolved from an operator-recorded binding, and it
carries no claim about any runtime account. It is meant to be read by humans on a shared record,
so unlike [`ActorRef`] it has `Display` — showing it is its purpose."
);

slug_like!(
    OrgId,
    "an org id",
    "The trusted scope key for a tenant's records.

Only ever obtained by resolving a credential server-side (B-1). A value of this type appearing
anywhere a caller supplied it is a defect, which is why the only authoritative one lives inside
[`crate::Verified`]."
);

/// Who sent an inbound message, as the channel attested it.
///
/// # It authorizes and attributes. It never renders.
///
/// This is a channel-native identifier — a Telegram `from.id`, and nothing richer. Its whole job
/// is to select a [`ContributorId`] from the registry and to key a durable proposal. It must not
/// reach guidance, records, a prompt, an envelope or a log line, because a value the sender
/// controls that also *selects* something is the caller-text authority path this design exists to
/// remove.
///
/// **Placement is the guarantee, and the API is what makes placement checkable.** There is no
/// `Display`, so it cannot be interpolated into a message by accident; `Debug` is redacted, so a
/// careless `{:?}` in an error path prints nothing; and the one way to a string is
/// [`persistence_key`](Self::persistence_key), whose name says at every call site that a store or
/// a registry boundary is the only place it belongs. `Ask` takes `Option<&str>` for its speaker
/// and there is no conversion into one — a would-be caller has to type `persistence_key()` and be
/// visible in review.
///
/// It is deliberately **not** a [`RuntimeIdentity`]. Two Telegram bots receive the same
/// `from.id`, and neither can prove which runtime member the other paired it to; treating this as
/// a member handle would assert a cross-system link nothing verifies.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorRef(String);

/// Bounded because it lands in an operator's registry file and a SQLite `TEXT` column. Telegram
/// ids run to twenty digits; a hundred and twenty-eight bytes is room for a channel we have not
/// met without accepting a paste.
const MAX_ACTOR_REF_BYTES: usize = 128;

impl ActorRef {
    pub fn parse(raw: impl Into<String>) -> Result<Self, IdError> {
        let v = raw.into();
        let what = "an actor reference";
        // No charset rule. An actor id is a foreign system's vocabulary, and a charset invented
        // here would refuse a legitimate channel we have not integrated yet. What is checked is
        // only what would make the value unusable as a stable key.
        let reason = if v.is_empty() {
            return Err(IdError::Empty { what });
        } else if v.trim() != v {
            // A key with invisible variants is not stable: `"12"` and `"12 "` would be two
            // contributors in the registry and one to the person reading it.
            Some("must not have leading or trailing whitespace")
        } else if v.len() > MAX_ACTOR_REF_BYTES {
            Some("is longer than 128 bytes")
        } else if v.chars().any(|c| c == '\0' || c.is_control()) {
            Some("must not contain NUL or control characters")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(IdError::Shape { what, reason }),
            None => Ok(Self(v)),
        }
    }

    /// The one representation that leaves this type: what a registry compares and a store writes.
    ///
    /// Named for where it may go rather than for what it returns, so a call site that wants it
    /// for anything else reads wrongly at review time.
    pub fn persistence_key(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ActorRef {
    /// Redacted. A `{:?}` in an error path, a panic message or a tracing line must not carry the
    /// sender's channel identity, and the only way to be sure of that is for the value not to be
    /// there.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ActorRef(redacted)")
    }
}

/// The sealed runtime member a tenant serves as.
///
/// Opaque: it identifies an account on the runtime, never authenticates to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeIdentity(String);

impl RuntimeIdentity {
    pub fn parse(raw: impl Into<String>) -> Result<Self, IdError> {
        let v = raw.into();
        if v.trim().is_empty() {
            return Err(IdError::Empty { what: "a runtime identity" });
        }
        Ok(Self(v))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a tenant's guidance lives. A reference, never the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceRef(String);

impl GuidanceRef {
    pub fn new(location: impl Into<String>) -> Self {
        Self(location.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// **Where** a credential lives — never its value.
///
/// This is the single most important type in this module. `ironworks-domain` is pure and holds no
/// secrets: a credential is resolved by an adapter at the moment of use, so nothing in the domain
/// model can leak one into a log, a journal, or a model prompt (A-4, F-3, E-11).
///
/// It implements no `Display` and its `Debug` names only the location, so even a careless
/// `{:?}` reveals a path rather than a token.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialRef {
    /// What this credential is for. Two tenants must never share one (B-6).
    purpose: CredentialPurpose,
    /// An opaque handle an adapter knows how to resolve.
    handle: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPurpose {
    /// Changes operator-owned IronClaw configuration for one instance.
    ///
    /// This is deliberately distinct from [`Self::RuntimeMember`]: both cross HTTP as bearer
    /// tokens, but a member is tenant-serving authority while this can change shared runtime
    /// policy. Treating them as one purpose would leave that distinction to an environment-variable
    /// name at the call site.
    OperatorConfiguration,
    /// Authenticates as the tenant's sealed runtime member.
    RuntimeMember,
    /// Resolves to exactly one org in the record store. **Reads only.**
    RecordScope,
    /// Appends one confirmed note to the record store, and nothing else.
    ///
    /// # A third purpose rather than a mode on the second
    ///
    /// A credential that could read *and* write depending on how it was called would make
    /// "which authority is this" a property of the call site. Two purposes make it a property of
    /// the value, so a `RecordScope` handle cannot append however it is passed, and this one
    /// cannot read.
    ///
    /// The record store agrees by the same construction: append tokens live in their own
    /// identity map, so a read token is not merely refused there, it is absent.
    ///
    /// **It never enters a [`crate::ServiceInstance`].** That type is what a turn is built from —
    /// it carries the composition, the records binding and the capability profile — and an
    /// append handle inside it would be one refactor away from a model input. The construction
    /// refuses it in either credential slot, so the separation is checked rather than intended.
    RecordAppend,
    /// Produces a complete scoped book export. It cannot select an organization.
    RecordExport,
    /// Appends a correction that supersedes historical evidence.
    RecordCorrect,
    /// Appends a retraction tombstone; it never deletes the target evidence.
    RecordRetract,
    /// Deletes every record in the credential's resolved organization during deprovisioning.
    RecordDelete,
}

impl CredentialRef {
    pub fn new(purpose: CredentialPurpose, handle: impl Into<String>) -> Self {
        Self { purpose, handle: handle.into() }
    }
    pub fn purpose(&self) -> CredentialPurpose {
        self.purpose
    }
    /// The handle an adapter resolves. Not the credential.
    pub fn handle(&self) -> &str {
        &self.handle
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialRef({:?} @ {})", self.purpose, self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_reject_anything_that_is_not_a_slug() {
        // The positive control, asserting the *value*: `is_ok()` alone would pass for a
        // parser that accepted this and returned something else.
        assert_eq!(Slug::parse("acme-sales").unwrap().as_str(), "acme-sales");
        for bad in ["", "Acme", "acme_sales", "acme sales", "acme/sales"] {
            assert!(Slug::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_actor_reference_keeps_what_a_channel_attests_and_refuses_what_cannot_be_a_key() {
        // A Telegram `from.id` as the adapter emits it, and a channel we have not met.
        assert_eq!(ActorRef::parse("184223311").unwrap().persistence_key(), "184223311");
        assert!(ActorRef::parse("U04AB9XYZ").is_ok(), "a foreign charset is not ours to refuse");

        assert!(matches!(ActorRef::parse(""), Err(IdError::Empty { .. })));
        for (bad, why) in [
            (" 184223311", "leading whitespace makes two keys look like one"),
            ("184223311 ", "and so does trailing"),
            ("18422\u{0}3311", "a NUL reaches a registry file and a TEXT column"),
            ("18422\n3311", "so does a newline"),
        ] {
            assert!(matches!(ActorRef::parse(bad), Err(IdError::Shape { .. })), "{why}");
        }
        assert!(ActorRef::parse("x".repeat(129)).is_err(), "unbounded is not a key");
        assert!(ActorRef::parse("x".repeat(128)).is_ok(), "the bound itself is allowed");
    }

    #[test]
    fn refusing_an_actor_reference_does_not_echo_it() {
        // The refusal is the one place a rejected value would otherwise reach an operator's
        // terminal and their shell history. `Charset` names what it refused because a slug is
        // theirs; this is the sender's, and naming it would defeat the redacted `Debug`.
        let err = ActorRef::parse(" 184223311").unwrap_err();
        assert!(!err.to_string().contains("184223311"), "{err}");
    }

    #[test]
    fn an_actor_reference_reveals_nothing_to_a_careless_debug() {
        // The load-bearing one. Every other guarantee about where this value may go is a rule
        // someone has to follow; this is the one the compiler and the formatter enforce together.
        let actor = ActorRef::parse("184223311").unwrap();
        assert_eq!(format!("{actor:?}"), "ActorRef(redacted)");
        assert!(!format!("{actor:?}").contains("184223311"));
        assert!(!format!("{:?}", Some(actor.clone())).contains("184223311"));
        // And there is exactly one way out, which a call site has to name.
        assert_eq!(actor.persistence_key(), "184223311");
    }

    #[test]
    fn a_contributor_is_an_organizational_principal_and_not_a_runtime_member() {
        // Two types on purpose: the binding behind a contributor is operator-recorded, and a
        // `RuntimeIdentity` here would assert a paired IronClaw member nothing has verified.
        let c = ContributorId::parse("alice").unwrap();
        assert_eq!(c.to_string(), "alice", "a contributor is shown; that is what it is for");
        for bad in ["", "Alice", "alice smith", "alice_smith"] {
            assert!(ContributorId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_credential_ref_carries_a_location_not_a_secret() {
        let c = CredentialRef::new(CredentialPurpose::RuntimeMember, "keyring:acme/member");
        // The Debug output is what ends up in a careless log line.
        assert!(format!("{c:?}").contains("keyring:acme/member"));
        assert_eq!(c.handle(), "keyring:acme/member");
        // There is no way to ask this type for a credential, because it never had one.
    }
}

// ── handles that cross the runtime boundary ─────────────────────────────────────────────
//
// These are domain terms — CONTEXT.md defines both — so they live here rather than in the
// transport crate. The *format constraints* on a key are upstream's, mirrored here for the
// reason an anti-corruption layer exists: a bad key should cost a construct error, not a
// round trip and a 400.
/// The runtime's durable handle to a turn. Once we hold one, the answer is fetched, never
/// regenerated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResponseId(String);

impl ResponseId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, InvalidId> {
        let v = raw.into();
        if v.trim().is_empty() {
            return Err(InvalidId);
        }
        Ok(Self(v))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResponseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a response id may not be empty")]
pub struct InvalidId;

/// A handle minted before a request leaves, so a turn we may never hear back about still has a
/// name.
///
/// The constraints are **upstream's**, enforced here so a bad key costs a compile-or-construct
/// error rather than a round trip and a `400`. Read from the pinned source
/// (`validate_bounded_clean_ref`, `allow_colon: false`): non-empty, no surrounding whitespace,
/// at most 256 bytes, no NUL or control characters, no path separators, no colons.
///
/// Mirroring a foreign system's validation is exactly what an anti-corruption layer is for. If
/// upstream relaxes this at a pin bump, this is the one place that changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

/// Upstream's cap, in bytes.
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

impl IdempotencyKey {
    pub fn parse(raw: impl Into<String>) -> Result<Self, InvalidKey> {
        let v = raw.into();
        let reason = if v.is_empty() {
            Some("must not be empty")
        } else if v.trim() != v {
            Some("must not have leading or trailing whitespace")
        } else if v.len() > MAX_IDEMPOTENCY_KEY_BYTES {
            Some("is longer than 256 bytes")
        } else if v.chars().any(|c| c == '\0' || c.is_control()) {
            Some("must not contain NUL or control characters")
        } else if v.contains('/') || v.contains('\\') {
            Some("must not contain path separators")
        } else if v.contains(':') {
            Some("must not contain a colon")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(InvalidKey { reason }),
            None => Ok(Self(v)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("an idempotency key {reason}")]
pub struct InvalidKey {
    pub reason: &'static str,
}
