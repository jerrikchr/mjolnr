//! Image bytes in flight to a provider.
//!
//! [`ContentBlock::ImageRef`] persists a *path*, never bytes. Base64 in SQLite
//! would bloat the event log, break the diff-and-revert property of the record,
//! and put image payloads into every `Debug`-adjacent surface. So the durable
//! block keeps a workspace-relative source, and the bytes travel beside the
//! messages in a sidecar the runtime fills at request-assembly time.
//!
//! Providers may not read files (`AGENTS.md` §2.1: they "know wire formats and
//! nothing else"), which is the other half of why the sidecar exists. An adapter
//! joins by `source` and encodes; it never opens anything.
//!
//! [`ContentBlock::ImageRef`]: crate::core::message::ContentBlock::ImageRef

use std::collections::BTreeMap;
use std::sync::Arc;

/// The media types smed will send.
///
/// These four are the intersection of three constraints that happen to agree,
/// and the agreement is worth keeping: every image-capable adapter documents
/// them (`provider-contract.md` §5.5), and they are exactly the codecs the
/// `image` dependency is compiled with. So an image smed can *show* in the
/// transcript is an image smed can *send*, and neither half can quietly grow
/// past the other. Anything else is refused before a socket opens rather than
/// after a provider rejects it.
pub const SUPPORTED_MEDIA_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// smed's own per-image ceiling, in bytes of the *encoded file*.
///
/// Below every provider's documented limit on purpose (`provider-contract.md`
/// §5.5: Anthropic 10 MB base64, Gemini 20 MB for the whole request). A
/// provider-side rejection costs a round trip and the tokens that went with it
/// to learn something smed could have said instantly.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Most images one request may carry.
///
/// Well under Anthropic's 20-image threshold for stricter per-image dimension
/// limits, which makes that regime unreachable by construction rather than by
/// remembering it exists.
pub const MAX_IMAGES_PER_REQUEST: usize = 8;

/// One image's bytes, ready to encode.
///
/// `Debug` is manual and prints length and media type only. Secrets get this
/// treatment for confidentiality (`AGENTS.md` §3); this gets it for volume and
/// provenance — a megabyte of base64 in a log is useless to a reader and
/// ruinous to a terminal.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageBytes {
    pub media_type: String,
    pub bytes: Arc<[u8]>,
}

impl std::fmt::Debug for ImageBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageBytes")
            .field("media_type", &self.media_type)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .finish()
    }
}

impl ImageBytes {
    /// Base64 for the wire.
    #[must_use]
    pub fn base64(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// A `data:` URI, which is what the OpenAI-shaped adapters take.
    #[must_use]
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.base64())
    }
}

/// Image bytes for the `ImageRef` blocks in a request, keyed by `source`.
///
/// `BTreeMap` rather than `HashMap` so a request serialises deterministically —
/// the same property the rest of the wire layer relies on for reproducible
/// fixtures.
pub type ImageSidecar = BTreeMap<String, ImageBytes>;

/// Why an image could not be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRefusal {
    UnsupportedMediaType { media_type: String },
    TooLarge { bytes: usize, limit: usize },
    TooMany { count: usize, limit: usize },
    Unreadable { detail: String },
}

impl ImageRefusal {
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::UnsupportedMediaType { media_type } => format!(
                "`{media_type}` is not a media type smed sends; supported: {}",
                SUPPORTED_MEDIA_TYPES.join(", ")
            ),
            Self::TooLarge { bytes, limit } => {
                format!("the image is {bytes} bytes; smed's limit is {limit}")
            }
            Self::TooMany { count, limit } => {
                format!("this request carries {count} images; smed's limit is {limit}")
            }
            Self::Unreadable { detail } => format!("the image could not be read: {detail}"),
        }
    }
}

/// The media type for a path's extension, if smed sends it.
#[must_use]
pub fn media_type_for(path: &std::path::Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_never_reach_a_debug_surface() {
        // The whole reason `Debug` is hand-written. A derived one would put a
        // megabyte of pixel data into any log line that formatted a request.
        let image = ImageBytes {
            media_type: "image/png".to_owned(),
            bytes: Arc::from(vec![0xAB_u8; 4096].as_slice()),
        };
        let rendered = format!("{image:?}");
        assert!(rendered.contains("4096 bytes"));
        assert!(rendered.contains("image/png"));
        assert!(
            !rendered.contains("171") && !rendered.contains("ab"),
            "no payload may survive into Debug: {rendered}"
        );
    }

    #[test]
    fn a_data_uri_names_its_media_type() {
        let image = ImageBytes {
            media_type: "image/jpeg".to_owned(),
            bytes: Arc::from([1_u8, 2, 3].as_slice()),
        };
        assert!(image.data_uri().starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn only_the_formats_every_adapter_documents_are_sent() {
        use std::path::Path;
        assert_eq!(media_type_for(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(media_type_for(Path::new("a.jpeg")), Some("image/jpeg"));
        // Real image formats smed still will not send, because not every
        // adapter documents them and a partial capability is worse than none.
        assert_eq!(media_type_for(Path::new("a.heic")), None);
        assert_eq!(media_type_for(Path::new("a.bmp")), None);
        assert_eq!(media_type_for(Path::new("noextension")), None);
    }

    #[test]
    fn every_supported_type_has_an_extension_that_maps_to_it() {
        use std::path::Path;
        // Drift guard: a media type in the list that no extension produces is a
        // capability smed claims and can never exercise.
        for media_type in SUPPORTED_MEDIA_TYPES {
            let extension = media_type.trim_start_matches("image/");
            let path = format!("sample.{extension}");
            assert_eq!(
                media_type_for(Path::new(&path)),
                Some(*media_type),
                "no extension maps to {media_type}"
            );
        }
    }
}
