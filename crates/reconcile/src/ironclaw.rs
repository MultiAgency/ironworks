//! The IronClaw admin surface: who exists on the instance, and does it match what we recorded.
//!
//! # What the API can and cannot answer
//!
//! Read from the pinned revision, `crates/contracts/ironclaw_product_contracts/src/admin_users.rs`:
//!
//! ```text
//! AdminUserRecord { user_id, email, display_name, status, role, created_at,
//!                   updated_at, created_by, last_login_at, metadata }
//! ```
//!
//! **There is no tenant, org, or instance field.** `metadata` could carry one and never does —
//! provisioning sends `{"display_name": <name>, "role": "member"}` and nothing else. `created_by`
//! names the operator, not the instance. `display_name` is a mutable label and demonstrably
//! non-unique: the reference's own reconciliation groups unknown members *by* display name
//! because several share one.
//!
//! So the API answers **"does this id exist?"** and **"who exists?"**, and it cannot answer
//! **"which member belongs to this instance?"** That mapping lives only in what IronWorks
//! recorded creating, which is why these are two paths rather than one:
//!
//! > Per-instance reconciliation proves custody of identities IronWorks recorded; fleet
//! > reconciliation discovers external authority no IronWorks instance accounts for.
//!
//! # What this observes, and what it must not be read as
//!
//! Member-directory state only. **A member being absent is not evidence that its session was
//! revoked** — measured at the pin, the session store consults the signature, expiry, a
//! process-local revoked set and the tenant, and never reads the user directory. Deletion is not
//! revocation, and nothing here reports it as such.

use crate::OperatorToken;
use ironworks_domain::CredentialRef;
use ironworks_lifecycle::{
    Authority, AuthorityFact, AuthorityInventory, Blocked, BlockedReason, BoundSubject,
    Confinement, ConfinementAttempt, ConfinementPolicy, InstanceSpec, Measurement, ObservedAt,
    OpaqueId, Plan, PolicyBinding, Presence, Reconcile, RuntimeSubject, RuntimeTenantId,
    RuntimeUserId, Step, SubjectMismatch, assess,
};
use ironworks_records::{CredentialSource, Secret, encode};
use std::collections::BTreeSet;

/// One read of the member's tool settings: each row's effective state, the subset the runtime
/// reports as `locked`, and the global approval flag.
///
/// All three come from the same response and must not be fetched separately — a plan and the
/// record of what it was planned against would otherwise be able to disagree.
///
/// # A struct, because a tuple alias could not keep that promise
///
/// This was `type CatalogueReading = (BTreeMap<String, String>, BTreeSet<String>, bool)`, and an
/// alias gives no field names: one caller destructured it `(states, locked, auto_approve)` and
/// another `(tools, locked, auto_approve)`. Two of the three are string collections, so
/// transposing them compiles — which is the failure the doc paragraph above was written to prevent
/// and the type was not enforcing.
struct CatalogueReading {
    /// Each tool row's effective state, by name.
    states: std::collections::BTreeMap<String, String>,
    /// The subset the runtime reports as `locked`, read rather than assumed.
    locked: std::collections::BTreeSet<String>,
    /// The global approval flag, as the catalogue reported it.
    auto_approve: bool,
}

/// Upstream's own cap. Asking for more than this is not an error, it is silently clamped — so a
/// caller that assumed its page size was honoured would under-enumerate without being told.
const MAX_PAGE: usize = 200;

/// A ceiling on pages followed, so a cursor that never terminates fails closed instead of
/// spinning. At 200 per page this covers 20,000 members; an instance past that is a fact worth
/// surfacing rather than paging through.
const MAX_PAGES: usize = 100;

/// The admin surface of one IronClaw instance. Reads only.
pub struct Admin {
    agent: ureq::Agent,
    base: String,
    token: Option<OperatorToken>,
}

