//! Adapter tests against an in-process stub store.
//!
//! A stub rather than a mock of our own client: the point is to exercise the real HTTP path,
//! real parsing, and the real scope guard. No external service, no fixtures on disk, no network.

use super::*;
use ironworks_domain::{
    Ask, CapabilityProfile, Composition, ContextBinding, CredentialPurpose, Degradation, Guidance,
    ModelPin, ModelSelection, PartRef, PartSource, RecordsState, RuntimeIdentity, ScopeSource,
    ServiceDefinition, ServiceId, ServiceInstance, Slug, Unverified, VerifyError,
};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

// ── an in-process stub record store ──────────────────────────────────────────────────────

/// Serve canned responses keyed by path prefix. Returns the base URL.
fn stub(routes: Vec<(&'static str, u16, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().take(16) {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            // Drain headers so the client sees a well-formed exchange.
            loop {
                let mut h = String::new();
                match reader.read_line(&mut h) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if h == "\r\n" || h == "\n" => break,
                    Ok(_) => {}
                }
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (status, body) = routes
                .iter()
                .find(|(p, _, _)| path.starts_with(p))
                .map(|(_, s, b)| (*s, b.clone()))
                .unwrap_or((404, r#"{"error":"no route"}"#.to_string()));
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    base
}

fn catalog_body(org: &str) -> String {
    format!(
        r#"{{"source":"multiagency","org":"{org}","count":2,"accounts":[
            {{"account_id":"BW-010","name":"Blackwater","updated_at":"2026-08-01T00:00:00+00:00"}},
            {{"account_id":"CD-002","name":"Cobalt","updated_at":null}}]}}"#
    )
}

fn record_body(org: &str) -> String {
    format!(
        r#"{{"source":"multiagency","org":"{org}","record_id":"BW-010","found":true,
            "account":{{"name":"Blackwater","budget":"$60k"}},"contacts":[],"activities":[]}}"#
    )
}

struct Creds;
impl CredentialSource for Creds {
    fn resolve(&self, _: &CredentialRef) -> Option<Secret> {
        Some(Secret::new("token"))
    }
}
struct NoCreds;
impl CredentialSource for NoCreds {
    fn resolve(&self, _: &CredentialRef) -> Option<Secret> {
        None
    }
}

fn scope_credential() -> CredentialRef {
    CredentialRef::new(CredentialPurpose::RecordScope, "env:TEST")
}

// ── instance construction, so a RecordScope can exist at all ─────────────────────────────

struct Parts(BTreeMap<String, String>);
impl PartSource for Parts {
    fn read(&self, p: &PartRef) -> Option<&str> {
        self.0.get(p.as_str()).map(String::as_str)
    }
}
fn parts() -> Parts {
    Parts(BTreeMap::from([
        ("p".to_string(), "PART".to_string()),
        ("t".to_string(), "TAIL".to_string()),
    ]))
}

fn instance(slug: &str, claimed: Option<&str>) -> ServiceInstance<Unverified> {
    let definition = ServiceDefinition::new(
        ServiceId::parse("s").unwrap(),
        1,
        "r",
        Composition::new(vec![PartRef::new("p")], "# H", PartRef::new("t")).unwrap(),
        CapabilityProfile::READ_ONLY_RECORDS,
        None,
    )
    .unwrap();
    let guidance = Guidance::parse(
        &format!(
            "<!-- tenant-guidance v1 slug: {slug} service: s -->\n\n{}",
            "content ".repeat(80)
        ),
        slug,
        "s",
    )
    .unwrap();
    ServiceInstance::new(ironworks_domain::InstanceParts {
        slug: Slug::parse(slug).unwrap(),
        definition,
        guidance,
        runtime_identity: RuntimeIdentity::parse("m").unwrap(),
        runtime_credential: CredentialRef::new(CredentialPurpose::RuntimeMember, "env:RUNTIME"),
        context_binding: ContextBinding::new(scope_credential()).unwrap(),
        model: ModelSelection::from_pin(&ModelPin::parse("M # c").unwrap()),
        requested_capabilities: CapabilityProfile::READ_ONLY_RECORDS,
        claimed_org: claimed.map(|c| OrgId::parse(c).unwrap()),
        parts: &parts(),
    })
    .unwrap()
}

