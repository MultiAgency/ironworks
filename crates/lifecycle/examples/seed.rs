//! Populate a lifecycle store for manual inspection. Development scaffolding, not product.
//!
//!   seed <store> <slug> <through> [member-id]
//!
//! `member-id` defaults to a literal, which is correct for the synthetic lifecycle proofs: they
//! reconcile against fixtures and make no claim about a live member. It is **wrong** for anything
//! that then confines that member — the id must be one a runtime actually issued, or the binding
//! check will refuse it. `dev/prove-confinement.sh` therefore passes a minted id explicitly, and
//! the id is an identifier rather than a secret, so it is safe on a command line.
use ironworks_lifecycle::*;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, slug, through) = (&args[0], &args[1], &args[2]);
    let member_id = args.get(3).cloned().unwrap_or_else(|| "user_7f3a".to_owned());
    let now = "2026-08-27T12:00:00+00:00";
    let store = LifecycleStore::open(path).expect("open");
    let spec = InstanceSpec {
        slug: ironworks_domain::Slug::parse(slug.clone()).unwrap(),
        org: ironworks_domain::OrgId::parse("acme-sales").unwrap(),
        // Spelled out rather than taken from `ironworks_services::service::ACCOUNTS`, because this
        // crate does not depend on the corpus and scaffolding does not earn a new crate edge. It
        // must still name a service the corpus holds: `lifecycle confine` loads the recorded
        // instance's definition to derive its policy, so a slug seeded against a service that will
        // not load refuses before it confines anything. `dev/prove-confinement.sh` check 3 is what
        // fails if this drifts, and it names the service in its message.
        service: ironworks_domain::ServiceId::parse("relationship-intelligence").unwrap(),
        channel: ironworks_domain::ChannelRef::new("-100900001"),
    };
    store.declare(&spec, now).expect("declare");
    for stage in Stage::ALL {
        let id = Authority::ALL.iter().copied().find(|a| a.created_at() == stage).map(|a| {
            OpaqueId::new(if a == Authority::Member {
                member_id.clone()
            } else {
                format!("{}-1", a.tag())
            })
        });
        store.reach(&spec.slug, stage, false, id.as_ref(), now).expect("reach");
        if stage.tag() == through {
            return;
        }
    }
    let smoke = SmokeEvidence::from_outcome(
        &SmokeOutcome::Passed { isolation: "own org only".into(), reachability: "200".into() },
        now,
    )
    .unwrap();
    store.activate(&spec.slug, &smoke, now).expect("activate");
}