impl Admin {
    /// `credential` names **where** the operator token lives; it is resolved once, here, and the
    /// value never leaves this struct.
    pub fn new(base: impl Into<String>, credential: &CredentialRef) -> Self {
        Self {
            agent: crate::operator_agent(),
            base: base.into().trim_end_matches('/').to_owned(),
            token: OperatorToken::from_env(credential),
        }
    }

    /// Supply the operator token directly. Test-only: the environment cannot be mutated under
    /// `unsafe_code = "forbid"`, and a fixture admin surface still has to be authenticated.
    #[cfg(test)]
    pub(crate) fn set_token_for_test(&mut self, token: crate::OperatorToken) {
        self.token = Some(token);
    }

    /// The same GET, authenticated as a member rather than the operator. Separate method so the
    /// two credentials cannot be confused at a call site.
    pub(crate) fn get_as_member(
        &self,
        path: &str,
        secret: &Secret,
    ) -> Result<(u16, String), BlockedReason> {
        self.request(path, &secret.authorization_header())
    }

    fn get(&self, path: &str) -> Result<(u16, String), BlockedReason> {
        let token = self
            .token
            .as_ref()
            .ok_or(BlockedReason::NoCredential { system: "ironclaw admin".into() })?;
        self.request(path, &token.header())
    }

    /// Write to the **caller's own** scope, as the member.
    ///
    /// There is no operator form of this on purpose: measured at 1.4.0, `POST /settings/tools`
    /// resolves the caller's permission scope and `?user_id=` is silently ignored, so an operator
    /// token here would confine the operator and report success.
    pub(crate) fn post_as_member(
        &self,
        path: &str,
        secret: &Secret,
        body: &str,
    ) -> Result<(u16, String), BlockedReason> {
        let mut response = self
            .agent
            .post(format!("{}{path}", self.base))
            .header("authorization", &secret.authorization_header())
            .header("content-type", "application/json")
            .send(body)
            .map_err(|e| BlockedReason::Unreachable { system: format!("ironclaw admin ({e})") })?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
        Ok((status, text))
    }

    fn request(&self, path: &str, authorization: &str) -> Result<(u16, String), BlockedReason> {
        let mut response = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("authorization", authorization)
            .call()
            .map_err(|e| BlockedReason::Unreachable { system: format!("ironclaw admin ({e})") })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
        Ok((status, body))
    }

    /// Does this member exist?
    ///
    /// The one deterministic question. `404` is authoritative absence — the desired end state of
    /// a deletion is *no such record*, and the reference reads it the same way. Anything else is
    /// unverifiable: a `401` says the operator token is wrong, a `500` says the instance is
    /// unwell, and neither says anything about the member.
    pub fn member(&self, id: &OpaqueId) -> Presence {
        match self.get(&format!("/api/webchat/v2/admin/users/{}", encode(id.as_str()))) {
            Ok((200..=299, _)) => Presence::Present { id: id.clone() },
            Ok((404, _)) => Presence::Absent,
            Ok((401 | 403, _)) => Presence::Unverifiable(BlockedReason::CredentialRejected),
            Ok((status, _)) => Presence::Unverifiable(BlockedReason::Malformed {
                detail: format!("HTTP {status}"),
            }),
            Err(reason) => Presence::Unverifiable(reason),
        }
    }

