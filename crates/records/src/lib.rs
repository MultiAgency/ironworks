//! The record store: the authority on which org a credential authorises, and the only thing that
//! reads a tenant's book.
//!
//! # Identity implies scope
//!
//! A caller presents a credential and **cannot assert an org**. The store resolves it
//! server-side; nothing here accepts an organization identifier as a parameter, so there is no
//! argument for one to arrive in. [`Book::catalog`] and [`Book::record`] take a
//! [`RecordScope`] — obtainable only from a verified service instance — and read the org from
//! the evidence inside it.
//!
//! # Scope verification is a control; record availability is a data source
//!
//! [`Book::resolve`] fails closed on every count: unreachable, refused, or unreadable all yield
//! no evidence, so no instance is verified and nothing downstream can be reached. That cost is
//! real and accepted — a store outage becomes a serving outage for that instance.
//!
//! Retrieval is different. [`Book::catalog`] returning an error is a fact about the store, not
//! about the tenant, and the caller is expected to render it as
//! [`RecordsState::Degraded`](ironworks_domain::RecordsState) and serve the turn anyway. Degrading
//! a *control* would restore the silence it replaced; degrading a *data source* is honest.
//!
//! # Scope drift
//!
//! Every response names the org the store believes it served. If that ever disagrees with the
//! scope we asked under, the adapter refuses rather than returning the rows: a credential that
//! has been re-pointed at another org mid-flight is exactly the event that would otherwise
//! deliver one tenant's records to another.

use ironworks_domain::record::{AccountId, AccountRecord, Timestamp};
use ironworks_domain::{
    CredentialRef, OrgId, RecordScope, ScopeEvidence, ScopeFailure, ScopeResolver,
};
use serde::Deserialize;
use std::time::Duration;

mod credential;
pub use credential::{CredentialSource, EnvCredentials, Secret};

// The write half, deliberately a sibling rather than a method on `Book`: every `Book` call takes
// a `RecordScope`, which only a verified `ServiceInstance` can produce, and a write method there
// would put mutation on the type a turn is built from.
mod append;
pub use append::{Append, AppendClient, AppendFailure, Appended};
mod administration;
pub use administration::{
    BookExport, Correction, DeleteReceipt, RecordAdministration, RecordAdministrationFailure,
    Retraction,
};

/// Why a record read did not produce records.
///
/// Distinct from [`ScopeFailure`] on purpose: that one is a control failing closed, this one is a
/// data source the caller may degrade around.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum BookFailure {
    #[error("the record store could not be reached: {detail}")]
    Unreachable { detail: String },
    #[error("the record store refused this credential")]
    Unauthorized,
    #[error("the record store answered in a shape this cannot read: {detail}")]
    Malformed { detail: String },
    #[error(
        "SCOPE DRIFT: asked as {asked} but the store answered for {answered} — refusing to \
         return records that may belong to another organization"
    )]
    ScopeDrift { asked: OrgId, answered: OrgId },
    #[error("no credential is available for {handle}")]
    NoCredential { handle: String },
}

/// One row of the bounded account list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: AccountId,
    pub name: String,
    /// The version this row was last changed at. Freshness is decided by comparing this to what
    /// a thread was last shown — never by asking the user.
    pub updated_at: Option<Timestamp>,
}

/// An org's bounded account list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    org: OrgId,
    entries: Vec<CatalogEntry>,
    /// Rows the store returned that could not be read.
    ///
    /// Dropping an unusable row is right — one bad row must not blind the analyst to the other
    /// ninety-nine — but dropping it *silently* is not. `Degradation::Partial { unreadable }`
    /// exists so the model can be told "two are missing", and its own doc argues the model can act
    /// on that and cannot act on "some". Without this the caller reported `Complete` over a book
    /// that had quietly shrunk, so a store schema change would take records away from every tenant
    /// with nothing anywhere reporting it.
    dropped: usize,
}

