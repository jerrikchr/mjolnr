//! Phase 27 `/config` settings-surface acceptance tests.
//!
//! The surface is a lens over diffable files: a change made through it must
//! produce exactly the file edit a hand-editor would, load through the Phase 15
//! loader without diagnostics, and record no policy event — it edits config, it
//! is not a control channel.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::event::SmedEvent;
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::FakeProvider;
use smed::routing::definition::{load_dir, parse_route};
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use smed::tui::reducer::{Overlay, ViewState};

const ROUTE_FILE: &str = "# a route file a hand-editor wrote\n\
    hops:\n  - provider: \"openai\"\n    model: \"gpt-5.4\"\n\
    roles: [\"default\"]\n";

async fn settle_until(runtime: &Runtime, predicate: impl Fn(&RuntimeSnapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if predicate(&runtime.snapshot()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition settled");
}

#[test]
fn config_overlay_toggles_cleanly() {
    let mut view = ViewState::default();
    assert_eq!(view.overlay, Overlay::None);
    view.toggle_config();
    assert_eq!(view.overlay, Overlay::Config);
    view.toggle_config();
    assert_eq!(view.overlay, Overlay::None);
}

#[tokio::test]
async fn binding_a_persona_through_config_writes_the_route_file_and_reloads_live() {
    let temp = tempfile::tempdir().expect("tempdir");
    let routes = temp.path().join(".smed").join("routes");
    std::fs::create_dir_all(&routes).expect("create routes dir");
    let route_path = routes.join("main.yaml");
    std::fs::write(&route_path, ROUTE_FILE).expect("write route");

    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    settle_until(&runtime, |snap| snap.workspace_root.is_some()).await;
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle_until(&runtime, |snap| snap.session.is_some()).await;
    let session = runtime.snapshot().session.expect("session");

    // The edit a hand-editor would make, made through the surface instead.
    runtime
        .dispatch(SmedCommand::BindRoutePersona {
            route: "main".to_owned(),
            persona: Some("mentor".to_owned()),
        })
        .await
        .expect("bind persona");
    settle_until(&runtime, |snap| {
        snap.routes
            .iter()
            .any(|route| route.persona.as_deref() == Some("mentor"))
    })
    .await;

    // The file on disk carries the binding and still loads without diagnostics.
    let written = std::fs::read_to_string(&route_path).expect("read route");
    let route = parse_route("main".to_owned(), &written).expect("route parses");
    assert_eq!(route.persona.as_deref(), Some("mentor"));
    assert!(written.contains("# a route file a hand-editor wrote"));
    let (_table, diagnostics) = load_dir(temp.path());
    assert!(diagnostics.is_empty(), "the written config loads clean");

    // The live route table reflects the binding without a restart.
    let main = runtime
        .snapshot()
        .routes
        .iter()
        .find(|route| route.name == "main")
        .cloned()
        .expect("main route present after reload");
    assert_eq!(main.persona.as_deref(), Some("mentor"));

    // Clearing it through the surface removes the line and round-trips.
    runtime
        .dispatch(SmedCommand::BindRoutePersona {
            route: "main".to_owned(),
            persona: None,
        })
        .await
        .expect("clear persona");
    settle_until(&runtime, |snap| {
        snap.routes.iter().all(|route| route.persona.is_none())
    })
    .await;
    let cleared = std::fs::read_to_string(&route_path).expect("read route");
    assert!(!cleared.contains("persona:"));

    // /config edits configuration; it gates nothing and records no policy event.
    let events = store.events(session).await.expect("events");
    assert!(
        !events
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::PolicyChanged { .. })),
        "/config must not record a policy event"
    );
}