    /// Every member on the instance, by authoritative id, following `next_cursor` to the end.
    ///
    /// **Ids only.** Display names and emails are read past deliberately: carrying them would put
    /// a mutable label next to an identity in a report about custody, and someone would
    /// eventually match on it.
    pub fn all_member_ids(&self) -> Result<BTreeSet<OpaqueId>, BlockedReason> {
        let mut ids = BTreeSet::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = BTreeSet::new();
        for _ in 0..MAX_PAGES {
            let path = match &cursor {
                Some(c) => {
                    format!("/api/webchat/v2/admin/users?limit={MAX_PAGE}&cursor={}", encode(c))
                }
                None => format!("/api/webchat/v2/admin/users?limit={MAX_PAGE}"),
            };
            let (status, body) = self.get(&path)?;
            require_success(status)?;
            let page: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
            // A MISSING `users` field and an EMPTY one are different answers. The reference
            // could not tell them apart (`d.get("users", ...)`) so it failed closed on both;
            // parsing lets us say that an instance genuinely holding no members is a complete
            // enumeration, while a response that never mentioned members is unusable.
            let Some(users) = page.get("users").and_then(|v| v.as_array()) else {
                return Err(BlockedReason::Malformed {
                    detail: "the admin listing carried no users field".into(),
                });
            };
            for user in users {
                let Some(id) = user.get("user_id").and_then(|v| v.as_str()) else {
                    return Err(BlockedReason::Malformed {
                        detail: "a listed member carried no user_id".into(),
                    });
                };
                ids.insert(OpaqueId::new(id));
            }
            match page.get("next_cursor").and_then(|v| v.as_str()) {
                None => return Ok(ids),
                Some(next) => {
                    // A cursor that repeats is a server that will never terminate. Failing closed
                    // beats enumerating forever and beats reporting a partial set as complete.
                    if !seen_cursors.insert(next.to_owned()) {
                        return Err(BlockedReason::Malformed {
                            detail: "the admin listing returned a repeating cursor".into(),
                        });
                    }
                    cursor = Some(next.to_owned());
                }
            }
        }
        Err(BlockedReason::Malformed {
            detail: format!("the admin listing did not terminate within {MAX_PAGES} pages"),
        })
    }
}

// ── per-instance ────────────────────────────────────────────────────────────────────────────

/// Does the member IronWorks recorded creating still exist?
///
/// It takes the recorded id and **nothing else**. In particular it ignores the
/// [`InstanceSpec`] entirely — see [`IronclawMember::observe`] — because no field of a spec can
/// be matched against a member without inventing an attribution the API does not support.
pub struct IronclawMember<'a> {
    admin: &'a Admin,
    recorded: Option<OpaqueId>,
}

impl<'a> IronclawMember<'a> {
    pub fn new(admin: &'a Admin, recorded: Option<OpaqueId>) -> Self {
        Self { admin, recorded }
    }
}

impl Reconcile for IronclawMember<'_> {
    /// **`spec` is deliberately unused, and that is the point.**
    ///
    /// The only thing that could connect this instance to a member is the id IronWorks recorded.
    /// A slug, an org, a service or a channel cannot be matched against anything the admin API
    /// returns — so a reconciler that read the spec would be reaching for `display_name`, which
    /// is a mutable label anyone can change. The unused parameter is the guarantee.
    fn observe(&self, _spec: &InstanceSpec) -> AuthorityInventory {
        let presence = match &self.recorded {
            // No subject, so no question. Not absence: a member may well exist, and this cannot
            // ask about it. The fleet path is what finds those.
            None => Presence::Unverifiable(BlockedReason::NoRecordedIdentity),
            Some(id) => self.admin.member(id),
        };
        AuthorityInventory::Reconciled(vec![AuthorityFact {
            authority: Authority::Member,
            presence,
        }])
    }
}

// ── fleet ───────────────────────────────────────────────────────────────────────────────────

/// What a complete pass over the instance's member directory found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetInventory {
    /// Enumeration reached the terminal page.
    ///
    /// **Both directions.** `unaccounted` is every member id that no lifecycle record claims — the
    /// set that, in the reference, was 41 of 43. `dangling` is the reverse: every id IronWorks
    /// recorded creating that the instance does not hold.
    ///
    /// Reporting only the first produces a clean verdict while records point at members that do
    /// not exist — measured, on a live instance returning an empty directory against a store
    /// holding one recorded id. "Every member on the instance is accounted for" was true and
    /// useless.
    Complete { observed: usize, unaccounted: BTreeSet<OpaqueId>, dangling: BTreeSet<OpaqueId> },
    /// Enumeration finished, but **IronWorks accounts for no members at all**, so there is
    /// nothing to difference against.
    ///
    /// Deliberately a variant rather than a flag: it can never be read as an answer, and it still
    /// carries what was observed. Every member on the instance is unaccounted in this state —
    /// that fact is real and must still reach an operator, it just cannot be scored as agreement.
    NothingToReconcile { observed: usize, unaccounted: BTreeSet<OpaqueId> },
    /// Enumeration did not finish, so the set difference is meaningless. **Never reported as
    /// zero unknowns** — a truncated page is exactly how a clean score gets computed from
    /// partial evidence.
    Blocked(BlockedReason),
}

