//! Contained, bounded reads of the open project's files (Phase D7).
//!
//! One responsibility: turn a project-relative path into a listing or a file
//! read that provably stayed inside the workspace. It owns no policy beyond
//! containment, no approval, no persistence, and no git — the runtime routes
//! intent here, supplies what git knows about ignored paths, and records the
//! outcome.
//!
//! Two properties are load-bearing and easy to lose in a refactor:
//!
//! 1. **Containment is rechecked immediately before every filesystem read**,
//!    not once at validation time (AGENTS.md §3). `policy::paths::existing`
//!    canonicalizes, so the value it returns is the path that was proven
//!    contained, and it is the only path this module then opens. The gap
//!    between check and use is the vulnerability, and the way it is closed here
//!    is that there is no unchecked path to use.
//! 2. **A symlink is never followed to decide what an entry is.** Listing uses
//!    `symlink_metadata`; a link resolves through containment or it does not
//!    resolve at all. Following first and checking after is how a listing
//!    reports the size and kind of a file outside the workspace.

mod error;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::core::workspace_files::{
    ContentFacts, DirectoryEntry, DirectoryListing, EntryKind, FileMode, FileRead, FileSave,
    MAX_EDITABLE_FILE_BYTES, MAX_ENUMERATED_ENTRIES, PREVIEW_BYTES, PreviewReason, SNIFF_BYTES,
    SymlinkTarget, WorkspaceFileSaveRequest,
};
use crate::policy::paths;

pub use error::WorkspaceFileError;

/// The marker that makes a file generated.
///
/// Declared, never inferred. See `ContentFacts::Sniffed::generated` for why a
/// directory name is not evidence.
const GENERATED_MARKER: &[u8] = b"@generated";

