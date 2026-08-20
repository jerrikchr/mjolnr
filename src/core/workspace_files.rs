//! What the runtime knows about the open project's files (Phase D7 producer).
//!
//! Three modules, three reasons to change, the same split `core::repository`
//! established: `core` defines these types, `crate::workspace_files` produces
//! them by reading the filesystem under `policy::paths` containment, and
//! `runtime::client_bridge` projects them onto the wire and applies the trust
//! class. Two architecture guards keep it honest — `core` may not depend on
//! `crate::workspace_files` (AGENTS.md §2.1) and the bridge may not either, so
//! a type both the runtime snapshot and the projection can name has to live
//! here.
//!
//! **Nothing here claims currency.** A listing is what one `read_dir` returned
//! at one moment, and a file read is the bytes that were on disk at one moment.
//! Nothing watches the filesystem, so between two reads a file can change under
//! mjolnr and this module will not know. That is exactly why [`FileRead`] carries
//! a digest: the save path compares it rather than trusting it.

use crate::core::error::ReasonCode;

/// Largest file mjolnr will hand to an editor, in bytes.
///
/// Above this a file opens in bounded preview instead (§D7 acceptance: "binary
/// and over-limit files open in bounded preview mode, not the editor"). It is a
/// *memory* bound as much as a UX one — the producer reads the whole file to
/// hash and decode it, so the ceiling has to exist before the bytes are read,
/// not after.
pub const MAX_EDITABLE_FILE_BYTES: u64 = 1_048_576;

/// Bytes of a preview-mode file the producer will carry.
///
/// A preview exists to tell a human *what* they are looking at, not to show
/// them the file. The bridge clamps again on the way out; see
/// `MAX_FILE_PREVIEW_BYTES`.
pub const PREVIEW_BYTES: usize = 4_096;

/// Bytes sniffed from the front of a file to classify it.
///
/// Bounded because a directory page classifies every entry it carries, so this
/// multiplies by the page size. 8 KiB is enough to find a NUL byte or a
/// generated-file marker in any format that puts one near the top, and both of
/// those are the only questions asked of the prefix.
pub const SNIFF_BYTES: usize = 8_192;

/// Largest number of directory entries the producer will enumerate before
/// giving up on counting.
///
/// Distinct from the page size: a page is what a client receives, and this is
/// how deep the producer will walk one directory to build the pages from. A
/// directory with more children than this reports `total_truncated`, because a
/// total that silently stopped counting is a wrong total, not a bounded one.
pub const MAX_ENUMERATED_ENTRIES: usize = 10_000;

/// One directory, as one read of it went.
///
/// There is no `fresh` flag and no `is_current` method, for the reason
/// [`crate::core::repository::RepositoryProjection`] has none: nothing watches
/// the filesystem, so mjolnr can say what it saw and when it looked, and cannot
/// say the answer is still true.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectoryListing {
    /// Project-relative path of the directory that was listed. Empty for the
    /// project root, so a client never has to special-case a leading separator.
    pub path: String,
    /// The entries on the requested page, directories first and then by name.
    pub entries: Vec<DirectoryEntry>,
    /// Zero-based index of the page these entries came from.
    pub page: u32,
    /// How many entries the directory has in total, as far as the producer
    /// walked it.
    pub total_entries: u32,
    /// True when the directory has more children than
    /// [`MAX_ENUMERATED_ENTRIES`], so `total_entries` is a floor rather than a
    /// count.
    pub total_truncated: bool,
}

/// One child of a directory.
///
/// The metadata §D7 asks for by name — symlink, binary, generated, ignored,
/// large-file, and permission — is spread across [`Self::symlink`],
/// [`Self::content`], [`Self::ignored`], and [`Self::writable`] rather than
/// collected into six booleans, because three of those questions have a fourth
/// answer that is not yes or no: mjolnr could not look.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectoryEntry {
    pub name: String,
    /// Project-relative, `/`-separated regardless of platform.
    pub path: String,
    pub kind: EntryKind,
    /// Present only when the entry is itself a symbolic link.
    pub symlink: Option<SymlinkTarget>,
    pub content: ContentFacts,
    /// `None` for directories and for entries whose metadata could not be read.
    pub size_bytes: Option<u64>,
    /// True when git reports the path as ignored. False also means "no
    /// repository to ask", which is the honest default: an unasked question
    /// cannot answer "ignored".
    pub ignored: bool,
    /// Permission metadata: whether this process could write the entry, from
    /// the filesystem's read-only bit. It is not a claim that a write would
    /// succeed — ownership, ACLs, and mounts all outrank it — so a surface
    /// renders it as a hint and mjolnr still tries and reports.
    pub writable: bool,
}