impl FleetInventory {
    /// Whether this may be read as an answer at all. Only [`Self::Complete`] may, and only it
    /// can produce a passing verdict.
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }
}

/// Where the instance's member directory and IronWorks' records disagree, in both directions.
///
/// `accounted` is the set of member ids recorded across **all** instances — a set difference
/// against our own records, because the instance cannot tell us whose a member is.
///
/// Compares opaque identifiers and nothing else: no tenant, no records, no context. It could not
/// reach a tenant's book if it wanted to, and it has no parameter through which one could arrive.
pub fn unaccounted_members(admin: &Admin, accounted: &BTreeSet<OpaqueId>) -> FleetInventory {
    match admin.all_member_ids() {
        Ok(observed) => {
            let unaccounted: BTreeSet<OpaqueId> = observed.difference(accounted).cloned().collect();
            // A difference against an empty set is arithmetic, not evidence. Checked AFTER
            // enumerating, so what is out there is still reported — an instance holding members
            // while IronWorks accounts for none is the most alarming state this can find, and it
            // must not be silenced by the same rule that stops it being scored as agreement.
            if accounted.is_empty() {
                return FleetInventory::NothingToReconcile {
                    observed: observed.len(),
                    unaccounted,
                };
            }
            FleetInventory::Complete {
                observed: observed.len(),
                unaccounted,
                dangling: accounted.difference(&observed).cloned().collect(),
            }
        }
        Err(reason) => FleetInventory::Blocked(reason),
    }
}

/// A non-2xx status, as the reason it blocks a measurement.
///
/// Written out three times in this file with the same two arms. **A refused credential is not a
/// malformed answer**: one says we were not allowed to look, the other says we looked and could
/// not read what came back, and `BlockedReason` keeps them apart precisely so a run can say which.
/// Three copies is three places for that distinction to drift.
fn require_success(status: u16) -> Result<(), BlockedReason> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    Err(match status {
        401 | 403 => BlockedReason::CredentialRejected,
        _ => BlockedReason::Malformed { detail: format!("HTTP {status}") },
    })
}

// ── confinement ─────────────────────────────────────────────────────────────────────────────

/// What a confinement run wrote. **Not** a statement about whether the member is confined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Written { steps: usize },
    Stopped { applied: usize, at: Option<Step>, why: BlockedReason },
}

/// Whether a tool-permission response confirms the state we asked for.
///
/// The status is not the answer. IronClaw's own settings client validates the persisted entry —
/// `value.state` must equal the requested state — because a save that returns 200 and stores
/// nothing is a save that looks applied.
fn confirms_disabled(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("entry")
                .and_then(|e| e.get("value"))
                .and_then(|value| value.get("state"))
                .and_then(|s| s.as_str())
                .map(|s| s == "disabled")
        })
        .unwrap_or(false)
}

/// Whether a settings response still reports automatic approval as on.
///
/// **Measured at 1.4.0**, not inferred: the reply is
/// `{"entry":{"key":"agent.auto_approve_tools","value":false,"source":"override",…}}`. An earlier
/// version of this read a `settings` map, which the frontend client's *request* shape suggested
/// and the response does not have — so a write that had landed was reported unconfirmed. Reading
/// the wire is the rule; reading a client that talks to it is not the same thing.
///
/// Fails closed: a body this cannot parse, or one whose entry names something else, reports *on*.
/// That stops the run rather than letting every denial after it be reversible.
fn confirms_auto_approve_on(body: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return true;
    };
    let entry = parsed.get("entry");
    let names_the_setting = entry
        .and_then(|e| e.get("key"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| k == "agent.auto_approve_tools");
    if !names_the_setting {
        return true;
    }
    match entry.and_then(|e| e.get("value")) {
        Some(serde_json::Value::Bool(on)) => *on,
        _ => true,
    }
}

