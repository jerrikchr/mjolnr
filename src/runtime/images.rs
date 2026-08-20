//! Loading image bytes for a request, and refusing when they cannot go
//! .
//!
//! Three things happen here, in this order, and the order is the point:
//!
//! 1. **Containment is rechecked immediately before the read**, through
//!    `policy::paths` — the real gate, not the TUI's display-only check in
//!    `tui/image.rs`. A path validated at paste time is a path that may have
//!    become a symlink since.
//! 2. **Bounds are mjolnr's own and enforced before the provider's.** A
//!    provider-side rejection spends a round trip and the tokens with it to
//!    learn something mjolnr could have said instantly.
//! 3. **A model that cannot accept images never receives a broken request.**
//!    Either the run is refused before it opens a socket, or the block is
//!    projected into a labelled placeholder — never silently dropped, which
//!    would leave the model answering about a picture it was never sent.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::core::image::{
    ImageBytes, ImageRefusal, ImageSidecar, MAX_IMAGE_BYTES, MAX_IMAGES_PER_REQUEST,
    SUPPORTED_MEDIA_TYPES,
};
use crate::core::message::{CanonicalMessage, ContentBlock};

/// Load every image the messages reference, or say why not.
///
/// # Errors
/// [`ImageRefusal`] when the set is too large, a path will not resolve inside
/// the workspace, or a file is unreadable or oversized.
pub fn load(
    messages: &[CanonicalMessage],
    workspace_root: &Path,
) -> Result<ImageSidecar, ImageRefusal> {
    // Distinct sources, because a transcript that shows one screenshot three
    // times should send its bytes once. The count that matters to a provider is
    // blocks, but the count that matters to a *request budget* is payloads.
    let sources: BTreeSet<&str> = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ImageRef { source, .. } => Some(source.as_str()),
            _ => None,
        })
        .collect();

    if sources.len() > MAX_IMAGES_PER_REQUEST {
        return Err(ImageRefusal::TooMany {
            count: sources.len(),
            limit: MAX_IMAGES_PER_REQUEST,
        });
    }

    let mut sidecar = ImageSidecar::new();
    for source in sources {
        sidecar.insert(source.to_owned(), read_one(source, workspace_root)?);
    }
    Ok(sidecar)
}

fn read_one(source: &str, workspace_root: &Path) -> Result<ImageBytes, ImageRefusal> {
    // The containment recheck. `policy::paths::existing` canonicalises and
    // rejects a symlink that leaves the workspace, which a path captured at
    // paste time cannot guarantee about itself later.
    let resolved =
        crate::policy::paths::existing(workspace_root, Path::new(source)).map_err(|refusal| {
            ImageRefusal::Unreadable {
                detail: refusal.detail,
            }
        })?;

    let Some(media_type) = crate::core::image::media_type_for(&resolved) else {
        return Err(ImageRefusal::UnsupportedMediaType {
            media_type: resolved
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("unknown")
                .to_owned(),
        });
    };

    // Size is checked from metadata before the read, so an oversized file is
    // refused without first pulling it into memory.
    let metadata = std::fs::metadata(&resolved).map_err(|error| ImageRefusal::Unreadable {
        detail: error.to_string(),
    })?;
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if size > MAX_IMAGE_BYTES {
        return Err(ImageRefusal::TooLarge {
            bytes: size,
            limit: MAX_IMAGE_BYTES,
        });
    }

    let bytes = std::fs::read(&resolved).map_err(|error| ImageRefusal::Unreadable {
        detail: error.to_string(),
    })?;
    Ok(ImageBytes {
        media_type: media_type.to_owned(),
        bytes: Arc::from(bytes.as_slice()),
    })
}

/// Whether any message carries an image.
#[must_use]
pub fn any_images(messages: &[CanonicalMessage]) -> bool {
    messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .any(|block| matches!(block, ContentBlock::ImageRef { .. }))
}

/// Replace every `ImageRef` with a labelled text placeholder.
///
/// Used when the resolved model cannot accept images but the history contains
/// one. mjolnr's history is mixed by construction — `/model` mid-session, Phase
/// 24 handoff, and Phase 15 fallback routing all change models, the last two
/// without the user asking each time — so a transcript with one screenshot in it
/// is routinely replayed to a model that cannot take it.
///
/// Of the three image-input options considered, this is the one
/// that keeps fallback routing working without ever implying the model saw
/// something it did not. Refusing the run outright would turn a provider outage
/// into a dead session for anyone who once pasted a screenshot. Dropping the
/// block silently would leave the model answering about a picture that was never
/// sent, which is worse than either.
///
/// This is a projection of the record for one request, never a rewrite of it:
/// the durable `ImageRef` stays exactly as it was.
#[must_use]
pub fn placeholder_images(messages: &[CanonicalMessage], model: &str) -> Vec<CanonicalMessage> {
    messages
        .iter()
        .map(|message| {
            if !message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ImageRef { .. }))
            {
                return message.clone();
            }
            let mut projected = message.clone();
            projected.blocks = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::ImageRef { source, .. } => ContentBlock::Text {
                        text: format!("[image: {source} — not sent; {model} cannot accept images]"),
                    },
                    other => other.clone(),
                })
                .collect();
            projected
        })
        .collect()
}

/// The media types mjolnr will send, for a refusal that names them.
#[must_use]
pub fn supported_media_types() -> String {
    SUPPORTED_MEDIA_TYPES.join(", ")
}

