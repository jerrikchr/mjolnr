//! Inline transcript images.
//!
//! A message may carry a markdown image link — `Ctrl-V` on a screenshot writes
//! one, pointing at `.mjolnr/assets/`. This module turns such a link into cells
//! the terminal can draw, using whichever graphics protocol the terminal
//! actually reported (kitty, iTerm2, sixel) and falling back to unicode
//! half-blocks everywhere else.
//!
//! Three constraints shape it:
//!
//! * **Containment is rechecked here, not inherited.** `tui` may not import
//!   `policy` (`AGENTS.md` §2.1), so the workspace-root check below is a
//!   deliberate second implementation rather than a call into the first. It is
//!   read-only and narrower than the policy gate: a path that escapes the
//!   workspace is not displayed, whatever the message says about it.
//! * **Decoding is paid once per image, at build time, never per frame.**
//!   Rendering happens inside `terminal.draw`, so a per-frame resize would put
//!   an image decode on the redraw path. `Protocol` is encoded once for a fixed
//!   cell box and the cache is dropped when the transcript width changes.
//! * **A refusal is visible.** An image that cannot be read, decoded, or
//!   contained renders its caption plus the reason. Silently dropping it would
//!   be the transcript lying about what a message contained.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

/// Largest file the transcript will open. A screenshot is far below this; the
/// cap exists so a link to a multi-gigabyte file cannot stall a redraw.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Pixel ceiling handed to the decoder, independent of the file size — a small
/// file can still declare enormous dimensions.
const MAX_PIXEL_EDGE: u32 = 8192;
/// Widest cell box an inline image may claim, so a wide image cannot push the
/// transcript into a single tall column.
pub(crate) const MAX_COLUMNS: u16 = 64;
/// Tallest cell box, so one image cannot own the whole viewport.
pub(crate) const MAX_ROWS: u16 = 16;
/// The composer's thumbnail box. Deliberately smaller than the transcript's:
/// the preview answers "which image is attached", and the composer is four
/// rows of a working surface, not a gallery.
pub(crate) const PREVIEW_COLUMNS: u16 = 32;
pub(crate) const PREVIEW_ROWS: u16 = 6;

/// Why an image link did not become an image. Rendered next to the caption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoWorkspace,
    Outside,
    Missing,
    NotAFile,
    TooLarge,
    Unreadable,
    Undecodable,
    Unavailable,
}

impl Refusal {
    pub(crate) const fn detail(self) -> &'static str {
        match self {
            Self::NoWorkspace => "no workspace root",
            Self::Outside => "outside the workspace",
            Self::Missing => "file not found",
            Self::NotAFile => "not a regular file",
            Self::TooLarge => "file too large to render",
            Self::Unreadable => "unreadable",
            Self::Undecodable => "not a decodable image",
            Self::Unavailable => "image rendering unavailable",
        }
    }
}

/// Cache identity: one encoding per path *per cell box*. The same screenshot is
/// encoded twice — once for the transcript, once as a composer thumbnail — and
/// keying on the box is what lets both live at once instead of evicting each
/// other on every frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    path: PathBuf,
    box_width: u16,
    box_height: u16,
}

/// One image link resolved against the store.
#[derive(Debug, Clone)]
pub(crate) enum Slot {
    /// Decoded and encoded; `size` is the cell box it will occupy.
    Ready {
        key: CacheKey,
        size: Size,
    },
    Refused(Refusal),
}

/// A decoded image link found in a message, with its offset into that
/// message's rendered lines.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    pub(crate) offset: usize,
    pub(crate) key: CacheKey,
    pub(crate) size: Size,
}

struct Cached {
    protocol: Protocol,
    size: Size,
    len: u64,
    modified: Option<SystemTime>,
}

/// Encoded images for the current transcript width.
///
/// `Protocol` holds encoded terminal payloads and implements neither `Debug`
/// nor `Default`, so both are written by hand rather than derived — the manual
/// `Debug` also keeps decoded image bytes out of any diagnostic output.
#[derive(Default)]
pub(crate) struct ImageStore {
    picker: Option<Picker>,
    width: u16,
    entries: HashMap<CacheKey, Cached>,
}

