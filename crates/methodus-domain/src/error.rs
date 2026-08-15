//! Domain-level errors (invalid transitions, parse failures). No I/O.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("invalid {entity} status transition: {from} → {to}")]
    InvalidTransition {
        entity: &'static str,
        from: String,
        to: String,
    },

    #[error("invalid {entity} status value: {value}")]
    InvalidStatus { entity: &'static str, value: String },
}