fn first_line(body: &str) -> String {
    body.lines().next().unwrap_or("").chars().take(180).collect()
}

/// Observes a member's effective tool authority, **as that member**.
///
/// # Why this needs the member's own credential
///
/// `POST/GET /api/webchat/v2/settings/tools` resolves the **caller's** permission scope. Measured
/// at 1.4.0: an operator reading it sees the operator's own catalogue, and `?user_id=` is silently
/// ignored — so a reconciler holding only the operator token would read one catalogue N times and
/// certify every member from it.
///
/// So confinement observation authenticates as the member, and the credential is named by a
/// [`CredentialRef`] resolved at the moment of use. The lifecycle store holds no credential and
/// must not: what it records is the member *id*, and the registry holds the secret.
///
/// **This is also the guarantee's boundary.** The same credential that lets IronWorks verify
/// confinement lets its holder undo it, because 1.4.0 has no non-overridable lock. Custody is the
/// containment, and the type takes a credential reference to make that visible at every call site.
pub struct MemberConfinement<'a, C: CredentialSource> {
    admin: &'a Admin,
    credentials: &'a C,
    member_credential: CredentialRef,
}

/// Why a member could not be bound.
///
/// The two variants are exactly the two inputs the non-measuring attempt constructors take, so a
/// caller cannot record the wrong kind of failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// `/session` did not answer usably. No subject was established.
    Blocked(BlockedReason),
    /// It answered, and it is not our member.
    Mismatch(SubjectMismatch),
}

/// A member whose runtime-reported subject has been compared with lifecycle's record.
///
/// # It is the only way to reach the settings surface
///
/// [`MemberConfinement`] has no catalogue, observe or apply method. They live here, and the only
/// constructor of this type is [`MemberConfinement::bind`], which makes the comparison. Forgetting
/// the comparison is therefore not a mistake anyone can make at a call site — there is no call site
/// that compiles without it.
///
/// # It holds no secret
///
/// It borrows the [`MemberConfinement`], which retains the [`CredentialRef`] and the
/// [`CredentialSource`] and resolves the value at the moment of each use, exactly as before.
pub struct BoundMember<'b, 'a, C: CredentialSource> {
    confinement: &'b MemberConfinement<'a, C>,
    bound: BoundSubject,
}

/// What one catalogue read produced: the raw states a plan needs, and the sealed attempt a ledger
/// takes. **One read yields both**, so the binding and the catalogue facts in an appended row
/// cannot have come from different moments.
pub enum Reading {
    Measured {
        states: std::collections::BTreeMap<String, String>,
        /// The rows this same read reported as `locked` — carried beside `states` so a plan and
        /// the record of what it was planned against come from one measurement.
        locked: std::collections::BTreeSet<String>,
        auto_approve: bool,
        attempt: ConfinementAttempt,
    },
    Blocked {
        attempt: ConfinementAttempt,
    },
}

impl Reading {
    pub fn attempt(&self) -> &ConfinementAttempt {
        match self {
            Self::Measured { attempt, .. } | Self::Blocked { attempt } => attempt,
        }
    }
}

impl<'a, C: CredentialSource> MemberConfinement<'a, C> {
    pub fn new(admin: &'a Admin, credentials: &'a C, member_credential: CredentialRef) -> Self {
        Self { admin, credentials, member_credential }
    }

