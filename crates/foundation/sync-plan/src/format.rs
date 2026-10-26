//! Forme JSON versionnée d'un plan.

use serde_json::Value;
use sync_errors::{ClassifiedError, ErrorClass};
use thiserror::Error;

use crate::plan::{CASE_MERGE_PLAN_SCHEMA_VERSION, ExecutionPlan, PLAN_SCHEMA_VERSION, RawPlan};
use crate::validate::PlanError;

/// Erreur de lecture ou d'écriture de la forme JSON d'un plan.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlanFormatError {
    /// Le texte n'est pas un JSON de plan bien formé.
    #[error("plan JSON mal formé : {0}")]
    Malformed(#[source] serde_json::Error),
    /// Le champ `schema_version` est absent ou n'est pas un entier.
    #[error("plan sans version de schéma")]
    MissingVersion,
    /// Version de schéma non prise en charge : le plan n'est pas interprété.
    #[error(
        "version de schéma de plan {found} non prise en charge (attendu {PLAN_SCHEMA_VERSION})"
    )]
    UnsupportedVersion {
        /// Version lue.
        found: u64,
    },
    /// Le plan est bien formé mais viole un invariant.
    #[error("plan invalide : {0}")]
    Invalid(#[source] PlanError),
    /// Sérialisation impossible.
    #[error("sérialisation du plan impossible : {0}")]
    Serialize(#[source] serde_json::Error),
}

impl ClassifiedError for PlanFormatError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Malformed(_) | Self::MissingVersion => ErrorClass::InvalidInput,
            Self::UnsupportedVersion { .. } => ErrorClass::NotSupported,
            Self::Invalid(_) => ErrorClass::Integrity,
            Self::Serialize(_) => ErrorClass::Internal,
        }
    }
}

impl ExecutionPlan {
    /// Forme JSON indentée, champs dans un ordre stable.
    ///
    /// # Errors
    /// [`PlanFormatError::Serialize`] (non attendu : tous les champs sont
    /// représentables).
    pub fn to_json(&self) -> Result<String, PlanFormatError> {
        serde_json::to_string_pretty(self).map_err(PlanFormatError::Serialize)
    }

    /// Charge un plan JSON : la version de schéma est vérifiée **avant**
    /// toute interprétation, puis tous les invariants le sont (un plan
    /// modifié à la main ne peut pas, par exemple, réintroduire une
    /// suppression refusée).
    ///
    /// # Errors
    /// [`PlanFormatError`] : mal formé, version absente ou non prise en
    /// charge, invariant violé.
    pub fn from_json(text: &str) -> Result<Self, PlanFormatError> {
        let value: Value = serde_json::from_str(text).map_err(PlanFormatError::Malformed)?;
        let found = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or(PlanFormatError::MissingVersion)?;
        if found != u64::from(PLAN_SCHEMA_VERSION)
            && found != 2
            && found != u64::from(CASE_MERGE_PLAN_SCHEMA_VERSION)
        {
            return Err(PlanFormatError::UnsupportedVersion { found });
        }
        let raw: RawPlan = serde_json::from_value(value).map_err(PlanFormatError::Malformed)?;
        Self::try_from(raw).map_err(PlanFormatError::Invalid)
    }
}