impl std::fmt::Debug for ImageStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageStore")
            .field("protocol", &self.picker.as_ref().map(Picker::protocol_type))
            .field("width", &self.width)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl ImageStore {
    /// Adopt the protocol and font size the terminal reported. Until this is
    /// called every link refuses with [`Refusal::Unavailable`], which is what
    /// keeps frame tests and headless hosts free of graphics escapes.
    pub(crate) fn enable(&mut self, picker: Picker) {
        self.picker = Some(picker);
        self.entries.clear();
    }

    /// Drop every encoding when the *terminal* width changes. Each `Protocol`
    /// is encoded for a fixed cell box, so a resize invalidates all of them —
    /// the same contract `RenderCache::prepare` has for styled lines. This
    /// takes the terminal width, not a pane width: two panes with different
    /// widths asking in turn would otherwise clear the cache on every frame
    /// and decode every image, every redraw.
    pub(crate) fn prepare(&mut self, width: u16) {
        if self.width != width {
            self.width = width;
            self.entries.clear();
        }
    }

    /// Resolve, read, decode, and encode `target` into `available`, or say why
    /// not. The caller owns the cell box: the transcript and the composer want
    /// the same image at different sizes.
    pub(crate) fn resolve(
        &mut self,
        target: &str,
        workspace_root: Option<&Path>,
        available: Size,
    ) -> Slot {
        let Some(picker) = self.picker.as_ref() else {
            return Slot::Refused(Refusal::Unavailable);
        };
        let Some(root) = workspace_root else {
            return Slot::Refused(Refusal::NoWorkspace);
        };
        if available.width == 0 || available.height == 0 {
            return Slot::Refused(Refusal::Unavailable);
        }
        let key = match contained_path(target, root) {
            Ok(path) => CacheKey {
                path,
                box_width: available.width,
                box_height: available.height,
            },
            Err(refusal) => return Slot::Refused(refusal),
        };

        let Ok(metadata) = std::fs::metadata(&key.path) else {
            return Slot::Refused(Refusal::Missing);
        };
        if !metadata.is_file() {
            return Slot::Refused(Refusal::NotAFile);
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Slot::Refused(Refusal::TooLarge);
        }
        let modified = metadata.modified().ok();

        if let Some(cached) = self.entries.get(&key)
            && cached.len == metadata.len()
            && cached.modified == modified
        {
            return Slot::Ready {
                key,
                size: cached.size,
            };
        }

        let decoded = match decode(&key.path) {
            Ok(image) => image,
            Err(refusal) => return Slot::Refused(refusal),
        };
        let Ok(protocol) = picker.new_protocol(decoded, available, Resize::Fit(None)) else {
            return Slot::Refused(Refusal::Undecodable);
        };

        let size = protocol.size();
        self.entries.insert(
            key.clone(),
            Cached {
                protocol,
                size,
                len: metadata.len(),
                modified,
            },
        );
        Slot::Ready { key, size }
    }

    /// The encoded image for a key returned by [`Self::resolve`].
    pub(crate) fn protocol(&self, key: &CacheKey) -> Option<&Protocol> {
        self.entries.get(key).map(|cached| &cached.protocol)
    }
}

fn decode(path: &Path) -> Result<image::DynamicImage, Refusal> {
    let opened = image::ImageReader::open(path).map_err(|_| Refusal::Unreadable)?;
    // Format comes from the file's own bytes, never from its extension: a
    // message can name a path but it cannot be trusted to describe it.
    let mut reader = opened
        .with_guessed_format()
        .map_err(|_| Refusal::Unreadable)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXEL_EDGE);
    limits.max_image_height = Some(MAX_PIXEL_EDGE);
    reader.limits(limits);
    reader.decode().map_err(|_| Refusal::Undecodable)
}

/// Resolve a link target to a real path inside `root`, or refuse.
///
/// Symlinks are resolved *before* the containment test, so a link inside the
/// workspace pointing at `~/.ssh` is refused rather than rendered.
fn contained_path(target: &str, root: &Path) -> Result<PathBuf, Refusal> {
    let raw = percent_decode(target.strip_prefix("file://").unwrap_or(target));
    let candidate = Path::new(&raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let Ok(resolved) = joined.canonicalize() else {
        return Err(Refusal::Missing);
    };
    let Ok(root) = root.canonicalize() else {
        return Err(Refusal::NoWorkspace);
    };
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(Refusal::Outside)
    }
}

/// Decode `%XX` escapes. A `file://` URL for a path containing a space arrives
/// percent-encoded, and an undecoded one would look like a missing file.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let decoded = if byte == b'%' {
            match (bytes.get(index + 1), bytes.get(index + 2)) {
                (Some(high), Some(low)) => hex_pair(*high, *low),
                _ => None,
            }
        } else {
            None
        };
        if let Some(value) = decoded {
            out.push(value);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    let high = char::from(high).to_digit(16)?;
    let low = char::from(low).to_digit(16)?;
    u8::try_from(high * 16 + low).ok()
}

/// One `![alt](target)` occurrence in message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) alt: String,
    pub(crate) target: String,
}