// ── scope resolution is a control: it fails closed ───────────────────────────────────────

#[test]
fn an_unreachable_store_yields_no_verified_instance() {
    // Port 1 on loopback refuses immediately.
    let book = Book::new("http://127.0.0.1:1", Creds);
    let err = instance("acme", None).verify(&book).unwrap_err();
    assert!(matches!(
        err,
        VerifyError::Unresolved { source: ScopeFailure::Unreachable { .. }, .. }
    ));
}

#[test]
fn a_refused_credential_yields_no_verified_instance() {
    let base = stub(vec![("/list_accounts", 401, "Invalid or missing auth token".into())]);
    let book = Book::new(base, Creds);
    assert!(matches!(
        instance("acme", None).verify(&book).unwrap_err(),
        VerifyError::Unresolved { source: ScopeFailure::Unauthorized, .. }
    ));
}

#[test]
fn an_unreadable_answer_yields_no_verified_instance() {
    let base = stub(vec![("/list_accounts", 200, "not json at all".into())]);
    let book = Book::new(base, Creds);
    assert!(matches!(
        instance("acme", None).verify(&book).unwrap_err(),
        VerifyError::Unresolved { source: ScopeFailure::Malformed { .. }, .. }
    ));
}

#[test]
fn a_missing_credential_is_a_control_failure_not_a_silent_empty_book() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base, NoCreds);
    assert!(instance("acme", None).verify(&book).is_err());
}

// ── the store is authoritative over the claim ────────────────────────────────────────────

#[test]
fn verification_carries_evidence_naming_its_source() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base.clone(), Creds);
    let v = instance("acme", None).verify(&book).unwrap();
    assert_eq!(v.org().as_str(), "acme-sales");
    match v.scope_evidence().source() {
        ScopeSource::RecordStore { endpoint } => assert_eq!(*endpoint, base),
    }
}

#[test]
fn a_mismatched_claim_is_overridden_by_the_store_and_reported() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base, Creds);
    // The config claims one org; the store resolves another. The store wins, and the
    // disagreement is visible rather than silent — a config file describing an instance it no
    // longer scopes is something an operator must hear about.
    let v = instance("acme", Some("stale-claim")).verify(&book).unwrap();
    assert_eq!(v.org().as_str(), "acme-sales");
    assert!(!v.claim_matched());
}

#[test]
fn a_matching_claim_is_reported_as_matching() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base, Creds);
    assert!(instance("acme", Some("acme-sales")).verify(&book).unwrap().claim_matched());
}

// ── cross-tenant: scope drift is refused, not returned ───────────────────────────────────

#[test]
fn a_catalog_answered_for_another_org_is_refused() {
    // First call resolves the scope; a later call answers for a different org — a credential
    // re-pointed mid-flight. Returning those rows would deliver one tenant's book to another.
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base, Creds);
    let verified = instance("acme", None).verify(&book).unwrap();
    let scope = verified.record_scope();

    let drifted = stub(vec![("/list_accounts", 200, catalog_body("rival-sales"))]);
    let other = Book::new(drifted, Creds);
    let err = other.catalog(&scope).unwrap_err();
    match err {
        BookFailure::ScopeDrift { asked, answered } => {
            assert_eq!(asked.as_str(), "acme-sales");
            assert_eq!(answered.as_str(), "rival-sales");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_record_answered_for_another_org_is_refused() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let scope = instance("acme", None).verify(&Book::new(base, Creds)).unwrap().record_scope();

    let drifted = stub(vec![("/get_account_context", 200, record_body("rival-sales"))]);
    let other = Book::new(drifted, Creds);
    let err = other.record(&scope, &AccountId::parse("BW-010").unwrap()).unwrap_err();
    assert!(matches!(err, BookFailure::ScopeDrift { .. }));
}

// ── retrieval is a data source: it degrades ──────────────────────────────────────────────

#[test]
fn a_catalog_read_succeeds_and_carries_versions() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let book = Book::new(base, Creds);
    let scope = instance("acme", None).verify(&book).unwrap().record_scope();
    let catalog = book.catalog(&scope).unwrap();
    assert_eq!(catalog.org().as_str(), "acme-sales");
    assert_eq!(catalog.len(), 2);
    assert!(catalog.entries()[0].updated_at.is_some());
    assert!(catalog.entries()[1].updated_at.is_none(), "an absent version stays absent");
}

