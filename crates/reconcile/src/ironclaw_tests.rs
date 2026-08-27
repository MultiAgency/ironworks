//! The IronClaw member reconciler, per-instance and fleet-wide.
//!
//! Every assertion about absence or about "nothing unaccounted" carries a positive control. A
//! fleet inventory that found nothing because it never enumerated satisfies "no strays" perfectly,
//! and that is the exact score the reference's own tool would have produced past 100 members.

use super::*;
use crate::ironclaw::{Admin, FleetInventory, IronclawMember, unaccounted_members};
use ironworks_domain::{ChannelRef, CredentialPurpose, OrgId, ServiceId, Slug};
use ironworks_lifecycle::ConfinementAxis;
use ironworks_lifecycle::{
    Custody, ProvisionProgress, Recorded, ServingObservation, Stage, Verdict, holdings, inspect,
};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn spec() -> InstanceSpec {
    InstanceSpec {
        slug: Slug::parse("acme").unwrap(),
        org: OrgId::parse("acme-sales").unwrap(),
        service: ServiceId::parse("account-analysis").unwrap(),
        channel: ChannelRef::new("-100900001"),
    }
}

fn handle() -> CredentialRef {
    CredentialRef::new(CredentialPurpose::RuntimeMember, "env:PROBE_OPERATOR_TOKEN")
}

/// A stand-in admin surface. Routes on the request line, so pagination is exercised end to end
/// rather than simulated.
struct FakeAdmin {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

/// One canned reply, keyed by the path prefix it answers.
type Route = (&'static str, u16, String);

fn admin(routes: Vec<Route>) -> FakeAdmin {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            loop {
                let mut h = String::new();
                match reader.read_line(&mut h) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if h == "\r\n" => break,
                    Ok(_) => {}
                }
            }
            let target = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            log.lock().unwrap().push(target.clone());
            // Longest matching prefix wins, so `/users?cursor=p2` beats `/users`.
            let hit = routes
                .iter()
                .filter(|(p, _, _)| target.starts_with(p))
                .max_by_key(|(p, _, _)| p.len());
            let (status, body) = match hit {
                Some((_, s, b)) => (*s, b.clone()),
                None => (404, String::new()),
            };
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    });
    FakeAdmin { base, requests }
}

/// The admin surface needs an operator token. Set once per process; the value is a fixture.
fn with_token() -> CredentialRef {
    // SAFETY-adjacent note: `set_var` is unavailable under `unsafe_code = "forbid"`, so the
    // fixture supplies the token directly rather than through the environment.
    handle()
}

fn client(f: &FakeAdmin) -> Admin {
    let mut admin = Admin::new(&f.base, &with_token());
    admin.set_token_for_test(OperatorToken::new("operator-fixture-token"));
    admin
}

fn page(users: &[&str], next: Option<&str>) -> String {
    let list: Vec<String> = users
        .iter()
        .map(|u| format!("{{\"user_id\":\"{u}\",\"display_name\":\"whoever\"}}"))
        .collect();
    match next {
        Some(c) => format!("{{\"users\":[{}],\"next_cursor\":\"{c}\"}}", list.join(",")),
        None => format!("{{\"users\":[{}]}}", list.join(",")),
    }
}

// ── per-instance ────────────────────────────────────────────────────────────────────────────

#[test]
fn a_recorded_member_that_exists_is_managed() {
    let f = admin(vec![("/api/webchat/v2/admin/users/user_7f3a", 200, "{}".into())]);
    let recorded_id = OpaqueId::new("user_7f3a");
    let inventory = IronclawMember::new(&client(&f), Some(recorded_id.clone())).observe(&spec());

    assert_eq!(
        inventory.presence(Authority::Member),
        Presence::Present { id: recorded_id.clone() }
    );

    let ours = Recorded {
        progress: ProvisionProgress::Provisioning { reached: Stage::MemberMinted },
        ids: [(Authority::Member, recorded_id)].into_iter().collect(),
    };
    let member =
        holdings(&ours, &inventory).into_iter().find(|h| h.authority == Authority::Member).unwrap();
    assert_eq!(member.custody, Custody::Managed);
}

