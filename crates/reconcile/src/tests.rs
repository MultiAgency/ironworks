//! The Account Service adapter, against a stand-in service.
//!
//! Every assertion about a *negative* answer carries a positive control, because a reconciler
//! that reported nothing would satisfy "it never claimed absence" vacuously.

use super::*;
use ironworks_domain::{ChannelRef, CredentialPurpose, OrgId, ServiceId, Slug};
use ironworks_lifecycle::ConfinementAxis;
use ironworks_lifecycle::{
    Custody, ProvisionProgress, Recorded, ServingObservation, Stage, Verdict, holdings, inspect,
};
use ironworks_records::CredentialSource;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

fn spec() -> InstanceSpec {
    InstanceSpec {
        slug: Slug::parse("acme").unwrap(),
        org: OrgId::parse("acme-sales").unwrap(),
        service: ServiceId::parse("account-analysis").unwrap(),
        channel: ChannelRef::new("-100900001"),
    }
}

fn handle() -> CredentialRef {
    CredentialRef::new(CredentialPurpose::RecordScope, "env:ACME_ORG_TOKEN")
}

struct Creds;
impl CredentialSource for Creds {
    fn resolve(&self, _r: &CredentialRef) -> Option<ironworks_records::Secret> {
        Some(ironworks_records::Secret::new("dev-token-acme"))
    }
}

/// A stand-in Account Service that answers once and stops. Deliberately tiny: it exists to give
/// the adapter a real socket to talk to, not to be a service.
///
/// The `Content-Length` is computed rather than written by hand. Hand-counting it once produced a
/// truncated body — which the adapter reported as `Unverifiable(Malformed)`, correctly, but the
/// test was then measuring the fixture's arithmetic instead of the adapter's behaviour.
fn json(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn serve(response: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            loop {
                let mut h = String::new();
                match reader.read_line(&mut h) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if h == "\r\n" => break,
                    Ok(_) => {}
                }
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    base
}

fn own_org() -> String {
    json("200 OK", r#"{"org":"acme-sales","accounts":[]}"#)
}
fn other_org() -> String {
    json("200 OK", r#"{"org":"rival-co","accounts":[]}"#)
}
fn refused() -> String {
    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_owned()
}

/// The credential resolves to this instance's org, and it is the identifier we recorded creating.
#[test]
fn a_matching_org_registration_is_managed() {
    let book = Book::new(serve(own_org()), Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());

    assert_eq!(
        inventory.presence(Authority::OrgRegistration),
        Presence::Present { id: OpaqueId::new("acme-sales") }
    );

    let ours = Recorded {
        progress: ProvisionProgress::Provisioning { reached: Stage::OrgRegistered },
        ids: [(Authority::OrgRegistration, OpaqueId::new("acme-sales"))].into_iter().collect(),
    };
    let held = holdings(&ours, &inventory);
    let org = held.iter().find(|h| h.authority == Authority::OrgRegistration).unwrap();
    assert_eq!(org.custody, Custody::Managed);
}

/// A credential that authorises somebody else is reported **with the org it actually opens**, so
/// the disagreement reaches an operator instead of being flattened into "not what we expected".
#[test]
fn a_credential_for_another_org_is_reported_as_what_it_is() {
    let book = Book::new(serve(other_org()), Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());

    assert_eq!(
        inventory.presence(Authority::OrgRegistration),
        Presence::Present { id: OpaqueId::new("rival-co") },
        "the observation was rewritten to match what we hoped for"
    );

    // We recorded creating a credential for acme-sales. What is out there opens rival-co, so it
    // is not ours.
    let ours = Recorded {
        progress: ProvisionProgress::Provisioning { reached: Stage::OrgRegistered },
        ids: [(Authority::OrgRegistration, OpaqueId::new("acme-sales"))].into_iter().collect(),
    };
    let held = holdings(&ours, &inventory);
    let org = held.iter().find(|h| h.authority == Authority::OrgRegistration).unwrap();
    assert_eq!(org.custody, Custody::KnownUnmanaged);
}

/// **The asymmetry, asserted.** A refused credential is evidence about the credential. Reading it
/// as "no registration exists" is the same mistake as reading an empty journal that way, and it
/// is the mistake that mints a second live credential.
#[test]
fn a_refused_credential_is_unverifiable_never_absent() {
    let book = Book::new(serve(refused()), Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());

    assert_eq!(
        inventory.presence(Authority::OrgRegistration),
        Presence::Unverifiable(BlockedReason::CredentialRejected)
    );
    assert_ne!(inventory.presence(Authority::OrgRegistration), Presence::Absent);
}

/// An Account Service that cannot be reached blocks the whole inventory. Not even the one
/// authority it owns may be reported, because nothing was learned.
#[test]
fn an_unreachable_account_service_blocks() {
    // A port nothing is listening on.
    let book = Book::new("http://127.0.0.1:1", Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());

    assert!(matches!(inventory, AuthorityInventory::Blocked(BlockedReason::Unreachable { .. })));
    for a in Authority::ALL {
        assert!(!inventory.presence(a).is_measured(), "{a:?} was treated as measured");
    }

    let report = inspect(
        &spec(),
        &Recorded::from_progress(ProvisionProgress::NotStarted),
        &inventory,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
}

/// **P2 through a real adapter.** Nothing is recorded locally and a credential authorises the org.
/// Whatever else happens, the org registration is never reported absent.
#[test]
fn an_empty_journal_with_a_live_credential_is_never_absent() {
    let book = Book::new(serve(own_org()), Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());
    let nothing_recorded = Recorded::from_progress(ProvisionProgress::NotStarted);

    assert_ne!(inventory.presence(Authority::OrgRegistration), Presence::Absent);
    let held = holdings(&nothing_recorded, &inventory);
    let org = held.iter().find(|h| h.authority == Authority::OrgRegistration).unwrap();
    assert_eq!(
        org.custody,
        Custody::KnownUnmanaged,
        "authority exists that we have no record of; it must be reported, not adopted"
    );

    let report = inspect(
        &spec(),
        &nothing_recorded,
        &inventory,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Blocked);
    assert!(report.findings.iter().any(|f| f.note.contains("no record of creating it")));
}

/// The adapter reports one authority and stays silent about four. Silence reads as unverifiable,
/// so a single reconciler cannot produce a clean overall verdict — which is correct, and is why
/// this is the *first* reconciler rather than the only one.
#[test]
fn it_speaks_only_for_the_authority_it_owns() {
    let book = Book::new(serve(own_org()), Creds);
    let inventory = AccountService::new(&book, handle()).observe(&spec());

    let AuthorityInventory::Reconciled(facts) = &inventory else { panic!("expected facts") };
    assert_eq!(facts.len(), 1, "it claimed something about a system it never asked");
    assert_eq!(facts[0].authority, Authority::OrgRegistration);

    for a in Authority::ALL {
        if a != Authority::OrgRegistration {
            assert_eq!(
                inventory.presence(a),
                Presence::Unverifiable(BlockedReason::NotObserved),
                "{a:?} was answered for by a service that has never heard of it"
            );
        }
    }
}