/// What kind of thing a directory entry is, after following nothing.
///
/// `Symlink` is deliberately absent: a link's kind is the kind of what it points
/// at, and the fact that it is a link is [`DirectoryEntry::symlink`]. Folding
/// the two together would make "a link to a directory" unrepresentable without
/// resolving it, and resolution is exactly what containment governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryKind {
    Directory,
    File,
    /// A socket, FIFO, device node, or an entry whose metadata could not be
    /// read. mjolnr offers no way to open one.
    Other,
}

/// Where a symbolic link points, decided by containment and never by following.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SymlinkTarget {
    /// Resolves to a path inside the workspace; the project-relative target is
    /// carried so a surface can say where it goes.
    Contained { path: String },
    /// Resolves outside the workspace, or could not be resolved at all. Both
    /// are one variant on purpose: mjolnr refuses to open either, and giving the
    /// unresolvable case its own name would invite a caller to treat it as
    /// merely unknown rather than as refused.
    Escaping,
}

/// What the producer could tell about a file's bytes, or that it could not tell.
///
/// An enum rather than `binary: bool, generated: bool`, because "not binary"
/// and "never looked" are different statements and a pair of `false`s cannot
/// distinguish them (AGENTS.md §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentFacts {
    /// The first [`SNIFF_BYTES`] were read and classified.
    Sniffed {
        /// A NUL byte appeared in the prefix — the same rule `tools::files`
        /// applies, deliberately, so one file is not binary to the explorer and
        /// text to the read tool.
        binary: bool,
        /// A `@generated` marker appeared in the prefix.
        ///
        /// Declared, never inferred. mjolnr does not guess from a directory name
        /// that `build/` or `dist/` holds generated code: that is a claim about
        /// somebody else's conventions, and being wrong about it mislabels
        /// hand-written source. [`DirectoryEntry::ignored`] already answers the
        /// common case that motivates the guess.
        generated: bool,
    },
    /// Larger than [`MAX_EDITABLE_FILE_BYTES`], so no prefix was read and the
    /// file is not editable. Not the same as binary — an oversized file may be
    /// perfectly good text — and kept separate because the two send a reader to
    /// different remedies.
    Oversized,
    /// The prefix could not be read. Not evidence of anything about the
    /// content; a surface must not render it as text.
    Unreadable,
    /// Not a regular file, so there is no content to classify.
    NotAFile,
}

impl ContentFacts {
    /// Whether an editor may open this, on content grounds alone.
    ///
    /// Containment and staleness are decided elsewhere and are not softened by
    /// a `true` here.
    #[must_use]
    pub const fn editable(self) -> bool {
        matches!(self, Self::Sniffed { binary: false, .. })
    }
}

/// One file, as one read of it went.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileRead {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub mode: FileMode,
    /// SHA-256 of the exact bytes that were on disk, for the save path to
    /// compare against. Computed over the whole file even in preview mode, so
    /// the value means the same thing in both.
    pub digest: String,
    pub size_bytes: u64,
    /// Permission metadata; see [`DirectoryEntry::writable`] for what it is not.
    pub writable: bool,
}

/// The bounded, operator-controlled save request from a client.
///
/// `expected_digest` is the full-file digest returned by [`FileRead`]. It is
/// not an advisory version: the save producer compares it with the bytes on
/// disk immediately before writing and refuses a mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceFileSaveRequest {
    pub path: String,
    pub expected_digest: String,
    pub text: String,
}

impl WorkspaceFileSaveRequest {
    #[must_use]
    pub fn new(path: String, expected_digest: String, text: String) -> Self {
        Self {
            path,
            expected_digest,
            text,
        }
    }
}

/// The fact a contained file save produced after the filesystem write.
///
/// This is deliberately a digest-and-size record, not the file contents. The
/// contents already live on disk and an unbounded duplicate in the session log
/// would turn an operator edit into a second file store.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSave {
    pub path: String,
    pub observed_digest: String,
    pub new_digest: String,
    pub size_bytes: u64,
}

/// Whether a read produced something editable, or only something to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileMode {
    /// Decoded UTF-8 text within the editable ceiling.
    Editable { text: String },
    /// A bounded excerpt and the reason the editor may not have the file.
    ///
    /// §D7's acceptance is that these open in preview mode *rather than* the
    /// editor, so the reason travels with the excerpt: a preview that did not
    /// say why would leave a surface guessing, and guessing wrong is how a
    /// binary file ends up in a text buffer.
    Preview {
        reason: PreviewReason,
        excerpt: String,
        excerpt_truncated: bool,
    },
}

