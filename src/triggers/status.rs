//! The one read model behind `smed triggers list`, the `/triggers` TUI
//! overlay, and the scheduler's own startup state.
//!
//! One function, three consumers: this is deliberate. A second, divergent way
//! to compute "is this trigger disabled" would let the CLI and the TUI
//! disagree about a fact a human is about to act on.

use std::path::Path;

use time::OffsetDateTime;

use crate::core::event::SmedEvent;
use crate::core::store::EventStore;
use crate::core::trigger::TriggerStatus;

use super::control;
use super::definition::{self, TriggerDefinition, TriggerLoadDiagnostic};
use super::schedule::CronSchedule;

/// Collect display state for every trigger configured under `project_root`.
///
/// # Errors
/// A store failure reading a control session's history. A missing or
/// unparseable trigger *file* is not an error here — it is a
/// [`TriggerLoadDiagnostic`], because one bad file must not hide every other
/// trigger's status.
pub async fn collect(
    store: &dyn EventStore,
    project_root: &Path,
    project_root_realpath: &str,
) -> Result<(Vec<TriggerStatus>, Vec<TriggerLoadDiagnostic>), crate::core::store::StoreError> {
    let (definitions, diagnostics) = definition::load_dir(project_root);
    let mut statuses = Vec::with_capacity(definitions.len());
    for definition in &definitions {
        statuses.push(status_of(store, project_root_realpath, definition).await?);
    }
    Ok((statuses, diagnostics))
}

async fn status_of(
    store: &dyn EventStore,
    project_root_realpath: &str,
    definition: &TriggerDefinition,
) -> Result<TriggerStatus, crate::core::store::StoreError> {
    let control_session = control::control_session_id(project_root_realpath, &definition.name);
    let events = control::history(store, control_session).await?;
    let state = control::replay(&events, &definition.name);

    let last_fired_at = events.iter().rev().find_map(|stored| match &stored.event {
        SmedEvent::TriggerFired { trigger, .. } if trigger == &definition.name => {
            Some(stored.occurred_at)
        }
        _ => None,
    });

    let enabled = state.disabled_reason.is_none();
    let next_fire_at = if enabled { next_fire(definition) } else { None };

    Ok(TriggerStatus {
        name: definition.name.clone(),
        source: definition.source.kind(),
        overlap: definition.overlap,
        enabled,
        disabled_reason: state.disabled_reason,
        consecutive_failures: state.consecutive_failures,
        max_consecutive_failures: definition.max_consecutive_failures,
        last_outcome: state.last_outcome,
        last_fired_at,
        next_fire_at,
    })
}

fn next_fire(definition: &TriggerDefinition) -> Option<OffsetDateTime> {
    match &definition.source {
        super::definition::TriggerSource::Schedule { cron } => {
            let schedule = CronSchedule::parse(cron).ok()?;
            schedule.next_after(OffsetDateTime::now_utc())
        }
        super::definition::TriggerSource::Webhook { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::store::ProjectId;
    use crate::store::memory::InMemoryEventStore;

    #[tokio::test]
    async fn a_trigger_with_no_history_is_enabled_with_no_last_outcome() {
        let store = InMemoryEventStore::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("triggers");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("nightly.yaml"),
            "schedule: \"0 3 * * *\"\ndirective: run\nprovider: fake\nmodel: fake-1\n",
        )
        .expect("write");

        let (statuses, diagnostics) = collect(&store, temp.path(), &temp.path().to_string_lossy())
            .await
            .expect("collect");
        assert!(diagnostics.is_empty());
        assert_eq!(statuses.len(), 1);
        let status = statuses.first().expect("one status");
        assert!(status.enabled);
        assert_eq!(status.last_outcome, None);
        assert!(status.next_fire_at.is_some());
    }

    #[tokio::test]
    async fn a_disabled_trigger_is_reported_disabled_with_no_next_fire() {
        let store = InMemoryEventStore::new();
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("triggers");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("nightly.yaml"),
            "schedule: \"0 3 * * *\"\ndirective: run\nprovider: fake\nmodel: fake-1\n",
        )
        .expect("write");
        let root_realpath = temp.path().to_string_lossy().into_owned();
        let control_session = control::control_session_id(&root_realpath, "nightly");
        let project = ProjectId::new();
        store
            .create_session(control_session, project, "trigger:nightly".to_owned(), None)
            .await
            .expect("create control session");
        store
            .append(SmedEvent::TriggerDisabled {
                session: control_session,
                trigger: "nightly".to_owned(),
                code: crate::core::error::ReasonCode::TriggerDisabled,
                consecutive_failures: 3,
            })
            .await
            .expect("append");

        let (statuses, _) = collect(&store, temp.path(), &root_realpath)
            .await
            .expect("collect");
        let status = statuses.first().expect("one status");
        assert!(!status.enabled);
        assert_eq!(
            status.disabled_reason,
            Some(crate::core::error::ReasonCode::TriggerDisabled)
        );
        assert_eq!(status.next_fire_at, None);
    }
}
