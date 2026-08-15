//! Approval decision types. Policy lives in methodus-core; this is the domain enum.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// User (or policy) decision on a pending approval.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Once,
    Session,
    Deny,
    Abort,
}

impl fmt::Display for ApprovalDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Once => "once",
            Self::Session => "session",
            Self::Deny => "deny",
            Self::Abort => "abort",
        };
        write!(f, "{s}")
    }
}

impl FromStr for ApprovalDecision {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "once" => Ok(Self::Once),
            "session" => Ok(Self::Session),
            "deny" => Ok(Self::Deny),
            "abort" => Ok(Self::Abort),
            other => Err(DomainError::InvalidStatus {
                entity: "approval",
                value: other.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decisions() {
        assert_eq!(
            "once".parse::<ApprovalDecision>().unwrap(),
            ApprovalDecision::Once
        );
        assert_eq!(
            "session".parse::<ApprovalDecision>().unwrap(),
            ApprovalDecision::Session
        );
        assert!("maybe".parse::<ApprovalDecision>().is_err());
    }
}