/// Why a file opened in preview instead of the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreviewReason {
    /// A NUL byte in the sniffed prefix.
    Binary,
    /// Over [`MAX_EDITABLE_FILE_BYTES`].
    TooLarge,
    /// No NUL byte, but the bytes are not valid UTF-8. Distinct from `Binary`
    /// because the remedy differs — an encoding mjolnr does not decode is not
    /// the same thing as a file that is not text at all.
    NotUtf8,
}

impl PreviewReason {
    /// Stable identifier for the wire. Contract, like a reason code: prose may
    /// change, these may not (AGENTS.md §6).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::TooLarge => "tooLarge",
            Self::NotUtf8 => "notUtf8",
        }
    }
}

/// One contained read the runtime is asked to perform (§D7).
///
/// A request type rather than two runtime methods, because both answers are
/// produced by the same actor hop against the same open project, and two
/// methods would be two places for "no project is open" to be decided
/// differently.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceFileRequest {
    /// One page of one directory. `page_size` is the caller's, already bounded
    /// at the wire — the producer clamps it to at least one and otherwise
    /// honours it.
    Directory {
        path: String,
        page: u32,
        page_size: u32,
    },
    /// One file, whole.
    File { path: String },
}

/// What such a read produced. Boxed because the two variants are very different
/// sizes and the large one would otherwise set the enum's size for both.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceFileAnswer {
    Directory(Box<DirectoryListing>),
    File(Box<FileRead>),
}

/// What the runtime holds for the currently-requested listing.
///
/// Three states rather than an `Option`, for the reason
/// [`crate::core::repository::RepositoryView`] has three: "no project is open"
/// and "the directory could not be read" send a reader to different remedies,
/// and collapsing them into `None` offers the wrong one half the time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FileTreeView {
    /// No project is open, so there is nothing to list.
    #[default]
    NoProject,
    /// A project is open but the directory could not be read. Carries the
    /// refusal rather than an empty listing, which would render as an empty
    /// directory — a positive claim mjolnr did not earn.
    Unavailable {
        code: ReasonCode,
        detail: String,
    },
    Listed(Box<DirectoryListing>),
}

impl FileTreeView {
    /// The listing, when there is one.
    #[must_use]
    pub const fn listing(&self) -> Option<&DirectoryListing> {
        match self {
            Self::Listed(listing) => Some(listing),
            Self::NoProject | Self::Unavailable { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_view_claims_nothing() {
        let view = FileTreeView::default();
        assert_eq!(view, FileTreeView::NoProject);
        assert!(view.listing().is_none());
    }

    /// The distinction the enum exists for: an unreadable directory is not an
    /// empty one. An empty listing renders as "this directory has no files",
    /// which is a positive claim about a directory mjolnr could not read.
    #[test]
    fn an_unreadable_directory_is_not_an_empty_listing() {
        let view = FileTreeView::Unavailable {
            code: ReasonCode::ToolExecution,
            detail: "permission denied".to_owned(),
        };
        assert!(view.listing().is_none());
    }

    /// `Unreadable` and `NotAFile` must not read as "plain text". A future
    /// maintainer widening `editable` to "anything that is not known-binary"
    /// fails here, which is the point: that widening would hand an editor a
    /// file whose bytes were never looked at.
    #[test]
    fn only_a_sniffed_non_binary_file_is_editable() {
        assert!(
            ContentFacts::Sniffed {
                binary: false,
                generated: false
            }
            .editable()
        );
        assert!(
            ContentFacts::Sniffed {
                binary: false,
                generated: true
            }
            .editable()
        );
        assert!(
            !ContentFacts::Sniffed {
                binary: true,
                generated: false
            }
            .editable()
        );
        assert!(!ContentFacts::Oversized.editable());
        assert!(!ContentFacts::Unreadable.editable());
        assert!(!ContentFacts::NotAFile.editable());
    }

    #[test]
    fn preview_reason_identifiers_are_stable_contract() {
        assert_eq!(PreviewReason::Binary.as_str(), "binary");
        assert_eq!(PreviewReason::TooLarge.as_str(), "tooLarge");
        assert_eq!(PreviewReason::NotUtf8.as_str(), "notUtf8");
    }

    /// The preview bound has to be under the editable ceiling, or a file could
    /// be refused from the editor for being too large and then carried whole in
    /// the preview that replaced it.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn a_preview_is_smaller_than_the_file_it_stands_in_for() {
        assert!(PREVIEW_BYTES as u64 <= MAX_EDITABLE_FILE_BYTES);
        assert!(SNIFF_BYTES as u64 <= MAX_EDITABLE_FILE_BYTES);
        assert!(MAX_ENUMERATED_ENTRIES > 0);
    }
}
