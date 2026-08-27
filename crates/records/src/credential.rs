//! Turning a credential *reference* into a credential *value*, at the moment of use.
//!
//! `ironworks-domain` holds locations, never secrets, so something has to do this — and it belongs
//! in the adapter that presents the credential rather than anywhere the domain can reach.

use ironworks_domain::CredentialRef;

/// A credential value. Redacted in `Debug`, no `Display`, and `expose` is the only way out — so
/// leaking one takes a deliberate call rather than a stray `{:?}`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The one sanctioned way another crate may *use* this value: as an `Authorization` header.
    ///
    /// `expose` stays crate-private on purpose. This is narrower rather than wider — the caller
    /// receives a header it can only send, never the credential to store, log or compare. Added
    /// when the confinement reconciler needed to authenticate as a member, which is a use, not an
    /// inspection.
    pub fn authorization_header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

/// Resolves a reference to the value it names.
pub trait CredentialSource {
    fn resolve(&self, reference: &CredentialRef) -> Option<Secret>;
}

/// Resolves `env:NAME` handles from the process environment.
///
/// Deliberately the *only* scheme it understands: a source that silently accepted an unknown
/// handle shape would resolve nothing and look like a store outage.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvCredentials;

impl CredentialSource for EnvCredentials {
    fn resolve(&self, reference: &CredentialRef) -> Option<Secret> {
        let name = reference.handle().strip_prefix("env:")?;
        std::env::var(name).ok().filter(|v| !v.is_empty()).map(Secret::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironworks_domain::CredentialPurpose;

    #[test]
    fn a_secret_does_not_leak_through_debug() {
        let s = Secret::new("sk-super-secret");
        assert_eq!(format!("{s:?}"), "Secret(redacted)");
        assert!(!format!("{s:?}").contains("secret"));
    }

    /// Reads an existing variable rather than setting one: the workspace forbids `unsafe`, and
    /// `std::env::set_var` is unsafe in this edition because it races other threads.
    #[test]
    fn env_credentials_resolve_only_the_env_scheme() {
        let present = CredentialRef::new(CredentialPurpose::RecordScope, "env:PATH");
        assert!(EnvCredentials.resolve(&present).is_some(), "PATH is always set");

        let unknown_scheme = CredentialRef::new(CredentialPurpose::RecordScope, "keyring:PATH");
        assert!(
            EnvCredentials.resolve(&unknown_scheme).is_none(),
            "an unhandled scheme must not silently resolve — it would look like a store outage"
        );

        let absent =
            CredentialRef::new(CredentialPurpose::RecordScope, "env:IRONWORKS_DEFINITELY_ABSENT");
        assert!(EnvCredentials.resolve(&absent).is_none());
    }
}
