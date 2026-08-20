//! Runtime boundary for the explicit repository discovery command.

use crate::core::error::{MjolnrError, ReasonCode};
use crate::core::model::ModelDescriptor;

use super::Actor;

impl Actor {
    pub(super) async fn run_discovery(&mut self) -> Result<(), MjolnrError> {
        let Some(root) = self.state.workspace_root.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "No project is open, so discovery was refused and nothing was written",
            ));
        };
        let models = self
            .model_catalogs
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<ModelDescriptor>>();
        let outcome =
            tokio::task::spawn_blocking(move || crate::discovery::run(&root, &models)).await;
        match outcome {
            Ok(Ok(report)) => {
                self.last_discovery = Some(report);
                self.publish_snapshot();
                Ok(())
            }
            Ok(Err(error)) => Err(MjolnrError::workspace_refused(
                error.reason_code(),
                format!("discovery refused or failed: {error}"),
            )),
            Err(error) => Err(MjolnrError::workspace_refused(
                ReasonCode::ToolExecution,
                format!("discovery task did not complete: {error}"),
            )),
        }
    }
}