/// An authoritative 404 for the id we recorded. That is measured absence — and if we recorded
/// creating it, the world and our records disagree, which is a FAIL rather than a BLOCKED.
#[test]
fn an_authoritative_404_for_the_recorded_member_is_absent_and_contradicts() {
    let f = admin(vec![]); // every path 404s
    let recorded_id = OpaqueId::new("user_gone");
    let inventory = IronclawMember::new(&client(&f), Some(recorded_id.clone())).observe(&spec());
    assert_eq!(inventory.presence(Authority::Member), Presence::Absent);

    let ours = Recorded {
        progress: ProvisionProgress::Provisioning { reached: Stage::MemberMinted },
        ids: [(Authority::Member, recorded_id)].into_iter().collect(),
    };
    let report = inspect(
        &spec(),
        &ours,
        &inventory,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_eq!(report.verdict, Verdict::Fail);
    assert!(report.findings.iter().any(|x| x.note.contains("disagree")));

    // Positive control: the identical 404 with nothing recorded is not a contradiction, because
    // there is nothing to contradict.
    let nothing = Recorded::from_progress(ProvisionProgress::NotStarted);
    let ok = inspect(
        &spec(),
        &nothing,
        &inventory,
        &ServingObservation::Unknown,
        &ConfinementAxis::NotRequested,
    );
    assert_ne!(ok.verdict, Verdict::Fail);
}

/// No recorded id is no subject, so there is no question to ask. Never `Managed`, and never
/// `Absent` — a member may well exist and this path cannot see it.
#[test]
fn no_recorded_id_yields_no_recorded_identity_and_asks_nothing() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&["user_stray"], None))]);
    let inventory = IronclawMember::new(&client(&f), None).observe(&spec());

    assert_eq!(
        inventory.presence(Authority::Member),
        Presence::Unverifiable(BlockedReason::NoRecordedIdentity)
    );
    assert!(f.requests.lock().unwrap().is_empty(), "it asked a question with no subject");

    let reached =
        Recorded::from_progress(ProvisionProgress::Provisioning { reached: Stage::MemberMinted });
    let member = holdings(&reached, &inventory)
        .into_iter()
        .find(|h| h.authority == Authority::Member)
        .unwrap();
    assert_ne!(member.custody, Custody::Managed, "a stage reached is not custody of anything");
}

/// **The attribution refusal.** A member whose display name is this tenant's is still not this
/// tenant's member. The reconciler never looks, and the proof is that it issues no listing at all
/// when it has an id to check.
#[test]
fn a_member_is_never_attributed_by_display_name() {
    // The stray's display name is this tenant's slug. A reconciler that looked at labels would
    // pick it; this one never looks.
    let listing = "{\"users\":[{\"user_id\":\"user_stray\",\"display_name\":\"acme\"},\
                   {\"user_id\":\"user_7f3a\",\"display_name\":\"someone else\"}]}"
        .to_owned();
    let f = admin(vec![
        ("/api/webchat/v2/admin/users/user_7f3a", 200, "{}".into()),
        ("/api/webchat/v2/admin/users", 200, listing),
    ]);
    let inventory =
        IronclawMember::new(&client(&f), Some(OpaqueId::new("user_7f3a"))).observe(&spec());

    // It reports the id it was given, not the one whose label matches the slug.
    assert_eq!(
        inventory.presence(Authority::Member),
        Presence::Present { id: OpaqueId::new("user_7f3a") }
    );
    let asked = f.requests.lock().unwrap().clone();
    assert!(
        asked.iter().all(|r| r.contains("user_7f3a")),
        "it enumerated the directory looking for a name match: {asked:?}"
    );
}

#[test]
fn an_unusable_admin_endpoint_is_unverifiable_never_absent() {
    for (status, expected) in [
        (401, BlockedReason::CredentialRejected),
        (500, BlockedReason::Malformed { detail: "HTTP 500".into() }),
    ] {
        let f = admin(vec![("/api/webchat/v2/admin/users/u", status, "{}".into())]);
        let inventory = IronclawMember::new(&client(&f), Some(OpaqueId::new("u"))).observe(&spec());
        assert_eq!(inventory.presence(Authority::Member), Presence::Unverifiable(expected));
        assert_ne!(inventory.presence(Authority::Member), Presence::Absent);
    }

    // Unreachable: nothing listening.
    let mut unreachable = Admin::new("http://127.0.0.1:1", &handle());
    unreachable.set_token_for_test(OperatorToken::new("t"));
    let inventory = IronclawMember::new(&unreachable, Some(OpaqueId::new("u"))).observe(&spec());
    assert!(matches!(
        inventory.presence(Authority::Member),
        Presence::Unverifiable(BlockedReason::Unreachable { .. })
    ));
}

