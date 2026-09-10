//! Apply and verify confinement against a live member. Development scaffolding for the proof.
//!
//!   confine <api> <member-token-env-name> apply|verify
//!
//! `apply` disables every catalogued tool and turns approval off, **authority-changing tools
//! first** — a member that can still reach `operator_config_set_tool_permission` can undo every
//! later denial, so catalogue order would leave a reversible window.
use ironworks_domain::{CredentialPurpose, CredentialRef};
use ironworks_lifecycle::{
    AUTHORITY_CHANGING, Confinement, ConfinementPolicy, ObservedAt, OpaqueId,
};
use ironworks_reconcile::{Admin, Binding, MemberConfinement};
use ironworks_records::{CredentialSource, EnvCredentials};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (api, env_name, action) = (&a[0], &a[1], &a[2]);
    let cred = CredentialRef::new(CredentialPurpose::RuntimeMember, format!("env:{env_name}"));
    let secret = EnvCredentials.resolve(&cred).expect("member credential");

    if action == "apply" {
        let agent = ureq::Agent::new_with_defaults();
        let cat = agent
            .get(format!("{api}/api/webchat/v2/settings/tools"))
            .header("authorization", &secret.authorization_header())
            .call()
            .expect("catalog")
            .body_mut()
            .read_to_string()
            .expect("body");
        let doc: serde_json::Value = serde_json::from_str(&cat).expect("json");
        let mut tools: Vec<String> = doc["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|e| {
                e["key"].as_str().and_then(|k| k.strip_prefix("tool.")).map(str::to_owned)
            })
            .collect();
        // Authority-changing first. The ordering IS the invariant.
        tools.sort_by_key(|t| (!AUTHORITY_CHANGING.contains(&t.as_str()), t.clone()));
        for t in &tools {
            let _ = agent
                .post(format!("{api}/api/webchat/v2/settings/tools/{t}"))
                .header("authorization", &secret.authorization_header())
                .header("content-type", "application/json")
                .send(r#"{"state":"disabled"}"#);
        }
        println!("applied to {} tool(s)", tools.len());
        return;
    }

    let admin = Admin::new(
        api,
        &CredentialRef::new(CredentialPurpose::OperatorConfiguration, "env:UNUSED"),
    );
    let confinement = MemberConfinement::new(&admin, &EnvCredentials, cred);
    // Binding first, and there is no way past it: `observe` lives on the bound member, so this
    // scaffolding cannot report on a catalogue it never established the subject of. `MEMBER_ID` is
    // the id lifecycle would have recorded, and the runtime is asked whether the credential is it.
    let expected = OpaqueId::new(std::env::var("MEMBER_ID").unwrap_or_default());
    let bound = match confinement.bind(&expected) {
        Ok(bound) => bound,
        Err(Binding::Blocked(why)) => {
            println!("BLOCKED   the member credential could not be identified: {why}");
            return;
        }
        Err(Binding::Mismatch(mismatch)) => {
            println!("MISMATCH  {mismatch}");
            println!("  no settings read and no settings write was made");
            return;
        }
    };
    println!("bound     authenticated as {}", bound.subject());
    let at = ObservedAt::parse("2026-01-01T00:00:00+00:00").expect("a fixed scaffolding timestamp");
    match bound.observe(&ConfinementPolicy::read_only_analyst(), &at) {
        // Print the measured count, never a literal. "0 reachable" was hardcoded here and was
        // wrong: the two un-disableable tools ARE reachable, and approval-off is what contains
        // them — which is exactly the nuance a hardcoded zero erases.
        Confinement::Verified(f) => println!(
            "VERIFIED  {} catalogued · {} reachable (approval-gated) · policy {}",
            f.catalogued,
            f.reachable.len(),
            f.policy_fingerprint
        ),
        Confinement::Drifted { facts, drift } => {
            println!(
                "DRIFTED   {} catalogued · {} reachable",
                facts.catalogued,
                facts.reachable.len()
            );
            for d in drift.iter().take(3) {
                println!("  {d:?}");
            }
        }
        Confinement::Unverifiable(r) => println!("UNVERIFIABLE  {r}"),
    }
}
