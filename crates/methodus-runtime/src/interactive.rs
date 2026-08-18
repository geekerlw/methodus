//! Optional real-time control (Codex app-server, Claude background). Cursor: unsupported.

use async_trait::async_trait;

use methodus_domain::ApprovalDecision;

use crate::adapter::{RuntimeAdapter, RuntimeError, SessionHandle};

/// Real-time approval / interrupt / steer — implemented when using Codex app-server.
#[async_trait]
pub trait InteractiveRuntime: RuntimeAdapter {
    async fn resolve_approval(
        &self,
        _session: &SessionHandle,
        _id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::NotFound(
            "interactive approval not supported for this runtime".into(),
        ))
    }

    async fn interrupt(&self, _session: &SessionHandle) -> Result<(), RuntimeError> {
        Err(RuntimeError::NotFound(
            "interrupt not supported for this runtime".into(),
        ))
    }

    async fn send_turn(&self, _session: &SessionHandle, _prompt: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError::NotFound(
            "send_turn not supported for this runtime".into(),
        ))
    }
}