// ── fleet ───────────────────────────────────────────────────────────────────────────────────

/// Enumeration follows `next_cursor` to the end, and the strays are on page two — which is where
/// a single-request implementation would have found none.
#[test]
fn exhaustive_enumeration_finds_unaccounted_members_on_a_later_page() {
    let f = admin(vec![
        ("/api/webchat/v2/admin/users?limit=200&cursor=p2", 200, page(&["user_x", "user_y"], None)),
        ("/api/webchat/v2/admin/users?limit=200", 200, page(&["user_7f3a"], Some("p2"))),
    ]);
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_7f3a")].into_iter().collect();

    let inventory = unaccounted_members(&client(&f), &accounted);
    let FleetInventory::Complete { observed, unaccounted, dangling } = inventory else {
        panic!("expected a complete enumeration, got {inventory:?}")
    };
    assert_eq!(observed, 3);
    assert!(dangling.is_empty(), "the recorded member was on page one");
    assert_eq!(
        unaccounted,
        [OpaqueId::new("user_x"), OpaqueId::new("user_y")].into_iter().collect::<BTreeSet<_>>(),
        "every stray must be surfaced, not one picked"
    );
    assert_eq!(f.requests.lock().unwrap().len(), 2, "it stopped before the terminal page");
}

/// **The defect the reference has.** A second page that cannot be fetched must never produce
/// "0 unknown". A clean score computed from a truncated enumeration is the worst possible output.
#[test]
fn a_failed_later_page_blocks_and_never_reports_zero_unknown() {
    let f = admin(vec![
        ("/api/webchat/v2/admin/users?limit=200&cursor=p2", 500, "{}".into()),
        ("/api/webchat/v2/admin/users?limit=200", 200, page(&["user_7f3a"], Some("p2"))),
    ]);
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_7f3a")].into_iter().collect();

    let inventory = unaccounted_members(&client(&f), &accounted);
    assert!(!inventory.is_complete(), "a truncated enumeration was reported as an answer");
    assert!(matches!(inventory, FleetInventory::Blocked(_)));

    // Positive control: the identical first page, with a reachable second page holding nothing
    // new, IS complete and reports zero. So the block above is about the failure, not about the
    // fixture never having anything to find.
    let ok = admin(vec![
        ("/api/webchat/v2/admin/users?limit=200&cursor=p2", 200, page(&[], None)),
        ("/api/webchat/v2/admin/users?limit=200", 200, page(&["user_7f3a"], Some("p2"))),
    ]);
    let inventory = unaccounted_members(&client(&ok), &accounted);
    assert_eq!(
        inventory,
        FleetInventory::Complete {
            observed: 1,
            unaccounted: BTreeSet::new(),
            dangling: BTreeSet::new()
        }
    );
}

/// Many strays, all reported. The reference found 41; nothing here selects, samples or caps.
#[test]
fn every_stray_member_is_surfaced() {
    let strays: Vec<String> = (0..41).map(|n| format!("user_stray_{n:02}")).collect();
    let refs: Vec<&str> = strays.iter().map(String::as_str).collect();
    let mut all = vec!["user_7f3a"];
    all.extend(refs.iter().copied());
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&all, None))]);
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_7f3a")].into_iter().collect();

    let FleetInventory::Complete { observed, unaccounted, .. } =
        unaccounted_members(&client(&f), &accounted)
    else {
        panic!("expected a complete enumeration")
    };
    assert_eq!(observed, 42);
    assert_eq!(unaccounted.len(), 41, "the 41 must all be reported");
}

