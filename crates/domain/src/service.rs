//! What a service *is*, as data.
//!
//! The reusable half of a service. An instance is the other half — identity, credentials, a
//! channel, and its own guidance. Two instances of one service differ only in their records and
//! their guidance, and there is no branch anywhere on *who* an instance belongs to.
//!
//! # What is deliberately absent
//!
//! **No `model_policy`.** A field with one legal value is noise; the rule "a service cannot
//! choose its own model" is better expressed by there being no model field here at all, and
//! [`crate::ModelSelection`] being constructible only from a pin.
//!
//! **No `audience`.** Internal use earns no privilege, so an internal/external marker was
//! documentation. A field that exists to be looked at is a comment.
//!
//! **No `title`/`summary` pair.** One [`ServiceDefinition::responsibility`] — the word implies
//! accountability and a boundary, where "summary" invites marketing copy.

use crate::capability::CapabilityProfile;
use crate::composition::Composition;
use crate::ids::ServiceId;

/// The reusable half of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDefinition {
    id: ServiceId,
    version: u32,
    responsibility: String,
    composition: Composition,
    capabilities: CapabilityProfile,
    evaluation: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServiceDefinitionError {
    #[error("service {id}: version must be a positive integer, got {version}")]
    BadVersion { id: ServiceId, version: u32 },
    #[error("service {id}: a responsibility must say what the service is accountable for")]
    NoResponsibility { id: ServiceId },
    /// A ceiling promising something the runtime will not honour. Refused at construction so it
    /// cannot be compiled into the binary and then read by a tenant as a guarantee.
    #[error("service {id}: {source}")]
    IncoherentCapabilities {
        id: ServiceId,
        #[source]
        source: crate::capability::Incoherence,
    },
}

impl ServiceDefinition {
    pub fn new(
        id: ServiceId,
        version: u32,
        responsibility: impl Into<String>,
        composition: Composition,
        capabilities: CapabilityProfile,
        evaluation: Option<String>,
    ) -> Result<Self, ServiceDefinitionError> {
        let responsibility = responsibility.into();
        if version == 0 {
            return Err(ServiceDefinitionError::BadVersion { id, version });
        }
        if responsibility.trim().is_empty() {
            return Err(ServiceDefinitionError::NoResponsibility { id });
        }
        if let Some(source) = capabilities.incoherence() {
            return Err(ServiceDefinitionError::IncoherentCapabilities { id, source });
        }
        Ok(Self { id, version, responsibility, composition, capabilities, evaluation })
    }

    pub fn id(&self) -> &ServiceId {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn responsibility(&self) -> &str {
        &self.responsibility
    }
    pub fn composition(&self) -> &Composition {
        &self.composition
    }
    /// The **ceiling**. An instance may hold this or less, never more.
    pub fn capability_ceiling(&self) -> &CapabilityProfile {
        &self.capabilities
    }
    pub fn evaluation(&self) -> Option<&str> {
        self.evaluation.as_deref()
    }

    /// `<name>@<version>` — the operator-visible version string.
    pub fn service_id(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::PartRef;

    fn composition() -> Composition {
        Composition::new(vec![PartRef::new("p")], "# H", PartRef::new("t")).unwrap()
    }

    #[test]
    fn a_definition_carries_its_version_string() {
        let d = ServiceDefinition::new(
            ServiceId::parse("account-analysis").unwrap(),
            1,
            "reads a tenant's own records and separates evidence from inference",
            composition(),
            CapabilityProfile::READ_ONLY_RECORDS,
            Some("fixtures/book".into()),
        )
        .unwrap();
        assert_eq!(d.service_id(), "account-analysis@1");
        assert_eq!(*d.capability_ceiling(), CapabilityProfile::READ_ONLY_RECORDS);
    }

    #[test]
    fn version_zero_and_an_empty_responsibility_are_refused() {
        let id = ServiceId::parse("s").unwrap();
        assert!(matches!(
            ServiceDefinition::new(
                id.clone(),
                0,
                "r",
                composition(),
                CapabilityProfile::NONE,
                None
            ),
            Err(ServiceDefinitionError::BadVersion { .. })
        ));
        assert!(matches!(
            ServiceDefinition::new(id, 1, "   ", composition(), CapabilityProfile::NONE, None),
            Err(ServiceDefinitionError::NoResponsibility { .. })
        ));
    }
}