    /// Ask the runtime who this credential authenticates as.
    ///
    /// `GET /api/webchat/v2/session` answers from the same request-scoped caller value that
    /// `/settings/tools` scopes off
    /// (`ironclaw@4cb47cfaf3/crates/product/ironclaw_webui/src/webui_v2/handlers.rs:255-279`,
    /// `:3785-3789`), and the middleware builds it from the authenticated bearer with the tenant
    /// stamped from host-installation config — the browser body can influence neither
    /// (`webui_serve.rs:851-862`). So this is not an approximation of "whose catalogue is about to
    /// be read"; it is that question, asked of the component that answers it.
    ///
    /// The `user_id` is the same `UserId` an admin-created member is minted for
    /// (`ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/commands/serve.rs:74-88` mints for
    /// `(tenant, user_id)`; `session.rs:124-153` returns that id back), so it is comparable with
    /// what lifecycle recorded at `member_minted`.
    ///
    /// **Every anchor above is into `ironclaw@4cb47cfaf3`**, the revision `IRONCLAW_PIN` names.
    /// Saying so once is the point: a line number is durable only against a tree that cannot move
    /// under it, and one written `.../file.rs:851` names no revision and so promises nothing. The
    /// two short names are written as this repository found them — the pinned source is not
    /// checked out here, and inventing a directory for them would be worse than eliding one.
    pub fn authenticated_subject(&self) -> Result<RuntimeSubject, BlockedReason> {
        let Some(secret) = self.credentials.resolve(&self.member_credential) else {
            return Err(BlockedReason::NoCredential {
                system: format!("member credential at {}", self.member_credential.handle()),
            });
        };
        let (status, body) = self.admin.get_as_member("/api/webchat/v2/session", &secret)?;
        require_success(status)?;
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
        let field = |name: &str| -> Result<String, BlockedReason> {
            parsed.get(name).and_then(serde_json::Value::as_str).map(str::to_owned).ok_or_else(
                || BlockedReason::Malformed {
                    detail: format!("the session response carried no {name}"),
                },
            )
        };
        // Refused rather than repaired: an identifier we cannot re-encode is one we cannot ask
        // about again, and a repaired id would name a different member.
        let tenant = RuntimeTenantId::parse(&field("tenant_id")?)
            .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
        let user = RuntimeUserId::parse(&field("user_id")?)
            .map_err(|e| BlockedReason::Malformed { detail: e.to_string() })?;
        Ok(RuntimeSubject::new(tenant, user))
    }

    /// Compare the runtime's answer with the member lifecycle recorded.
    ///
    /// **Nothing touches the settings surface before this succeeds.** A mismatch returns without a
    /// second request, so a credential that is not our member's produces exactly one call —
    /// `/session` — and no catalogue read and no write.
    pub fn bind<'b>(&'b self, expected: &OpaqueId) -> Result<BoundMember<'b, 'a, C>, Binding> {
        let subject = self.authenticated_subject().map_err(Binding::Blocked)?;
        match BoundSubject::bind(expected.clone(), subject) {
            Ok(bound) => Ok(BoundMember { confinement: self, bound }),
            Err(mismatch) => Err(Binding::Mismatch(mismatch)),
        }
    }
}

impl<'b, 'a, C: CredentialSource> BoundMember<'b, 'a, C> {
    pub fn subject(&self) -> &RuntimeSubject {
        self.bound.authenticated()
    }
    pub fn expected(&self) -> &OpaqueId {
        self.bound.expected()
    }
    pub fn bound(&self) -> &BoundSubject {
        &self.bound
    }