/// A missing `users` field and an empty one are different answers, and only one of them is an
/// enumeration. The reference could not tell them apart and failed closed on both.
#[test]
fn a_response_with_no_users_field_is_blocked_but_an_empty_list_is_an_answer() {
    // A non-empty accounted set, so this measures the ENUMERATION rather than tripping the
    // nothing-to-reconcile rule.
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_recorded")].into_iter().collect();

    let missing = admin(vec![("/api/webchat/v2/admin/users", 200, "{\"ok\":true}".into())]);
    assert!(matches!(
        unaccounted_members(&client(&missing), &accounted),
        FleetInventory::Blocked(BlockedReason::Malformed { .. })
    ));

    // An instance genuinely holding no members IS a complete enumeration — and the recorded id
    // that is not there shows up as dangling, which is the other half working.
    let empty = admin(vec![("/api/webchat/v2/admin/users", 200, page(&[], None))]);
    assert_eq!(
        unaccounted_members(&client(&empty), &accounted),
        FleetInventory::Complete {
            observed: 0,
            unaccounted: BTreeSet::new(),
            dangling: [OpaqueId::new("user_recorded")].into_iter().collect()
        }
    );
}

/// A cursor that repeats would enumerate forever. Fail closed rather than spin, and never report
/// the partial set gathered on the way.
#[test]
fn a_repeating_cursor_blocks_rather_than_looping() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&["user_a"], Some("same")))]);
    assert!(matches!(
        unaccounted_members(&client(&f), &BTreeSet::new()),
        FleetInventory::Blocked(BlockedReason::Malformed { .. })
    ));
}

/// The fleet path compares identifiers and can reach nothing else. Asserted on the output: a
/// display name present in every fixture record never appears in what it returns.
#[test]
fn the_fleet_inventory_carries_identifiers_and_nothing_else() {
    let f = admin(vec![(
        "/api/webchat/v2/admin/users",
        200,
        "{\"users\":[{\"user_id\":\"user_x\",\"display_name\":\"Blackwater Instruments\",\
          \"email\":\"ops@blackwater.example\"}]}"
            .into(),
    )]);
    // One accounted id so the comparison is real; `user_x` is still unaccounted.
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_recorded")].into_iter().collect();
    let FleetInventory::Complete { unaccounted, .. } = unaccounted_members(&client(&f), &accounted)
    else {
        panic!("expected a complete enumeration")
    };
    let rendered = format!("{unaccounted:?}");
    assert!(rendered.contains("user_x"));
    for leaked in ["Blackwater", "blackwater.example", "display_name", "email"] {
        assert!(!rendered.contains(leaked), "{leaked} reached the fleet inventory");
    }
}

#[test]
fn an_operator_token_never_renders() {
    let token = OperatorToken::new("super-secret-operator-token");
    assert_eq!(format!("{token:?}"), "OperatorToken(redacted)");
    assert!(!format!("{token:?}").contains("super-secret"));
}

/// **The vacuous PASS, found by running it.** An empty directory against a store holding one
/// recorded id reported "every member on the instance is accounted for" — true, and a clean bill
/// of health that had only looked one way.
#[test]
fn a_recorded_member_missing_from_the_directory_is_reported_as_dangling() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&[], None))]);
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_7f3a")].into_iter().collect();

    let FleetInventory::Complete { observed, unaccounted, dangling } =
        unaccounted_members(&client(&f), &accounted)
    else {
        panic!("expected a complete enumeration")
    };
    assert_eq!(observed, 0);
    assert!(unaccounted.is_empty(), "nothing on the instance is unaccounted — that half was right");
    assert_eq!(
        dangling,
        [OpaqueId::new("user_7f3a")].into_iter().collect::<BTreeSet<_>>(),
        "a record pointing at a member that does not exist went unreported"
    );

    // Positive control: with the member present, both directions are clean.
    let ok = admin(vec![("/api/webchat/v2/admin/users", 200, page(&["user_7f3a"], None))]);
    assert_eq!(
        unaccounted_members(&client(&ok), &accounted),
        FleetInventory::Complete {
            observed: 1,
            unaccounted: BTreeSet::new(),
            dangling: BTreeSet::new()
        }
    );
}

/// **Zero accounted, zero observed.** Both sets empty, so they "agree" — and the agreement is
/// arithmetic. Identically true of a `--store` pointing anywhere, which is the likelier cause.
#[test]
fn nothing_accounted_and_nothing_observed_is_blocked_not_passing() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&[], None))]);
    let inventory = unaccounted_members(&client(&f), &BTreeSet::new());

    assert_eq!(
        inventory,
        FleetInventory::NothingToReconcile { observed: 0, unaccounted: BTreeSet::new() }
    );
    assert!(!inventory.is_complete(), "an empty comparison was readable as an answer");
}

