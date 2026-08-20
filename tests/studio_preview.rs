//! Integration tests for Studio responsive preview contract (Master Implementation Plan Phase 4 Slice 4.1).

#![allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may index and unwrap freely"
)]

use mjolnr::core::preview::{PreviewState, PreviewViewport, ViewportPreset};

#[test]
fn preview_viewport_presets_and_custom_dimensions() {
    let desktop_dims = ViewportPreset::Desktop.dimensions();
    let tablet_dims = ViewportPreset::Tablet.dimensions();
    let mobile_dims = ViewportPreset::Mobile.dimensions();

    assert_eq!(desktop_dims, (1280, 800));
    assert_eq!(tablet_dims, (768, 1024));
    assert_eq!(mobile_dims, (375, 667));

    let custom = PreviewViewport {
        preset: None,
        width: 1440,
        height: 900,
        zoom: 1.25,
        locale: Some("fr-FR".to_owned()),
    };

    assert!((custom.zoom - 1.25).abs() < f32::EPSILON);
    assert_eq!(custom.locale.as_deref(), Some("fr-FR"));
}

#[test]
fn client_snapshot_bridges_preview_state() {
    let preview = PreviewState {
        url: Some("http://localhost:5173".to_owned()),
        active: true,
        viewport: PreviewViewport {
            preset: Some(ViewportPreset::Tablet),
            width: 768,
            height: 1024,
            zoom: 1.0,
            locale: Some("de-DE".to_owned()),
        },
        auto_reload: true,
    };

    let json = serde_json::to_string(&preview).expect("serialize preview");
    assert!(json.contains("\"preset\":\"tablet\""));
    assert!(json.contains("\"autoReload\":true"));

    let deserialized: PreviewState =
        serde_json::from_str(&json).expect("deserialize preview state");
    assert_eq!(preview, deserialized);
}

#[test]
fn preview_state_is_a_read_only_presentation_projection() {
    let default_preview = PreviewState::default();
    assert!(!default_preview.active);
    assert_eq!(default_preview.url, None);
    assert_eq!(
        default_preview.viewport.preset,
        Some(ViewportPreset::Desktop)
    );
}