    /// Read the catalogue once, and produce both what a plan needs and what the ledger takes.
    ///
    /// Splitting these into two calls would let the catalogue move between the plan and the record
    /// of what was planned against; one read closes that.
    pub fn read(
        &self,
        policy: &ConfinementPolicy,
        binding: &PolicyBinding,
        at: &ObservedAt,
    ) -> Reading {
        match self.catalogue() {
            Ok(CatalogueReading { states, locked, auto_approve }) => {
                let measurement = Measurement::from_assessment(
                    policy,
                    &self.bound,
                    &states,
                    &locked,
                    auto_approve,
                    at,
                );
                match measurement {
                    Ok(measurement) => {
                        let attempt = ConfinementAttempt::measured(
                            self.bound.clone(),
                            binding.clone(),
                            at.clone(),
                            measurement,
                        );
                        Reading::Measured { states, locked, auto_approve, attempt }
                    }
                    Err(why) => Reading::Blocked {
                        attempt: ConfinementAttempt::catalogue_blocked(
                            self.bound.clone(),
                            binding.clone(),
                            at.clone(),
                            Blocked::from(&why),
                        ),
                    },
                }
            }
            Err(why) => Reading::Blocked {
                attempt: ConfinementAttempt::catalogue_blocked(
                    self.bound.clone(),
                    binding.clone(),
                    at.clone(),
                    Blocked::from(&why),
                ),
            },
        }
    }

    /// Compare the member's catalogue against policy, as a verdict rather than a record.
    ///
    /// Kept beside [`Self::read`] because an operator report and a durable row want different
    /// shapes of the same measurement, and both must come from a bound member.
    pub fn observe(&self, policy: &ConfinementPolicy, at: &ObservedAt) -> Confinement {
        match self.catalogue() {
            Ok(CatalogueReading { states: tools, locked, auto_approve }) => {
                assess(policy, self.bound.expected(), &tools, &locked, auto_approve, at.as_str())
            }
            Err(why) => Confinement::Unverifiable(why),
        }
    }

    /// Carry out a confinement plan, as the member, verifying each write landed.
    ///
    /// # It never reports the outcome as confinement
    ///
    /// Applying returns what was *written*. Whether the member is confined is a separate
    /// question answered by [`Self::observe`] afterwards — the same rule §B-8 states for sealed
    /// membership: probed, never inferred. A writer that concluded `Verified` from its own
    /// successful writes would be certifying its intent rather than the runtime's state.
    ///
    /// # Why it stops at the first failure
    ///
    /// The plan is ordered so the work cannot be undone by the thing being confined while it is
    /// in progress. Continuing past a failure abandons that ordering — if approval-off failed,
    /// every denial that follows is reversible by the member with no human involved. Stopping
    /// leaves a partial state, which is reported rather than hidden, and re-running resumes:
    /// `plan` only contains what is not already at policy.
    pub fn apply(&self, plan: &Plan) -> Applied {
        let Some(secret) =
            self.confinement.credentials.resolve(&self.confinement.member_credential)
        else {
            return Applied::Stopped {
                applied: 0,
                at: None,
                why: BlockedReason::NoCredential {
                    system: format!(
                        "member credential at {}",
                        self.confinement.member_credential.handle()
                    ),
                },
            };
        };
        for (done, step) in plan.steps().iter().enumerate() {
            let (path, body, confirm): (String, String, fn(&str) -> bool) = match step {
                Step::ApprovalOff => (
                    "/api/webchat/v2/settings/tools".to_owned(),
                    r#"{"enabled":false}"#.to_owned(),
                    // The catalogue reports the setting back; anything else is unconfirmed.
                    |body| !confirms_auto_approve_on(body),
                ),
                Step::Disable { tool } => (
                    format!("/api/webchat/v2/settings/tools/{}", encode(tool)),
                    r#"{"state":"disabled"}"#.to_owned(),
                    confirms_disabled,
                ),
            };
            match self.confinement.admin.post_as_member(&path, &secret, &body) {
                Ok((200..=299, body)) if confirm(&body) => continue,
                // A 2xx that does not confirm the state is the failure mode IronClaw's own UI
                // guards against — it validates the persisted entry rather than the status.
                Ok((200..=299, _)) => {
                    return Applied::Stopped {
                        applied: done,
                        at: Some(step.clone()),
                        why: BlockedReason::Malformed {
                            detail: "the runtime accepted the write without confirming the state"
                                .into(),
                        },
                    };
                }
                Ok((401 | 403, _)) => {
                    return Applied::Stopped {
                        applied: done,
                        at: Some(step.clone()),
                        why: BlockedReason::CredentialRejected,
                    };
                }
                Ok((status, body)) => {
                    return Applied::Stopped {
                        applied: done,
                        at: Some(step.clone()),
                        why: BlockedReason::Malformed {
                            detail: format!("HTTP {status}: {}", first_line(&body)),
                        },
                    };
                }
                Err(why) => {
                    return Applied::Stopped { applied: done, at: Some(step.clone()), why };
                }
            }
        }
        Applied::Written { steps: plan.len() }
    }