/// **Zero accounted, members present.** The strays are still surfaced — that fact is real and an
/// operator needs it — but it cannot be scored as agreement either.
#[test]
fn nothing_accounted_with_members_present_still_surfaces_them_and_blocks() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&["user_a", "user_b"], None))]);
    let inventory = unaccounted_members(&client(&f), &BTreeSet::new());

    let FleetInventory::NothingToReconcile { observed, unaccounted } = inventory else {
        panic!("expected NothingToReconcile, got {inventory:?}")
    };
    assert_eq!(observed, 2);
    assert_eq!(
        unaccounted,
        [OpaqueId::new("user_a"), OpaqueId::new("user_b")].into_iter().collect::<BTreeSet<_>>(),
        "members went unreported because there was nothing to compare them against"
    );
}

/// The positive control for both: one accounted id and an exhaustive matching observation still
/// passes, so the blocks above are about the empty set rather than about nothing ever passing.
#[test]
fn one_accounted_member_matching_exhaustively_still_passes() {
    let f = admin(vec![("/api/webchat/v2/admin/users", 200, page(&["user_7f3a"], None))]);
    let accounted: BTreeSet<OpaqueId> = [OpaqueId::new("user_7f3a")].into_iter().collect();

    let inventory = unaccounted_members(&client(&f), &accounted);
    assert!(inventory.is_complete());
    assert_eq!(
        inventory,
        FleetInventory::Complete {
            observed: 1,
            unaccounted: BTreeSet::new(),
            dangling: BTreeSet::new()
        }
    );
}

// ── binding: the subject must be measured before anything reads settings ────────────────────
//
// The design's whole point is that a confinement verdict names a subject it authenticated. These
// assert the mechanism, and the mismatch case asserts it against the **request log** rather than
// against state equality: "the catalogue is unchanged" would prove no write, and says nothing at
// all about whether a read happened.

use crate::ironclaw::{Binding, MemberConfinement, Reading};
use ironworks_domain::CredentialRef;
use ironworks_lifecycle::{Blocked, ConfinementPolicy, ObservedAt, PolicyBinding};
use ironworks_records::{CredentialSource, Secret};

/// A credential source that resolves one handle. The value is a fixture; the environment cannot be
/// mutated under `unsafe_code = "forbid"`.
struct FixtureCredential;

impl CredentialSource for FixtureCredential {
    fn resolve(&self, _reference: &CredentialRef) -> Option<Secret> {
        Some(Secret::new("member-fixture-token"))
    }
}

/// A source that resolves nothing, for the no-credential path.
struct NoCredential;

impl CredentialSource for NoCredential {
    fn resolve(&self, _reference: &CredentialRef) -> Option<Secret> {
        None
    }
}

const SESSION: &str = "/api/webchat/v2/session";
const SETTINGS: &str = "/api/webchat/v2/settings/tools";

fn member_handle() -> CredentialRef {
    CredentialRef::new(CredentialPurpose::RuntimeMember, "env:IRONCLAW_MEMBER_TOKEN")
}

fn session_body(tenant: &str, user: &str) -> String {
    format!("{{\"tenant_id\":\"{tenant}\",\"user_id\":\"{user}\",\"capabilities\":{{}}}}")
}

/// A catalogue with one disabled tool and approval off — the shape a confined member reports.
fn catalogue_body() -> String {
    r#"{"entries":[
        {"key":"agent.auto_approve_tools","value":false},
        {"key":"tool.builtin.http","value":{"state":"disabled"}}
    ]}"#
    .to_owned()
}

fn requests(f: &FakeAdmin) -> Vec<String> {
    f.requests.lock().expect("log").clone()
}

fn definition() -> ironworks_domain::ServiceDefinition {
    use ironworks_domain::composition::{Composition, PartRef};
    ironworks_domain::ServiceDefinition::new(
        ServiceId::parse("account-analysis").expect("id"),
        1,
        "read one organization's own account records and answer from them",
        Composition::new(vec![PartRef::new("personas/COORDINATOR.md")], "# G", PartRef::new("t"))
            .expect("composition"),
        ironworks_domain::CapabilityProfile::READ_ONLY_RECORDS,
        None,
    )
    .expect("definition")
}