#[test]
fn an_unusable_catalog_row_is_dropped_rather_than_failing_the_read() {
    let body = r#"{"org":"acme-sales","accounts":[
        {"account_id":"OK-1","name":"Fine"},
        {"account_id":"BAD-1"},
        {"account_id":"","name":"No id"}]}"#;
    let base = stub(vec![("/list_accounts", 200, body.into())]);
    let book = Book::new(base, Creds);
    let scope = instance("acme", None).verify(&book).unwrap().record_scope();
    let catalog = book.catalog(&scope).unwrap();
    assert_eq!(catalog.len(), 1, "one bad row must not blind the analyst to the good ones");
    // Dropping is right; dropping *silently* is not. `Degradation::Partial { unreadable }` exists
    // so the model can be told "two are missing" — its own doc argues the model can act on that
    // and cannot act on "some". Without a count here the caller reports `Complete` over a book
    // that quietly shrank, so a store schema change would take records away from every tenant
    // with nothing anywhere reporting it.
    assert_eq!(catalog.dropped(), 2, "the rows that were dropped were not counted");
}

#[test]
fn a_missing_record_is_none_rather_than_an_error() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let scope = instance("acme", None).verify(&Book::new(base, Creds)).unwrap().record_scope();
    let store = stub(vec![("/get_account_context", 404, r#"{"found":false}"#.into())]);
    let book = Book::new(store, Creds);
    assert_eq!(book.record(&scope, &AccountId::parse("NOPE").unwrap()).unwrap(), None);
}

/// The rule, end to end: a store that cannot be read still serves a turn, with the model told.
#[test]
fn a_degraded_catalog_still_builds_a_turn() {
    let base = stub(vec![("/list_accounts", 200, catalog_body("acme-sales"))]);
    let verified = instance("acme", None).verify(&Book::new(base, Creds)).unwrap();

    // The store goes away after verification.
    let down = Book::new("http://127.0.0.1:1", Creds);
    let failure = down.catalog(&verified.record_scope()).unwrap_err();
    assert!(matches!(failure, BookFailure::Unreachable { .. }));

    // A data source degrades: the turn is still built, and the state says so.
    let state = RecordsState::Degraded(Degradation::Unavailable);
    let turn = verified
        .build_turn(
            &parts(),
            &Ask {
                question: "what is outstanding?",
                records: &[],
                records_state: &state,
                retrieved_at: &ironworks_domain::record::Timestamp::parse(
                    "2026-08-27T12:00:00+00:00",
                )
                .unwrap(),
                speaker: None,
                fact_fields: None,
            },
            None,
        )
        .unwrap();
    assert!(!turn.input().records().has_records());
    assert!(!turn.instructions().is_empty(), "the alloy is still composed");
    assert_eq!(*turn.input().records(), state);
}

/// And the asymmetry, stated as one test: the same outage before verification serves nothing.
#[test]
fn the_same_outage_before_verification_serves_nothing() {
    let down = Book::new("http://127.0.0.1:1", Creds);
    assert!(
        instance("acme", None).verify(&down).is_err(),
        "a control that cannot verify has established nothing"
    );
}
