//! Phase 22 guided-onboarding verification, exercised through the public API.
//!
//! The wizard itself is an interactive host (it needs a TTY), so these tests
//! drive the pure surface the plan's verification checklist actually names:
//! first-run detection, and that a wizard-generated `.mjolnr/` loads clean and
//! round-trips through the Phase 15 loader. They deliberately do *not* assert
//! that files appear unconditionally — the previous test did, and that is what
//! let a non-interactive stub pass as a feature.

use mjolnr::cli::onboard::{self, Selections};
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::routing::scaffold::SeededRoute;

fn route(provider: &str, model: &str, roles: &[&str]) -> SeededRoute {
    SeededRoute {
        provider: ProviderId::new(provider),
        model: ModelId::new(model),
        roles: roles.iter().map(|role| (*role).to_owned()).collect(),
    }
}

#[test]
fn a_defaults_run_produces_a_mjolnr_that_loads_and_reaches_a_working_model() {
    // A run through the flow accepting mjolnr's suggestions: a flagship route
    // tagged plan, a cheap one tagged smol, plus an identity.
    let selections = Selections {
        routes: vec![
            route("anthropic", "claude-opus-4-8", &["plan"]),
            route("openai", "gpt-4o-mini", &["smol"]),
        ],
        soul: Some("# SOUL.md\n\nSmed.\n".to_owned()),
        user_profile: Some("# USER.md\n\nJerrik.\n".to_owned()),
        mcp_servers: vec![("docs".to_owned(), "https://example.test/mcp".to_owned())],
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let files = onboard::plan_files(&selections);
    // Write through the shared non-destructive path exactly as the wizard does.
    for file in &files {
        let path = temp.path().join(&file.relative_path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, &file.contents).expect("write");
    }

    // The identity and MCP files landed under .mjolnr/.
    let config_dir = temp.path().join(".mjolnr");
    assert!(config_dir.join("SOUL.md").exists());
    assert!(config_dir.join("USER.md").exists());
    assert!(config_dir.join("mcp.yaml").exists());

    // And the routing loads without diagnostics, resolving a default plus the
    // roles the person confirmed — a session that reaches a working model.
    let (table, diagnostics) = mjolnr::routing::load_dir(temp.path());
    assert!(
        diagnostics.is_empty(),
        "a wizard-generated .mjolnr/ must load clean: {diagnostics:?}"
    );
    assert_eq!(table.roles.get("default"), Some(&"anthropic".to_owned()));
    assert_eq!(table.roles.get("plan"), Some(&"anthropic".to_owned()));
    assert_eq!(table.roles.get("smol"), Some(&"openai".to_owned()));
}

#[test]
fn every_generated_path_stays_within_dot_mjolnr() {
    // No onboarding step writes outside `.mjolnr/` (the credential store aside,
    // which `auth` owns).
    let selections = Selections {
        routes: vec![route("openai", "gpt-4o", &[])],
        soul: Some("s".to_owned()),
        user_profile: Some("u".to_owned()),
        mcp_servers: vec![("s".to_owned(), "https://h.test".to_owned())],
    };
    for file in onboard::plan_files(&selections) {
        assert!(
            file.relative_path.starts_with(".mjolnr"),
            "onboarding wrote outside .mjolnr/: {}",
            file.relative_path.display()
        );
    }
}

#[test]
fn first_run_detection_leaves_configured_returning_and_declined_users_alone() {
    // The fresh machine opens the wizard.
    assert!(onboard::global_first_run(false, false, false));
    // A configured user, a returning user (session store present), and a user
    // who declined are each left to the normal launch.
    assert!(!onboard::global_first_run(true, false, false));
    assert!(!onboard::global_first_run(false, true, false));
    assert!(!onboard::global_first_run(false, false, true));
}

#[test]
fn a_project_with_dot_mjolnr_is_never_re_offered_the_project_flow() {
    let temp = tempfile::tempdir().expect("tempdir");
    assert!(onboard::project_first_run(temp.path()));
    std::fs::create_dir_all(temp.path().join(".mjolnr")).expect("mkdir");
    assert!(!onboard::project_first_run(temp.path()));
}
