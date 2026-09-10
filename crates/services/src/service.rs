//! Loading definitions from the embedded service library.
//!
//! A thin adapter: it turns committed JSON into [`ironworks_domain::ServiceDefinition`] and knows
//! nothing else. The domain rules live in `ironworks-domain`; what lives here is the on-disk shape and the
//! refusals that shape can produce.
//!
//! # Validation the type system does for free
//!
//! - a **required key** is a non-`Option` field, enforced by deserialization;
//! - an **unknown key** is rejected outright, so a typo like `part` instead of `parts` is a load
//!   error rather than a field that silently does nothing;
//! - **guidance is mandatory** with no field at all — [`ironworks_domain::ServiceInstance::new`]
//!   takes a validated `Guidance`, not an `Option`, so there is no fail-open mode to configure;
//! - **the model is the pin** with no field at all — `ModelSelection` is constructible only from
//!   a pin, so a service cannot name one.
//!
//! Definitions are compiled in, so an unknown key really is a typo. That is the exact inverse of
//! the rule for wire types, where an unknown field must be tolerated because the other side may
//! legitimately add one.
//!
//! # There is no default service, and that is deliberate
//!
//! A `DEFAULT` constant used to name the service an instance got when its configuration named
//! none. It held `account-analysis`, which existed to be organization-neutral so that the slot a
//! misconfiguration reaches could not address an organization nobody chose it for.
//!
//! `account-analysis` had no user. Cutting it left one service, and that service is MultiAgency's
//! own record of its own relationships — the one composition that must never fill a silence. The
//! two ways out were to promote it anyway or to stop having a slot, and the check that guarded the
//! old constant named them both: *either the part stops naming an organization, or `DEFAULT` stops
//! being the default.* The slot is gone.
//!
//! So a configuration that names no service is **refused**, not filled in. [`ironworks_domain`]'s
//! registry already required the field; what changed is that a guidance marker omitting `service:`
//! is an error too, rather than an assertion that the file meant whichever service was default.
//! The value that fills a silence is the value nobody chose.

use ironworks_domain::{
    Access, CapabilityProfile, Composition, PartRef, ServiceDefinition, ServiceId, ToolAccess,
};
use serde::Deserialize;

include!(concat!(env!("OUT_DIR"), "/definitions.rs"));

/// Stable machine id for MultiAgency's operator-facing account-coordination service.
///
/// **One id, not a set** — [`available`] is the collection. The name is the job the service does
/// (it coordinates accounts) rather than the id it does it under: `relationship-intelligence` is
/// carried for compatibility, being already bound into guidance markers and live instance
/// configuration, and `services/README.md` records why renaming it is a migration rather than an
/// edit. Call sites say what the service is for; the legacy string stays here.
pub const ACCOUNTS: &str = "relationship-intelligence";
/// Repository-relative evidence suite named by [`ACCOUNTS`]'s definition.
pub const ACCOUNTS_EVALUATION: &str = "fixtures/account-coordination";

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("unknown service {name:?}; known services are: {known}")]
    Unknown { name: String, known: String },

    #[error("service {name:?}: definition is not valid JSON: {source}")]
    Malformed {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "service {name:?}: definition declares service {declared:?} — the filename is the \
         service id and they must agree"
    )]
    NameMismatch { name: String, declared: String },

    #[error("service {name:?}: {source}")]
    BadId {
        name: String,
        #[source]
        source: ironworks_domain::IdError,
    },

    #[error("service {name:?}: {source}")]
    BadComposition {
        name: String,
        #[source]
        source: ironworks_domain::CompositionError,
    },

    #[error("service {name:?}: {source}")]
    BadDefinition {
        name: String,
        #[source]
        source: ironworks_domain::ServiceDefinitionError,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    service: String,
    version: u32,
    responsibility: String,
    composition: WireComposition,
    capabilities: WireCapabilities,
    evaluation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireComposition {
    parts: Vec<String>,
    guidance_heading: String,
    safety_tail: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCapabilities {
    records: WireAccess,
    writes: WireAccess,
    egress: WireAccess,
    outreach: WireAccess,
    /// Required, with no serde default, and that is the point. A default here would be the one
    /// value that silently outranks the file — the same rule the pins follow — and this axis
    /// decides whether a tenant's turns can reach a shell. Adding it to a definition is a
    /// decision somebody has to type.
    runtime_tools: WireToolAccess,
}

/// No `write` variant, mirroring [`Access`]: an effect capability that can be *named* in a
/// committed file is one somebody eventually grants.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum WireAccess {
    None,
    ReadOnly,
}

impl From<WireAccess> for Access {
    fn from(w: WireAccess) -> Self {
        match w {
            WireAccess::None => Access::None,
            WireAccess::ReadOnly => Access::ReadOnly,
        }
    }
}

/// No `full` variant, mirroring [`ToolAccess`]: the catalogue is 53 rows on a fresh member and a
/// spelling meaning "all of them" is one somebody eventually writes into a file.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum WireToolAccess {
    None,
    Sandbox,
}

