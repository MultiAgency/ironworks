//! Populate a lifecycle store for manual inspection. Development scaffolding, not product.
//!
//!   seed <store> <slug> <through> [member-id]
//!
//! `through` is a stage tag, and the reachable set stops at `member_minted` — see below.
//!
//! `member-id` defaults to a literal, which is correct for the synthetic lifecycle proofs: they
//! reconcile against fixtures and make no claim about a live member. It is **wrong** for anything
//! that then confines that member — the id must be one a runtime actually issued, or the binding
//! check will refuse it. `dev/prove-confinement.sh` therefore passes a minted id explicitly, and
//! the id is an identifier rather than a secret, so it is safe on a command line.
//!
//! # Why seeding stops at `member_minted`
//!
//! [`LifecycleStore::reach`] refuses `member_confined` outright, and refuses to skip it: a stage
//! may only be reached from its immediate predecessor, because *"`member_confined` sitting between
//! `member_minted` and `staged` is what stops a member with an unbounded tool surface becoming
//! routable"*. The only way past is [`LifecycleStore::certify_confinement`], which takes evidence
//! of a measurement that actually happened — §B-8, probed and never inferred.
//!
//! Scaffolding must not manufacture that evidence, so this seeds up to the last stage it can reach
//! honestly and refuses the rest by name. Nothing needs more: `dev/prove-confinement.sh` runs
//! `lifecycle confine`, which needs a recorded member and a spec and is itself what advances
//! `member_confined` — a store seeded past that point would defeat the proof rather than enable
//! it. `dev/prove-lifecycle.sh` reconciles org registration only, and says so in its own header.
//!
//! An earlier draft accepted `activated` and `serving`. Those were never stage tags, so the
//! comparison against `stage.tag()` never matched, the walk ran past every stage, and `reach`
//! panicked at `member_confined` — silently, in one caller, leaving a half-seeded store that the
//! caller's `[ -f ]` check reported as success. `through` is validated for that reason: a seed
//! that quietly produces a different store than it was asked for is worse than one that stops.
use ironworks_lifecycle::*;

/// The last stage a seed can reach without evidence it would have to invent.
const CEILING: Stage = Stage::MemberMinted;

/// The stage to seed through, or why the request cannot be honoured.
fn parse_through(raw: &str) -> Result<Stage, String> {
    let seedable = || {
        Stage::ALL.iter().filter(|s| **s <= CEILING).map(|s| s.tag()).collect::<Vec<_>>().join(", ")
    };
    match Stage::from_tag(raw) {
        Some(stage) if stage <= CEILING => Ok(stage),
        // A real stage, honestly unreachable from here. Naming the reason beats naming the set.
        Some(stage) => Err(format!(
            "{} is past {}, which is as far as a seed can go: reaching it requires confinement \
             evidence from a measurement that actually happened, and scaffolding must not \
             manufacture that. Seedable stages: {}",
            stage.tag(),
            CEILING.tag(),
            seedable()
        )),
        None => Err(format!("unknown stage {raw:?}. Seedable stages: {}", seedable())),
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(([path, slug, through], rest)) = args.split_first_chunk::<3>() else {
        eprintln!("usage: seed <store> <slug> <through> [member-id]");
        return std::process::ExitCode::from(64);
    };
    let through = match parse_through(through) {
        Ok(s) => s,
        Err(why) => {
            eprintln!("seed: {why}");
            return std::process::ExitCode::from(64);
        }
    };
    let member_id = rest.first().cloned().unwrap_or_else(|| "user_7f3a".to_owned());
    let now = ironworks_lifecycle::ObservedAt::parse("2026-08-27T12:00:00+00:00")
        .expect("the seed timestamp is the shape the ledger orders by");
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
    store.declare(&spec, &now).expect("declare");
    for stage in Stage::ALL.into_iter().take_while(|s| *s <= through) {
        let id = Authority::ALL.iter().copied().find(|a| a.created_at() == stage).map(|a| {
            OpaqueId::new(if a == Authority::Member {
                member_id.clone()
            } else {
                format!("{}-1", a.tag())
            })
        });
        store
            .reach(&spec.slug, stage, IdentityProvenance::Created, id.as_ref(), &now)
            .expect("reach");
    }
    std::process::ExitCode::SUCCESS
}