/// Split message text into the text to render and the image links to draw.
///
/// The links are removed from the text because they are shown as the image
/// itself plus a caption; leaving the raw `file://` line in place would print
/// the path twice.
pub(crate) fn extract_links(text: &str) -> (String, Vec<Link>) {
    let mut remaining = text;
    let mut stripped = String::with_capacity(text.len());
    let mut links = Vec::new();

    while let Some(start) = remaining.find("![") {
        let Some(parsed) = parse_link(&remaining[start..]) else {
            let cut = start + 2;
            stripped.push_str(&remaining[..cut]);
            remaining = &remaining[cut..];
            continue;
        };
        stripped.push_str(&remaining[..start]);
        remaining = &remaining[start + parsed.consumed..];
        links.push(parsed.link);
    }
    stripped.push_str(remaining);
    (stripped.trim_end().to_owned(), links)
}

struct Parsed {
    link: Link,
    consumed: usize,
}

/// Parse `![alt](target)` at the start of `input`. Neither field may span a
/// line: an unclosed bracket in prose must not swallow the rest of a message.
fn parse_link(input: &str) -> Option<Parsed> {
    let after_bang = input.strip_prefix("![")?;
    let alt_end = after_bang.find(']')?;
    let alt = after_bang.get(..alt_end)?;
    // A bracket inside the alt text means the `]` found above probably closed
    // something else — `![half a [link](url)` is prose, not an image. Parsing
    // it as one would delete the sentence from the transcript, so the
    // ambiguous case resolves toward showing the text unchanged.
    if alt.contains('\n') || alt.contains('[') {
        return None;
    }
    let after_alt = after_bang.get(alt_end + 1..)?;
    let after_paren = after_alt.strip_prefix('(')?;
    let target_end = after_paren.find(')')?;
    let target = after_paren.get(..target_end)?;
    if target.contains('\n') || target.is_empty() {
        return None;
    }
    Some(Parsed {
        link: Link {
            alt: alt.to_owned(),
            target: target.to_owned(),
        },
        // `![` + alt + `]` + `(` + target + `)`
        consumed: 2 + alt_end + 1 + 1 + target_end + 1,
    })
}

/// A thumbnail the composer will draw, and the caption beside it.
#[derive(Debug, Clone)]
pub(crate) struct Attachment {
    pub(crate) caption: String,
    pub(crate) key: CacheKey,
    pub(crate) size: Size,
}