impl Catalog {
    pub fn org(&self) -> &OrgId {
        &self.org
    }
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// How many rows the store returned that this could not read. See [`Catalog::dropped`]'s field.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

/// A client for one record store.
pub struct Book<C: CredentialSource> {
    agent: ureq::Agent,
    base: String,
    credentials: C,
}

impl<C: CredentialSource> std::fmt::Debug for Book<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Book").field("base", &self.base).finish_non_exhaustive()
    }
}

impl<C: CredentialSource> Book<C> {
    pub fn new(base: impl Into<String>, credentials: C) -> Self {
        Self { agent: agent(), base: base.into().trim_end_matches('/').to_owned(), credentials }
    }

    pub fn endpoint(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str, secret: &Secret) -> Result<(u16, String), String> {
        let mut resp = self
            .agent
            .get(format!("{}{}", self.base, path))
            .header("X-Service-Token", secret.expose())
            .call()
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, body))
    }

    /// Fetch this scope's bounded account list.
    ///
    /// Takes no organization identifier. The org is read from the scope's evidence and checked
    /// against what the store answers.
    pub fn catalog(&self, scope: &RecordScope) -> Result<Catalog, BookFailure> {
        let secret = self.secret_for(scope.credential())?;
        let (status, body) = self
            .get("/list_accounts", &secret)
            .map_err(|detail| BookFailure::Unreachable { detail })?;
        if status == 401 || status == 403 {
            return Err(BookFailure::Unauthorized);
        }
        if !(200..300).contains(&status) {
            return Err(BookFailure::Malformed {
                detail: format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()),
            });
        }
        let wire: WireCatalog = serde_json::from_str(&body)
            .map_err(|e| BookFailure::Malformed { detail: e.to_string() })?;
        let answered = OrgId::parse(wire.org.clone())
            .map_err(|e| BookFailure::Malformed { detail: e.to_string() })?;
        if &answered != scope.org() {
            return Err(BookFailure::ScopeDrift { asked: scope.org().clone(), answered });
        }
        let returned = wire.accounts.len();
        let entries: Vec<CatalogEntry> = wire
            .accounts
            .into_iter()
            .filter_map(|a| {
                // An unusable row is dropped rather than trusted: one bad row must not blind the
                // analyst to the other ninety-nine. Counted, though — see `Catalog::dropped`.
                let id = AccountId::parse(a.account_id).ok()?;
                let name = a.name.filter(|n| !n.trim().is_empty())?;
                let updated_at = a.updated_at.and_then(|t| Timestamp::parse(t).ok());
                Some(CatalogEntry { id, name, updated_at })
            })
            .collect();
        let dropped = returned - entries.len();
        Ok(Catalog { org: answered, entries, dropped })
    }

    /// Fetch one account's full record.
    ///
    /// `Ok(None)` means the store has no such record for this scope — a catalogued-but-absent
    /// row, not an error. It never means "not allowed": a row belonging to another org is simply
    /// not present in this scope.
    pub fn record(
        &self,
        scope: &RecordScope,
        id: &AccountId,
    ) -> Result<Option<AccountRecord>, BookFailure> {
        let secret = self.secret_for(scope.credential())?;
        let path = format!("/get_account_context?account_id={}", encode(id.as_str()));
        let (status, body) =
            self.get(&path, &secret).map_err(|detail| BookFailure::Unreachable { detail })?;
        if status == 401 || status == 403 {
            return Err(BookFailure::Unauthorized);
        }
        if status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(BookFailure::Malformed {
                detail: format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()),
            });
        }
        // The org guard applies to every response that carries one, not only the catalog.
        let scoped: WireScoped = serde_json::from_str(&body)
            .map_err(|e| BookFailure::Malformed { detail: e.to_string() })?;
        let answered = OrgId::parse(scoped.org)
            .map_err(|e| BookFailure::Malformed { detail: e.to_string() })?;
        if &answered != scope.org() {
            return Err(BookFailure::ScopeDrift { asked: scope.org().clone(), answered });
        }
        match AccountRecord::from_json(&body) {
            Ok(r) => Ok(Some(r)),
            // A record that came back but cannot be rendered is the same fact as one that did
            // not come back.
            Err(_) => Ok(None),
        }
    }

    fn secret_for(&self, r: &CredentialRef) -> Result<Secret, BookFailure> {
        self.credentials
            .resolve(r)
            .ok_or_else(|| BookFailure::NoCredential { handle: r.handle().to_owned() })
    }
}