/// The same shape, with the `locked` field the runtime actually sends on every row.
///
/// Measured at the pin: two rows report `locked: true` and the rest `locked: false`
/// (`docs/experiments/0037-what-the-seam-never-asked-for.md` check 2). One of each here, plus a
/// row that omits the field, because all three reach the reader.
fn catalogue_body_with_locked() -> String {
    r#"{"entries":[
        {"key":"agent.auto_approve_tools","value":false},
        {"key":"tool.builtin.http","value":{"state":"disabled","locked":false}},
        {"key":"tool.builtin.operator_config_set_auto_approve","value":{"state":"ask_each_time","locked":true}},
        {"key":"tool.builtin.time","value":{"state":"disabled"}}
    ]}"#
    .to_owned()
}

#[test]
fn the_reader_carries_the_locked_rows_the_catalogue_reports() {
    // `locked` is the runtime's own answer to "can this row be written at all". Reading it is what
    // keeps a future hard-floor capability from stalling a plan on a 400 it cannot act on.
    let f = admin(vec![
        (SESSION, 200, session_body("acme", "user_7f3a")),
        (SETTINGS, 200, catalogue_body_with_locked()),
    ]);
    let admin_client = client(&f);
    let confinement = MemberConfinement::new(&admin_client, &FixtureCredential, member_handle());
    let bound = confinement.bind(&OpaqueId::new("user_7f3a")).expect("the same id must bind");
    let at = ObservedAt::parse("2026-09-02T12:34:56+00:00").expect("timestamp");
    let policy = ConfinementPolicy::read_only_analyst();
    let binding = PolicyBinding::of(&definition(), &policy);
    let Reading::Measured { locked, states, .. } = bound.read(&policy, &binding, &at) else {
        panic!("the catalogue should have been read: {:?}", requests(&f));
    };
    // Exactly the row that said so — not the one that said `false`, and not the one that omitted
    // the field. Both halves matter: a reader that collected every row, or none, would satisfy a
    // test that only checked membership.
    assert_eq!(
        locked,
        std::collections::BTreeSet::from(["builtin.operator_config_set_auto_approve".to_owned()]),
        "only the row reporting locked:true belongs in the set"
    );
    assert_eq!(states.len(), 3, "every tool row is still read for its state: {states:?}");
}

#[test]
fn a_credential_that_authenticates_as_the_recorded_member_binds() {
    // The positive control for every refusal below: the mechanism does admit the right credential,
    // and it does then read the catalogue.
    let f = admin(vec![
        (SESSION, 200, session_body("acme", "user_7f3a")),
        (SETTINGS, 200, catalogue_body()),
    ]);
    let admin_client = client(&f);
    let confinement = MemberConfinement::new(&admin_client, &FixtureCredential, member_handle());
    let bound = confinement.bind(&OpaqueId::new("user_7f3a")).expect("the same id must bind");
    assert_eq!(bound.subject().user().as_str(), "user_7f3a");
    assert_eq!(bound.subject().tenant().as_str(), "acme");

    let at = ObservedAt::parse("2026-09-02T12:34:56+00:00").expect("timestamp");
    let policy = ConfinementPolicy::read_only_analyst();
    let binding = PolicyBinding::of(&definition(), &policy);
    let reading = bound.read(&policy, &binding, &at);
    let Reading::Measured { attempt, .. } = &reading else {
        panic!("the catalogue should have been read: {:?}", requests(&f));
    };
    assert!(attempt.measurement().expect("measurement").is_clean());

    let log = requests(&f);
    assert_eq!(log.iter().filter(|r| r.starts_with(SESSION)).count(), 1, "{log:?}");
    assert_eq!(log.iter().filter(|r| r.starts_with(SETTINGS)).count(), 1, "{log:?}");
}