/// List one directory of the project, one page at a time.
///
/// `ignored` is the set of project-relative paths git reports as ignored under
/// this directory, supplied by the caller. This module does not run git: a
/// module that both walks the filesystem and shells out to git has two reasons
/// to change, and the ignore answer belongs to the repository producer that
/// already owns every other git question (AGENTS.md §2.3).
///
/// An empty set is a legitimate answer and means "nothing here is ignored, as
/// far as anyone asked" — including the case where there is no repository to
/// ask. That is why [`DirectoryEntry::ignored`] documents `false` as also
/// meaning unasked: an unasked question cannot answer "ignored", and inventing
/// `true` from a directory name would be a guess about someone else's
/// conventions.
pub fn list_directory(
    root: &Path,
    requested: &str,
    page: u32,
    page_size: u32,
    ignored: &BTreeSet<String>,
) -> Result<DirectoryListing, WorkspaceFileError> {
    let page_size = page_size.max(1);
    // Containment first, and its canonical answer is the only path opened
    // below. `requested` is never touched again.
    let directory = contained_dir(root, requested)?;

    let mut names = Vec::new();
    let mut total_truncated = false;
    let iterator = std::fs::read_dir(&directory).map_err(|error| io(requested, &error))?;
    for entry in iterator {
        let entry = entry.map_err(|error| io(requested, &error))?;
        if names.len() >= MAX_ENUMERATED_ENTRIES {
            total_truncated = true;
            break;
        }
        names.push(entry.file_name());
    }

    // Deterministic order, decided here rather than left to `read_dir`, which
    // makes no ordering promise at all: two clients paging the same directory
    // would otherwise see different entries on page 2 (AGENTS.md §7).
    names.sort();

    let total = u32::try_from(names.len()).unwrap_or(u32::MAX);
    let pages = total.div_ceil(page_size).max(1);
    if page >= pages {
        return Err(WorkspaceFileError::PageOutOfRange {
            path: requested.to_owned(),
            page,
            pages,
        });
    }

    let skip = (page as usize).saturating_mul(page_size as usize);
    let entries = names
        .into_iter()
        .skip(skip)
        .take(page_size as usize)
        .map(|name| describe(root, &directory, requested, &name, ignored))
        .collect::<Vec<_>>();

    // Directories first, then by name. Applied after paging, not before: a sort
    // that reordered across page boundaries would move entries between pages
    // and let a client miss one entirely while paging forward.
    let mut entries = entries;
    entries.sort_by(|left, right| {
        let rank = |kind: EntryKind| u8::from(kind != EntryKind::Directory);
        rank(left.kind)
            .cmp(&rank(right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(DirectoryListing {
        path: normalize(requested),
        entries,
        page,
        total_entries: total,
        total_truncated,
    })
}

/// Read one file, deciding whether an editor may have it.
///
/// Containment is rechecked here, immediately before the open, even when the
/// caller listed the directory a moment ago: the listing proved a path was
/// contained *then*, and a symlink can be replaced between the two
/// (AGENTS.md §3).
///
/// The whole file is hashed, in both modes. A digest computed over a preview
/// excerpt would compare unequal against the file on every save and would
/// therefore be a staleness check that always fires, which is the same as none.
pub fn read_file(root: &Path, requested: &str) -> Result<FileRead, WorkspaceFileError> {
    let path = paths::existing(root, Path::new(requested))?;
    let metadata = std::fs::metadata(&path).map_err(|error| io(requested, &error))?;
    if !metadata.is_file() {
        return Err(WorkspaceFileError::WrongKind {
            path: requested.to_owned(),
            expected: "a regular file",
        });
    }
    let size_bytes = metadata.len();
    let writable = !metadata.permissions().readonly();

    // Over the ceiling, the file is still hashed but never decoded. Reading it
    // whole is deliberate: the digest has to describe the file a save would
    // overwrite, and streaming a hash over a bounded read would describe a
    // prefix instead. `MAX_EDITABLE_FILE_BYTES` bounds what reaches an editor,
    // not what may be hashed.
    let bytes = std::fs::read(&path).map_err(|error| io(requested, &error))?;
    let digest = hash(&bytes);
    let mode = classify_read(&bytes, size_bytes);

    Ok(FileRead {
        path: normalize(requested),
        mode,
        digest,
        size_bytes,
        writable,
    })
}

/// Save one existing, editable file after comparing the bytes the client read.
///
/// Containment is checked once to read the comparison bytes and once again at
/// the side-effect boundary. The canonical path from the second check is the
/// only path written. A stale digest is a refusal, never an invitation to
/// overwrite or relocate the edit.
pub fn save_file(
    root: &Path,
    request: WorkspaceFileSaveRequest,
) -> Result<FileSave, WorkspaceFileError> {
    let path = paths::existing(root, Path::new(&request.path))?;
    let metadata = std::fs::metadata(&path).map_err(|error| io(&request.path, &error))?;
    if !metadata.is_file() {
        return Err(WorkspaceFileError::WrongKind {
            path: request.path,
            expected: "a regular file",
        });
    }
    let text_size_bytes = u64::try_from(request.text.len()).unwrap_or(u64::MAX);
    if text_size_bytes > MAX_EDITABLE_FILE_BYTES {
        return Err(WorkspaceFileError::TooLarge {
            path: request.path,
            limit: MAX_EDITABLE_FILE_BYTES,
        });
    }
    if request.text.contains('\0') {
        return Err(WorkspaceFileError::Uneditable {
            path: request.path,
            detail: "editor text may not contain a NUL byte",
        });
    }

    let bytes = std::fs::read(&path).map_err(|error| io(&request.path, &error))?;
    let actual = hash(&bytes);
    if actual != request.expected_digest {
        return Err(WorkspaceFileError::Stale {
            path: request.path,
            expected: request.expected_digest,
            actual,
        });
    }
    if !matches!(
        classify_read(&bytes, metadata.len()),
        FileMode::Editable { .. }
    ) {
        return Err(WorkspaceFileError::Uneditable {
            path: request.path,
            detail: "binary, undecodable, or oversized files are preview-only",
        });
    }

    let immediate = paths::existing(root, Path::new(&request.path))?;
    if immediate != path {
        return Err(WorkspaceFileError::Stale {
            path: request.path,
            expected: request.expected_digest,
            actual,
        });
    }
    std::fs::write(&immediate, request.text.as_bytes())
        .map_err(|error| io(&request.path, &error))?;

    Ok(FileSave {
        path: normalize(&request.path),
        observed_digest: request.expected_digest,
        new_digest: hash(request.text.as_bytes()),
        size_bytes: request.text.len() as u64,
    })
}

/// Decide the mode from the bytes, in the order the reasons rank.
///
/// Size first: an oversized file is refused from the editor whatever its bytes
/// say, and deciding it last would mean decoding a megabyte to learn it was
/// never eligible. Then binary, then UTF-8 — `NotUtf8` is the residue after
/// "not binary", which is what makes it a distinct remedy rather than a second
/// name for the same condition.
fn classify_read(bytes: &[u8], size_bytes: u64) -> FileMode {
    if size_bytes > MAX_EDITABLE_FILE_BYTES {
        return preview(bytes, PreviewReason::TooLarge);
    }
    if sniffed(bytes).contains(&0) {
        return preview(bytes, PreviewReason::Binary);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => FileMode::Editable {
            text: text.to_owned(),
        },
        Err(_) => preview(bytes, PreviewReason::NotUtf8),
    }
}

/// A bounded, lossy excerpt. Lossy on purpose: a preview exists to say what a
/// human is looking at, and the one thing it must not do is fail to render
/// because the bytes it is describing are not text.
fn preview(bytes: &[u8], reason: PreviewReason) -> FileMode {
    let window = bytes.get(..bytes.len().min(PREVIEW_BYTES)).unwrap_or(bytes);
    FileMode::Preview {
        reason,
        excerpt: String::from_utf8_lossy(window).into_owned(),
        excerpt_truncated: bytes.len() > window.len(),
    }
}

/// Describe one child without following it.
///
/// Every failure here degrades to an honest `Unreadable` / `Other` rather than
/// aborting the page: one unreadable entry in a directory is ordinary, and
/// refusing the whole listing over it would make an explorer useless on any
/// tree containing a socket or a root-owned file.
fn describe(
    root: &Path,
    directory: &Path,
    requested: &str,
    name: &std::ffi::OsStr,
    ignored: &BTreeSet<String>,
) -> DirectoryEntry {
    let display_name = name.to_string_lossy().into_owned();
    let relative = join(requested, &display_name);
    let absolute = directory.join(name);

    let Ok(link) = std::fs::symlink_metadata(&absolute) else {
        return unreadable(display_name, relative, ignored);
    };

    if link.is_symlink() {
        // Resolved through containment, never by following first. An escaping
        // link is described as escaping and nothing about its target — not its
        // kind, not its size — is read or reported.
        let Ok(target) = paths::existing(root, Path::new(&relative)) else {
            return DirectoryEntry {
                name: display_name,
                ignored: ignored.contains(&relative),
                path: relative,
                kind: EntryKind::Other,
                symlink: Some(SymlinkTarget::Escaping),
                content: ContentFacts::NotAFile,
                size_bytes: None,
                writable: false,
            };
        };
        let symlink = Some(SymlinkTarget::Contained {
            path: relative_to(root, &target),
        });
        return describe_resolved(&target, display_name, relative, symlink, ignored);
    }

    describe_resolved(&absolute, display_name, relative, None, ignored)
}

fn describe_resolved(
    path: &Path,
    name: String,
    relative: String,
    symlink: Option<SymlinkTarget>,
    ignored: &BTreeSet<String>,
) -> DirectoryEntry {
    let Ok(metadata) = std::fs::metadata(path) else {
        return DirectoryEntry {
            name,
            ignored: ignored.contains(&relative),
            path: relative,
            kind: EntryKind::Other,
            symlink,
            content: ContentFacts::Unreadable,
            size_bytes: None,
            writable: false,
        };
    };

    let kind = if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };

    let (content, size_bytes) = match kind {
        EntryKind::File => (sniff(path, metadata.len()), Some(metadata.len())),
        EntryKind::Directory | EntryKind::Other => (ContentFacts::NotAFile, None),
    };

    DirectoryEntry {
        name,
        ignored: ignored.contains(&relative),
        path: relative,
        kind,
        symlink,
        content,
        size_bytes,
        writable: !metadata.permissions().readonly(),
    }
}

/// Classify a file from a bounded prefix.
///
/// One bounded read per file on the page, which is what keeps the cost of a
/// listing proportional to the page rather than to the directory. An oversized
/// file is not sniffed at all: it cannot reach the editor whatever the prefix
/// says, so reading it would buy nothing.
fn sniff(path: &Path, size_bytes: u64) -> ContentFacts {
    if size_bytes > MAX_EDITABLE_FILE_BYTES {
        return ContentFacts::Oversized;
    }
    let Ok(bytes) = sniff_prefix(path) else {
        return ContentFacts::Unreadable;
    };
    let window = sniffed(&bytes);
    ContentFacts::Sniffed {
        binary: window.contains(&0),
        generated: contains(window, GENERATED_MARKER),
    }
}

fn sniff_prefix(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0_u8; SNIFF_BYTES];
    let read = file.read(&mut buffer)?;
    buffer.truncate(read);
    Ok(buffer)
}

fn sniffed(bytes: &[u8]) -> &[u8] {
    bytes.get(..bytes.len().min(SNIFF_BYTES)).unwrap_or(bytes)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn unreadable(name: String, relative: String, ignored: &BTreeSet<String>) -> DirectoryEntry {
    DirectoryEntry {
        name,
        ignored: ignored.contains(&relative),
        path: relative,
        kind: EntryKind::Other,
        symlink: None,
        content: ContentFacts::Unreadable,
        size_bytes: None,
        writable: false,
    }
}

/// Containment for a directory, with "it is not a directory" kept separate from
/// "it is not contained". Collapsing them would report a typo as an escape
/// attempt, which is both wrong and alarming.
fn contained_dir(root: &Path, requested: &str) -> Result<PathBuf, WorkspaceFileError> {
    let path = if requested.is_empty() {
        paths::canonical_root(root).map_err(WorkspaceFileError::from)?
    } else {
        paths::existing(root, Path::new(requested))?
    };
    if !path.is_dir() {
        return Err(WorkspaceFileError::WrongKind {
            path: requested.to_owned(),
            expected: "a directory",
        });
    }
    Ok(path)
}

/// Project-relative, `/`-separated, with no leading separator.
fn normalize(requested: &str) -> String {
    requested.trim_matches('/').replace('\\', "/")
}

fn join(parent: &str, name: &str) -> String {
    let parent = normalize(parent);
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn relative_to(root: &Path, path: &Path) -> String {
    normalize(
        &path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

fn io(path: &str, error: &std::io::Error) -> WorkspaceFileError {
    WorkspaceFileError::Io {
        path: path.to_owned(),
        detail: error.to_string(),
    }
}

fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests;