impl From<WireToolAccess> for ToolAccess {
    fn from(w: WireToolAccess) -> Self {
        match w {
            WireToolAccess::None => ToolAccess::None,
            WireToolAccess::Sandbox => ToolAccess::Sandbox,
        }
    }
}

/// Every service name with a definition, sorted.
pub fn available() -> impl Iterator<Item = &'static str> {
    DEFINITIONS.iter().map(|(n, _)| *n)
}

/// Load and validate one definition. Fails closed on every count.
pub fn load(name: &str) -> Result<ServiceDefinition, ServiceError> {
    let raw =
        DEFINITIONS.iter().find(|(n, _)| *n == name).map(|(_, raw)| *raw).ok_or_else(|| {
            ServiceError::Unknown {
                name: name.to_owned(),
                known: available().collect::<Vec<_>>().join(", "),
            }
        })?;
    from_raw(name, raw)
}

/// The whole load path over supplied bytes, so a refusal can be tested against a doctored
/// definition rather than only against `Wire`'s parse. Every check below runs for the embedded
/// definitions too — [`load`] is a lookup in front of this.
fn from_raw(name: &str, raw: &str) -> Result<ServiceDefinition, ServiceError> {
    let wire: Wire = serde_json::from_str(raw)
        .map_err(|source| ServiceError::Malformed { name: name.to_owned(), source })?;
    if wire.service != name {
        return Err(ServiceError::NameMismatch { name: name.to_owned(), declared: wire.service });
    }
    let id = ServiceId::parse(wire.service)
        .map_err(|source| ServiceError::BadId { name: name.to_owned(), source })?;
    let composition = Composition::new(
        wire.composition.parts.into_iter().map(PartRef::new).collect(),
        wire.composition.guidance_heading,
        PartRef::new(wire.composition.safety_tail),
    )
    .map_err(|source| ServiceError::BadComposition { name: name.to_owned(), source })?;
    let capabilities = CapabilityProfile {
        records: wire.capabilities.records.into(),
        writes: wire.capabilities.writes.into(),
        egress: wire.capabilities.egress.into(),
        outreach: wire.capabilities.outreach.into(),
        runtime_tools: wire.capabilities.runtime_tools.into(),
    };
    ServiceDefinition::new(
        id,
        wire.version,
        wire.responsibility,
        composition,
        capabilities,
        wire.evaluation,
    )
    .map_err(|source| ServiceError::BadDefinition { name: name.to_owned(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_committed_service_loads() {
        let names: Vec<_> = available().collect();
        assert_eq!(names, vec!["relationship-intelligence"]);
        for name in names {
            let d = load(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.id().as_str(), name);
        }
    }

    #[test]
    fn account_coordination_keeps_its_stable_id_and_names_its_evidence_suite() {
        let d = load(ACCOUNTS).unwrap();
        assert_eq!(d.version(), 2);
        assert_eq!(d.evaluation(), Some(ACCOUNTS_EVALUATION));
        assert!(d.responsibility().contains("Coordinate MultiAgency's accounts"));

        // The suite is its corpus: the guidance a turn is composed under and the records it is
        // shown. **The graders are no longer part of it** — they were `cases.py` here and are now
        // `crates/graders`, which is why this list lost an entry rather than gaining one. A grader in
        // the fixture directory was a grader nothing compiled and nothing type-checked; the
        // controls that pin it now run under `cargo test` with everything else.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let suite = root.join(ACCOUNTS_EVALUATION);
        for required in ["guidance.md", "book"] {
            assert!(
                suite.join(required).exists(),
                "{} names an evaluation suite missing {required}",
                d.service_id()
            );
        }
        assert!(
            !suite.join("cases.py").exists(),
            "the graders moved to crates/graders; a cases.py here would be a second copy, and a \
             second copy of a grader is a second thing to disagree with the first"
        );
    }

    // ── the slot that no longer exists ──────────────────────────────────────────────

    /// The refusal itself. There is no service to fall back to and no spelling that asks for one:
    /// the empty name is not a wildcard, it is unknown like any other.
    ///
    /// This is what replaced `DEFAULT`. The known-service assertion is the positive control: the
    /// loader can resolve an explicit choice in the same test where it refuses silence.
    #[test]
    fn naming_no_service_is_refused_rather_than_defaulted() {
        assert!(load(ACCOUNTS).is_ok(), "an explicitly selected service resolves");
        for unnamed in ["", " ", "default"] {
            let e = load(unnamed).expect_err("an unnamed service must not resolve to one");
            assert!(
                matches!(e, ServiceError::Unknown { .. }),
                "{unnamed:?} must be unknown, not special: {e}"
            );
        }
    }

    #[test]
    fn an_unknown_service_names_the_known_ones() {
        let e = load("nope").unwrap_err();
        assert!(e.to_string().contains(ACCOUNTS), "the error should orient the reader");
    }

    #[test]
    fn every_service_is_read_only_and_ends_on_the_shared_safety_tail() {
        for name in available() {
            let d = load(name).unwrap();
            assert_eq!(*d.capability_ceiling(), CapabilityProfile::READ_ONLY_RECORDS);
            assert_eq!(
                d.composition().safety_tail().as_str(),
                "personas/_safety-tail.md",
                "one safety tail, not one per service"
            );
            assert_eq!(
                d.composition().all_parts().last().unwrap(),
                d.composition().safety_tail(),
                "the tail is last in the part order"
            );
        }
    }

    // ── fail-closed shapes, as parse failures rather than hand-written checks ────────

    fn doctored(from: &str, to: &str) -> Result<Wire, serde_json::Error> {
        let base = DEFINITIONS[0].1;
        let d = base.replace(from, to);
        assert_ne!(base, d, "the fixture must actually have been changed");
        serde_json::from_str(&d)
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_ignored() {
        assert!(doctored("\"parts\"", "\"part\"").is_err(), "a typo must not silently do nothing");
    }

    #[test]
    fn a_write_capability_cannot_even_be_spelled() {
        // Effect execution is unrepresentable, not merely unimplemented.
        assert!(doctored("\"writes\": \"none\"", "\"writes\": \"read-write\"").is_err());
    }

    #[test]
    fn a_missing_required_key_is_rejected() {
        assert!(serde_json::from_str::<Wire>(r#"{"service":"x","version":1}"#).is_err());
    }

    #[test]
    fn runtime_tools_has_no_default_and_no_full_spelling() {
        // No default: a definition that forgets the axis fails to parse rather than quietly
        // meaning `none`, because the value that fills a silence is the value nobody chose.
        assert!(
            doctored(",\n    \"runtime_tools\": \"none\"", "").is_err(),
            "omitting runtime_tools must refuse the definition"
        );
        // And there is no spelling that grants the catalogue wholesale.
        assert!(doctored("\"runtime_tools\": \"none\"", "\"runtime_tools\": \"full\"").is_err());
        assert!(doctored("\"runtime_tools\": \"none\"", "\"runtime_tools\": \"all\"").is_err());
    }

    #[test]
    fn a_sandbox_service_must_admit_its_egress() {
        // The two-sided control. `sandbox` + `egress: none` is the profile §L-10 says the runtime
        // will not honour, and it must be refused at load rather than shipped as a false promise.
        let base = DEFINITIONS[0].1;
        let lying = base.replace("\"runtime_tools\": \"none\"", "\"runtime_tools\": \"sandbox\"");
        assert_ne!(base, lying);
        let err =
            from_raw(DEFINITIONS[0].0, &lying).expect_err("sandbox with egress:none must refuse");
        assert!(
            err.to_string().contains("managed egress allowlist"),
            "the refusal must say why, got: {err}"
        );

        // The positive control: the same definition with the honest egress loads. Without this the
        // check above would pass the moment `sandbox` stopped parsing at all.
        let honest = lying.replace("\"egress\": \"none\"", "\"egress\": \"read-only\"");
        let d =
            from_raw(DEFINITIONS[0].0, &honest).expect("sandbox with read-only egress must load");
        assert_eq!(d.capability_ceiling().runtime_tools, ToolAccess::Sandbox);
        assert_eq!(d.capability_ceiling().egress, Access::ReadOnly);
    }

    /// Guidance and model policy have no fields at all. If someone re-adds one to a definition,
    /// `deny_unknown_fields` refuses it — the enforcement moved into the type model and the file
    /// must not grow a second, weaker copy of it.
    #[test]
    fn guidance_and_model_policy_cannot_be_configured_here() {
        assert!(doctored("\"version\": 2", "\"version\": 2, \"guidance\": \"optional\"").is_err());
        assert!(
            doctored("\"version\": 2", "\"version\": 2, \"model_policy\": \"caller\"").is_err()
        );
    }
}