// ── the HTTP budget ─────────────────────────────────────────────────────────────────────────
//
// Named rather than chained inline, because this exact ladder is written in four other crates and
// `crates/ironclaw/src/client.rs::Ironclaw::new` deliberately runs a longer one. That intentional
// difference was invisible against five silent copies of the same eight-call chain; against named
// constants it is a diff on a number. The crates cannot share the values without an edge none of
// them should have, so each states its own and says which system it is stating them about.

/// DNS for the record store.
const RESOLVE: Duration = Duration::from_secs(5);
/// TCP connect to the record store.
const CONNECT: Duration = Duration::from_secs(5);
/// Writing the request to the record store.
const SEND: Duration = Duration::from_secs(10);
/// Waiting for the record store to begin answering.
const RECV_RESPONSE: Duration = Duration::from_secs(30);
/// Reading the record store's answer.
const RECV_BODY: Duration = Duration::from_secs(30);

/// The HTTP agent both of this crate's clients use.
///
/// **Per-phase timeouts**, for the same reason the runtime client uses them: a phase-agnostic
/// timeout cannot say whether anything was attempted, and on a write that is the whole question.
/// One builder rather than one per client, so the read and the append cannot drift into
/// disagreeing about how long a store is allowed to take.
///
/// `http_status_as_error(false)` because a status is an answer to be read, not a transport
/// failure — reading a 409 as one would turn a permanent conflict into a retryable outage.
pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(None)
            .timeout_resolve(Some(RESOLVE))
            .timeout_connect(Some(CONNECT))
            .timeout_send_request(Some(SEND))
            .timeout_recv_response(Some(RECV_RESPONSE))
            .timeout_recv_body(Some(RECV_BODY))
            .http_status_as_error(false)
            .build(),
    )
}

/// The store is the authority on which org a credential authorises.
impl<C: CredentialSource> ScopeResolver for Book<C> {
    fn resolve(&self, credential: &CredentialRef) -> Result<ScopeEvidence, ScopeFailure> {
        let secret =
            self.credentials.resolve(credential).ok_or_else(|| ScopeFailure::Unreachable {
                detail: format!("no credential available for {}", credential.handle()),
            })?;
        let (status, body) = self
            .get("/list_accounts", &secret)
            .map_err(|detail| ScopeFailure::Unreachable { detail })?;
        if status == 401 || status == 403 {
            return Err(ScopeFailure::Unauthorized);
        }
        if !(200..300).contains(&status) {
            return Err(ScopeFailure::Malformed { detail: format!("HTTP {status}") });
        }
        let wire: WireCatalog = serde_json::from_str(&body)
            .map_err(|e| ScopeFailure::Malformed { detail: e.to_string() })?;
        let org = OrgId::parse(wire.org)
            .map_err(|e| ScopeFailure::Malformed { detail: e.to_string() })?;
        Ok(ScopeEvidence::from_record_store(org, &self.base))
    }
}

/// Percent-encode a path segment: RFC 3986 unreserved characters pass, everything else escapes.
///
/// `pub` and shared rather than copied. `crates/reconcile/src/ironclaw.rs` had a byte-identical
/// copy, and `reconcile` already depends on this crate — so the copy bought nothing and gave an
/// account id two encodings that could drift into disagreeing about which bytes are safe in a URL.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ── wire shapes: tolerant of unknown fields, because the store may add one ───────────────

#[derive(Debug, Deserialize)]
struct WireCatalog {
    org: String,
    #[serde(default)]
    accounts: Vec<WireCatalogRow>,
}

#[derive(Debug, Deserialize)]
struct WireCatalogRow {
    account_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireScoped {
    org: String,
}

#[cfg(test)]
mod tests;