/// Resolve the image links in composer text into drawable thumbnails.
///
/// Only drawable ones: a link that refuses gets no row in the preview band,
/// because the composer is a working surface and its text is still visible and
/// editable — the reason belongs in the transcript, where the message lands.
pub(crate) fn attachments(
    store: &mut ImageStore,
    text: &str,
    workspace_root: Option<&Path>,
    available: Size,
) -> Vec<Attachment> {
    let (_, links) = extract_links(text);
    links
        .into_iter()
        .filter_map(
            |link| match store.resolve(&link.target, workspace_root, available) {
                Slot::Ready { key, size } => Some(Attachment {
                    caption: if link.alt.is_empty() {
                        "image".to_owned()
                    } else {
                        link.alt
                    },
                    key,
                    size,
                }),
                Slot::Refused(_) => None,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript_box() -> Size {
        Size::new(MAX_COLUMNS, MAX_ROWS)
    }

    #[test]
    fn extracts_a_pasted_link_and_leaves_no_path_behind() {
        let (text, links) = extract_links("look ![pasted_image](file:///tmp/a.png) at this");
        assert_eq!(text, "look  at this");
        assert_eq!(
            links,
            vec![Link {
                alt: "pasted_image".to_owned(),
                target: "file:///tmp/a.png".to_owned(),
            }]
        );
    }

    #[test]
    fn extracts_every_link_in_one_message() {
        let (_, links) = extract_links("![a](one.png)\n![b](two.png)");
        let targets: Vec<&str> = links.iter().map(|link| link.target.as_str()).collect();
        assert_eq!(targets, vec!["one.png", "two.png"]);
    }

    #[test]
    fn leaves_prose_that_only_looks_like_a_link() {
        let source = "an unclosed ![thing and a [link](url) in prose";
        let (text, links) = extract_links(source);
        assert_eq!(text, source);
        assert!(links.is_empty());
    }

    #[test]
    fn keeps_an_ordinary_link_next_to_an_image_link() {
        let (text, links) = extract_links("see [docs](docs/x.md) and ![shot](a.png)");
        assert_eq!(text, "see [docs](docs/x.md) and");
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn refuses_a_link_whose_target_spans_a_line() {
        assert!(parse_link("![a](one\n.png)").is_none());
    }

    #[test]
    fn decodes_percent_escapes_only_where_they_are_valid() {
        assert_eq!(percent_decode("/tmp/a%20b.png"), "/tmp/a b.png");
        assert_eq!(percent_decode("100% sure%zz"), "100% sure%zz");
    }

    #[test]
    fn refuses_a_path_that_escapes_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let target = outside.path().to_string_lossy().into_owned();
        assert_eq!(
            contained_path(&target, root.path()),
            Err(Refusal::Outside),
            "an absolute path outside the workspace must not be rendered"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_that_escapes_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let link = root.path().join("inside.png");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert_eq!(
            contained_path("inside.png", root.path()),
            Err(Refusal::Outside),
            "containment is tested after symlink resolution, not before"
        );
    }

    #[test]
    fn accepts_a_relative_path_inside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let assets = root.path().join(".mjolnr/assets");
        std::fs::create_dir_all(&assets).unwrap();
        let file = assets.join("paste_1.png");
        std::fs::write(&file, b"not really a png").unwrap();
        let resolved = contained_path(".mjolnr/assets/paste_1.png", root.path()).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn a_store_without_a_picker_refuses_every_link() {
        let mut store = ImageStore::default();
        store.prepare(80);
        assert!(matches!(
            store.resolve("anything.png", None, transcript_box()),
            Slot::Refused(Refusal::Unavailable)
        ));
    }

    #[test]
    fn a_real_png_encodes_into_a_bounded_cell_box() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("wide.png");
        image::RgbImage::new(600, 300).save(&path).unwrap();

        let mut store = ImageStore::default();
        store.enable(Picker::halfblocks());
        store.prepare(200);

        let Slot::Ready { key, size } =
            store.resolve("wide.png", Some(root.path()), transcript_box())
        else {
            panic!("a readable png must render");
        };
        assert!(
            size.width <= MAX_COLUMNS && size.height <= MAX_ROWS,
            "an image may not claim more than its cell box: {size:?}"
        );
        assert!(size.width > 0 && size.height > 0);
        assert!(store.protocol(&key).is_some());
    }

    #[test]
    fn the_same_image_encodes_once_per_cell_box() {
        let root = tempfile::tempdir().unwrap();
        image::RgbImage::new(200, 200)
            .save(root.path().join("a.png"))
            .unwrap();
        let mut store = ImageStore::default();
        store.enable(Picker::halfblocks());
        store.prepare(120);

        let big = store.resolve("a.png", Some(root.path()), transcript_box());
        let small = store.resolve(
            "a.png",
            Some(root.path()),
            Size::new(PREVIEW_COLUMNS, PREVIEW_ROWS),
        );
        let (Slot::Ready { key: big, .. }, Slot::Ready { key: small, .. }) = (big, small) else {
            panic!("both boxes must render");
        };
        assert_ne!(big, small, "one path at two sizes is two encodings");
        assert!(
            store.protocol(&big).is_some() && store.protocol(&small).is_some(),
            "a thumbnail must not evict the transcript encoding of the same file"
        );
    }

    #[test]
    fn a_composer_attachment_carries_its_caption() {
        let root = tempfile::tempdir().unwrap();
        image::RgbImage::new(64, 64)
            .save(root.path().join("shot.png"))
            .unwrap();
        let mut store = ImageStore::default();
        store.enable(Picker::halfblocks());
        store.prepare(120);

        let found = attachments(
            &mut store,
            "look ![pasted_image](shot.png) and ![gone](missing.png)",
            Some(root.path()),
            Size::new(PREVIEW_COLUMNS, PREVIEW_ROWS),
        );
        assert_eq!(found.len(), 1, "only drawable links become thumbnails");
        assert_eq!(
            found.first().map(|a| a.caption.as_str()),
            Some("pasted_image")
        );
    }

    #[test]
    fn a_width_change_drops_every_encoding() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a.png");
        image::RgbImage::new(40, 40).save(&path).unwrap();

        let mut store = ImageStore::default();
        store.enable(Picker::halfblocks());
        store.prepare(80);
        let Slot::Ready { key, .. } = store.resolve("a.png", Some(root.path()), transcript_box())
        else {
            panic!("a readable png must render");
        };
        store.prepare(40);
        assert!(
            store.protocol(&key).is_none(),
            "an encoding sized for the old width must not survive a resize"
        );
    }

    #[test]
    fn undecodable_bytes_refuse_rather_than_render() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.png"), b"still not a png").unwrap();
        let mut store = ImageStore::default();
        store.enable(Picker::halfblocks());
        store.prepare(80);
        assert!(matches!(
            store.resolve("a.png", Some(root.path()), transcript_box()),
            Slot::Refused(Refusal::Undecodable)
        ));
    }
}