/// **The mechanical form of the rule.** A credential that is not this member's must produce exactly
/// one request — `/session` — and no settings traffic of any kind.
#[test]
fn a_subject_mismatch_makes_exactly_one_session_request_and_no_settings_request() {
    // `reborn-cli` is what the probe's env-bearer token resolves to, so this is the shape the
    // shipped proof has actually been running in.
    let f = admin(vec![
        (SESSION, 200, session_body("acme", "reborn-cli")),
        // Wired and answering, so a catalogue read would succeed if one were made. Without this
        // the assertion below could pass because the route was missing rather than unused.
        (SETTINGS, 200, catalogue_body()),
    ]);
    let admin_client = client(&f);
    let confinement = MemberConfinement::new(&admin_client, &FixtureCredential, member_handle());

    let Err(Binding::Mismatch(mismatch)) = confinement.bind(&OpaqueId::new("user_7f3a")) else {
        panic!("a different subject must not bind");
    };
    assert_eq!(mismatch.expected().as_str(), "user_7f3a");
    assert_eq!(mismatch.authenticated().user().as_str(), "reborn-cli");

    let log = requests(&f);
    assert_eq!(log.len(), 1, "exactly one request may be made: {log:?}");
    assert!(log[0].starts_with(SESSION), "{log:?}");
    assert!(
        !log.iter().any(|r| r.contains("settings/tools")),
        "a settings request was made after a mismatch: {log:?}"
    );
}

#[test]
fn an_unusable_session_response_blocks_and_reads_no_settings() {
    // Every shape of "we could not establish who this is". Each must block rather than bind, and
    // none may fall through to the settings surface.
    let cases: Vec<(&str, u16, String)> = vec![
        ("rejected", 401, String::new()),
        ("server error", 500, String::new()),
        ("no user_id", 200, r#"{"tenant_id":"acme"}"#.to_owned()),
        ("no tenant_id", 200, r#"{"user_id":"user_7f3a"}"#.to_owned()),
        // An id the pin's own charset rules refuse: unusable as a path segment, so it could never
        // be asked about again.
        ("an unusable user_id", 200, session_body("acme", "..")),
    ];
    for (label, status, body) in cases {
        let f = admin(vec![(SESSION, status, body), (SETTINGS, 200, catalogue_body())]);
        let admin_client = client(&f);
        let confinement =
            MemberConfinement::new(&admin_client, &FixtureCredential, member_handle());
        assert!(
            matches!(confinement.bind(&OpaqueId::new("user_7f3a")), Err(Binding::Blocked(_))),
            "{label} must block"
        );
        let log = requests(&f);
        assert!(
            !log.iter().any(|r| r.contains("settings/tools")),
            "{label} reached the settings surface: {log:?}"
        );
    }
}

#[test]
fn an_unresolvable_credential_blocks_before_any_request_at_all() {
    let f = admin(vec![(SESSION, 200, session_body("acme", "user_7f3a"))]);
    let admin_client = client(&f);
    let confinement = MemberConfinement::new(&admin_client, &NoCredential, member_handle());
    assert!(matches!(
        confinement.bind(&OpaqueId::new("user_7f3a")),
        Err(Binding::Blocked(BlockedReason::NoCredential { .. }))
    ));
    assert!(requests(&f).is_empty(), "no credential means no request: {:?}", requests(&f));
}

#[test]
fn a_blocked_catalogue_read_still_produces_an_attempt_naming_its_subject() {
    // The bound-but-unmeasurable rung. It is recorded, and it carries the subject and the policy —
    // which is what makes it distinguishable from a binding failure months later.
    let f = admin(vec![
        (SESSION, 200, session_body("acme", "user_7f3a")),
        (SETTINGS, 500, String::new()),
    ]);
    let admin_client = client(&f);
    let confinement = MemberConfinement::new(&admin_client, &FixtureCredential, member_handle());
    let bound = confinement.bind(&OpaqueId::new("user_7f3a")).expect("binds");
    let at = ObservedAt::parse("2026-09-02T12:34:56+00:00").expect("timestamp");
    let policy = ConfinementPolicy::read_only_analyst();
    let binding = PolicyBinding::of(&definition(), &policy);

    let Reading::Blocked { attempt } = bound.read(&policy, &binding, &at) else {
        panic!("a 500 catalogue must not measure");
    };
    assert_eq!(attempt.expected().as_str(), "user_7f3a");
    assert_eq!(attempt.authenticated().expect("subject").user().as_str(), "user_7f3a");
    assert_eq!(attempt.policy().expect("policy"), &binding);
    assert!(matches!(
        attempt.historical(),
        ironworks_lifecycle::Historical::Unmeasured { reason: Blocked::UnreadableAnswer }
    ));
}