impl super::Actor {
    /// Decide what the request carries: the messages as sent, and the bytes
    /// beside them.
    ///
    /// # Errors
    /// `Err(())` when the run was refused. The failure is already recorded as a
    /// typed `RunFailed`, and no socket was opened to learn it.
    pub(super) async fn resolve_images(
        &mut self,
        model: &crate::core::model::ModelId,
        messages: Vec<CanonicalMessage>,
    ) -> Result<(Vec<CanonicalMessage>, ImageSidecar), ()> {
        if !any_images(&messages) {
            return Ok((messages, ImageSidecar::new()));
        }

        // A model that cannot see images gets labelled placeholders rather than
        // a refusal, so one screenshot early in a session does not make it
        // permanently unroutable — including to an automatic fallback, which
        // would otherwise turn a provider outage into a dead session.
        if !self.model_accepts_images(model) {
            return Ok((
                placeholder_images(&messages, model.as_str()),
                ImageSidecar::new(),
            ));
        }

        let Some(workspace_root) = self.state.workspace_root.clone() else {
            self.fail_run_for_images("open a workspace before sending an image".to_owned())
                .await;
            return Err(());
        };
        match load(&messages, &workspace_root) {
            Ok(images) => Ok((messages, images)),
            Err(refusal) => {
                self.fail_run_for_images(refusal.detail()).await;
                Err(())
            }
        }
    }

    /// Whether the resolved model declares `ImagesIn`.
    ///
    /// An unregistered model is treated as unable, which is the safe direction:
    /// a placeholder costs a sentence, while assuming a capability sends a
    /// request the provider rejects after the tokens are spent.
    fn model_accepts_images(&self, model: &crate::core::model::ModelId) -> bool {
        self.providers
            .iter()
            .flat_map(|provider| provider.models())
            .find(|descriptor| &descriptor.id == model)
            .is_some_and(|descriptor| descriptor.capabilities.images_in)
    }

    async fn fail_run_for_images(&mut self, detail: String) {
        let Some(run) = self.run.as_ref().map(|active| active.id) else {
            return;
        };
        self.fail_run(
            run,
            crate::core::error::ReasonCode::SchemaInvalid,
            format!("image refused: {detail}"),
        )
        .await;
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;
    use crate::core::message::ContentBlock;

    /// A workspace root the way the runtime holds one.
    ///
    /// `open_project` canonicalises through `policy::paths::canonical_root`, and
    /// on macOS a tempdir lives under a symlinked `/var`. A test that skips the
    /// canonicalisation is testing a state the runtime never reaches — and its
    /// failure looks exactly like a containment bug, which cost a few minutes
    /// here.
    fn workspace(directory: &tempfile::TempDir) -> std::path::PathBuf {
        crate::policy::paths::canonical_root(directory.path()).expect("canonical root")
    }

    fn image_message(source: &str) -> CanonicalMessage {
        let mut message = CanonicalMessage::user("look at this");
        message.blocks.push(ContentBlock::ImageRef {
            media_type: "image/png".to_owned(),
            source: source.to_owned(),
        });
        message
    }

    #[test]
    fn a_repeated_screenshot_is_read_once() {
        let root = tempfile::tempdir().expect("tempdir");
        image::RgbImage::new(4, 4)
            .save(root.path().join("shot.png"))
            .expect("write png");

        let messages = vec![image_message("shot.png"), image_message("shot.png")];
        let sidecar = load(&messages, &workspace(&root)).expect("load");
        assert_eq!(
            sidecar.len(),
            1,
            "one file referenced twice is one payload, not two"
        );
    }

    #[test]
    fn an_oversized_image_is_refused_without_being_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("huge.png");
        std::fs::write(&path, vec![0_u8; MAX_IMAGE_BYTES + 1]).expect("write");

        let refusal =
            load(&[image_message("huge.png")], &workspace(&root)).expect_err("must refuse");
        assert!(matches!(refusal, ImageRefusal::TooLarge { .. }));
    }

    #[test]
    fn a_path_outside_the_workspace_never_reaches_a_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        image::RgbImage::new(4, 4)
            .save(outside.path().join("secret.png"))
            .expect("write png");
        let escape = format!("../{}/secret.png", outside.path().display());

        let refusal = load(&[image_message(&escape)], &workspace(&root)).expect_err("must refuse");
        assert!(matches!(refusal, ImageRefusal::Unreadable { .. }));
    }

    #[test]
    fn too_many_distinct_images_is_refused_before_any_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let messages: Vec<CanonicalMessage> = (0..=MAX_IMAGES_PER_REQUEST)
            .map(|n| image_message(&format!("missing-{n}.png")))
            .collect();

        // Refused on count, not on the first unreadable path — proving the
        // bound is checked before the loop that would open files.
        let refusal = load(&messages, &workspace(&root)).expect_err("must refuse");
        assert!(matches!(refusal, ImageRefusal::TooMany { .. }));
    }

    #[test]
    fn a_placeholder_names_the_image_and_the_model_that_declined_it() {
        let projected = placeholder_images(&[image_message("shot.png")], "gemini-3.5-flash-low");
        let text = match &projected[0].blocks[1] {
            ContentBlock::Text { text } => text.clone(),
            other => panic!("expected a text placeholder, got {other:?}"),
        };
        assert!(text.contains("shot.png"));
        assert!(text.contains("gemini-3.5-flash-low"));
        assert!(
            text.contains("not sent"),
            "the model must not be able to read this as though it received the image: {text}"
        );
    }

    #[test]
    fn projecting_a_placeholder_leaves_the_record_alone() {
        // A projection for one request, never a rewrite of history.
        let original = vec![image_message("shot.png")];
        let _ = placeholder_images(&original, "some-model");
        assert!(matches!(
            original[0].blocks[1],
            ContentBlock::ImageRef { .. }
        ));
    }
}
