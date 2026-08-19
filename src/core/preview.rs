//! Studio responsive preview contracts and viewport models (Phase 4 Slice 4.1).
//!
//! Provides the data models for the embedded responsive iframe preview,
//! including preset viewports (Desktop, Tablet, Mobile, Custom), zoom scaling,
//! and locale switching.
//!
//! Visual preview is a read-only presentation projection — it does not execute
//! code on behalf of the agent or mutate workspace truth.

use serde::{Deserialize, Serialize};

/// Standard responsive viewport presets for the preview canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewportPreset {
    Desktop,
    Tablet,
    Mobile,
}

impl ViewportPreset {
    #[must_use]
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Desktop => (1280, 800),
            Self::Tablet => (768, 1024),
            Self::Mobile => (375, 667),
        }
    }
}

/// Viewport configuration for the responsive iframe preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewViewport {
    pub preset: Option<ViewportPreset>,
    pub width: u32,
    pub height: u32,
    pub zoom: f32,
    pub locale: Option<String>,
}

impl Default for PreviewViewport {
    fn default() -> Self {
        let (width, height) = ViewportPreset::Desktop.dimensions();
        Self {
            preset: Some(ViewportPreset::Desktop),
            width,
            height,
            zoom: 1.0,
            locale: None,
        }
    }
}

/// Studio Preview state projection for clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewState {
    pub url: Option<String>,
    pub active: bool,
    pub viewport: PreviewViewport,
    pub auto_reload: bool,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            url: None,
            active: false,
            viewport: PreviewViewport::default(),
            auto_reload: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_preset_dimensions_match_specs() {
        assert_eq!(ViewportPreset::Desktop.dimensions(), (1280, 800));
        assert_eq!(ViewportPreset::Tablet.dimensions(), (768, 1024));
        assert_eq!(ViewportPreset::Mobile.dimensions(), (375, 667));
    }

    #[test]
    fn preview_state_serializes_cleanly() {
        let preview = PreviewState {
            url: Some("http://localhost:3000".to_owned()),
            active: true,
            viewport: PreviewViewport {
                preset: Some(ViewportPreset::Mobile),
                width: 375,
                height: 667,
                zoom: 1.0,
                locale: Some("en-US".to_owned()),
            },
            auto_reload: true,
        };

        let json = serde_json::to_string(&preview).expect("serialize preview");
        assert!(json.contains("\"preset\":\"mobile\""));
        assert!(json.contains("\"url\":\"http://localhost:3000\""));

        let deserialized: PreviewState = serde_json::from_str(&json).expect("deserialize preview");
        assert_eq!(preview, deserialized);
    }
}