    /// The member's catalogue as the runtime reports it: tool id to raw state, and approval.
    ///
    /// # There is no `recorded` parameter any more, and its absence is the point
    ///
    /// This used to take the member id lifecycle wrote down and check it against nothing, on the
    /// argument that the credential was the binding and the caller asserted it. The caller asserted
    /// it from an ambient environment variable. Now the binding is [`BoundSubject`], which
    /// [`MemberConfinement::bind`] can only produce by asking the runtime — so the subject of this
    /// read is a measured fact rather than an assertion, and it is already in `self`.
    ///
    /// Private, because every caller must come through a bound member.
    ///
    /// `locked` is read rather than assumed. It is the runtime's own answer to "can this row be
    /// written at all", computed from the capability's declared effects, and taking it from the
    /// catalogue is what keeps a future hard-floor capability from stalling a plan on a `400`
    /// (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 2). A row that omits the
    /// field is treated as unlocked: the union in `ironworks_lifecycle::confinement` still carries
    /// the two this build has measured a refusal from, so a silent omission cannot lose them.
    fn catalogue(&self) -> Result<CatalogueReading, BlockedReason> {
        let confinement = self.confinement;
        let Some(secret) = confinement.credentials.resolve(&confinement.member_credential) else {
            return Err(BlockedReason::NoCredential {
                system: format!(
                    "member {} (no credential at {})",
                    self.bound.expected(),
                    confinement.member_credential.handle()
                ),
            });
        };
        let (status, body) =
            confinement.admin.get_as_member("/api/webchat/v2/settings/tools", &secret)?;
        require_success(status)?;
        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                return Err(BlockedReason::Malformed { detail: e.to_string() });
            }
        };
        // A missing `entries` array is an unreadable catalogue, NOT an empty one. An empty
        // catalogue would assess as fully confined, which is the vacuous pass this must refuse.
        let Some(entries) = parsed.get("entries").and_then(|v| v.as_array()) else {
            return Err(BlockedReason::Malformed {
                detail: "the tool settings response carried no entries array".into(),
            });
        };
        let mut tools = std::collections::BTreeMap::new();
        let mut locked = std::collections::BTreeSet::new();
        let mut auto_approve = None;
        for entry in entries {
            let Some(key) = entry.get("key").and_then(|v| v.as_str()) else { continue };
            if key == "agent.auto_approve_tools" {
                auto_approve = entry.get("value").and_then(serde_json::Value::as_bool);
                continue;
            }
            let Some(name) = key.strip_prefix("tool.") else { continue };
            let Some(state) =
                entry.get("value").and_then(|v| v.get("state")).and_then(|v| v.as_str())
            else {
                return Err(BlockedReason::Malformed {
                    detail: format!("tool {name} carried no state"),
                });
            };
            if entry.get("value").and_then(|v| v.get("locked")).and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                locked.insert(name.to_owned());
            }
            tools.insert(name.to_owned(), state.to_owned());
        }
        // An absent approval setting is not `false`. Assuming the safer value would certify a
        // setting nobody read.
        let Some(auto_approve) = auto_approve else {
            return Err(BlockedReason::Malformed {
                detail: "the catalogue never reported agent.auto_approve_tools".into(),
            });
        };
        if tools.is_empty() {
            return Err(BlockedReason::Malformed {
                detail: "the catalogue listed no tools at all".into(),
            });
        }
        Ok(CatalogueReading { states: tools, locked, auto_approve })
    }
}
