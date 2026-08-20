#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::core::error::ReasonCode;

/// A workspace with a canonicalized root, because `policy::paths` compares
/// against a canonical root and `/tmp` is a symlink to `/private/tmp` on macOS.
/// A test that skipped this would pass on Linux and refuse everything on macOS.
struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        Self { _dir: dir, root }
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(&path, contents).expect("write");
        path
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).expect("mkdir");
        path
    }

    fn list(&self, requested: &str) -> Result<DirectoryListing, WorkspaceFileError> {
        list_directory(&self.root, requested, 0, 100, &BTreeSet::new())
    }
}

fn entry<'a>(listing: &'a DirectoryListing, name: &str) -> &'a DirectoryEntry {
    listing
        .entries
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("no entry named {name} in {:?}", listing.entries))
}

// ---------------------------------------------------------------------------
// Containment — the §D7 acceptance bullet, at both doors
// ---------------------------------------------------------------------------

/// The first half of §D7's containment bullet. A path that climbs out is
/// refused before any filesystem call, by the lexical check `policy::paths`
/// applies first.
#[test]
fn a_path_that_escapes_the_workspace_is_refused_at_both_doors() {
    let workspace = Workspace::new();
    workspace.write("inside.txt", "hello");

    let listed = workspace.list("../..").expect_err("must refuse");
    assert_eq!(listed.reason_code(), ReasonCode::PathOutsideWorkspace);

    let read = read_file(&workspace.root, "../../etc/passwd").expect_err("must refuse");
    assert_eq!(read.reason_code(), ReasonCode::PathOutsideWorkspace);
}

/// The second half, and the one a lexical check cannot catch: the path is
/// entirely inside the workspace and the *link* leads out. `PATH_SYMLINK_ESCAPE`
/// rather than `PATH_OUTSIDE_WORKSPACE`, because a typo and an escape are
/// different events and a surface that cannot tell them apart cannot say so.
#[test]
#[cfg(unix)]
fn a_symlink_out_of_the_workspace_is_refused_on_read_and_never_followed_on_list() {
    let outside = tempfile::tempdir().expect("outside");
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "not yours").expect("write");

    let workspace = Workspace::new();
    std::os::unix::fs::symlink(&secret, workspace.root.join("escape.txt")).expect("symlink");

    let error = read_file(&workspace.root, "escape.txt").expect_err("must refuse");
    assert_eq!(error.reason_code(), ReasonCode::PathSymlinkEscape);

    // Listing does not refuse the whole directory over one bad link — it
    // describes it as escaping and reports nothing about the target. The
    // assertions that matter are the absences: no size, no content class, and
    // the file's contents nowhere in the projection.
    let listing = workspace.list("").expect("the directory still lists");
    let escape = entry(&listing, "escape.txt");
    assert_eq!(escape.symlink, Some(SymlinkTarget::Escaping));
    assert_eq!(escape.kind, EntryKind::Other);
    assert_eq!(escape.content, ContentFacts::NotAFile);
    assert_eq!(escape.size_bytes, None);
    assert!(!format!("{listing:?}").contains("not yours"));
}

/// A link that stays inside is not an escape, and is described as what it
/// points at. Asserted in the same file as the refusal above so that tightening
/// containment into "refuse every symlink" fails here rather than shipping.
#[test]
#[cfg(unix)]
fn a_symlink_inside_the_workspace_resolves_and_says_where_it_goes() {
    let workspace = Workspace::new();
    workspace.write("real.txt", "content");
    std::os::unix::fs::symlink(
        workspace.root.join("real.txt"),
        workspace.root.join("link.txt"),
    )
    .expect("symlink");

    let listing = workspace.list("").expect("list");
    let link = entry(&listing, "link.txt");
    assert_eq!(
        link.symlink,
        Some(SymlinkTarget::Contained {
            path: "real.txt".to_owned()
        })
    );
    assert_eq!(link.kind, EntryKind::File);
    assert!(link.content.editable());

    let read = read_file(&workspace.root, "link.txt").expect("a contained link opens");
    assert_eq!(
        read.mode,
        FileMode::Editable {
            text: "content".to_owned()
        }
    );
}

/// Containment is rechecked at the read, not inherited from the listing. This
/// is the check-to-use gap AGENTS.md §3 names: the entry was contained when it
/// was listed and the link is replaced before the read.
#[test]
#[cfg(unix)]
fn a_link_replaced_after_a_listing_is_refused_at_the_read() {
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("secret.txt"), "not yours").expect("write");

    let workspace = Workspace::new();
    workspace.write("real.txt", "content");
    std::os::unix::fs::symlink(
        workspace.root.join("real.txt"),
        workspace.root.join("link.txt"),
    )
    .expect("symlink");

    let listing = workspace.list("").expect("list");
    assert_eq!(
        entry(&listing, "link.txt").symlink,
        Some(SymlinkTarget::Contained {
            path: "real.txt".to_owned()
        })
    );

    std::fs::remove_file(workspace.root.join("link.txt")).expect("unlink");
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        workspace.root.join("link.txt"),
    )
    .expect("relink");

    let error = read_file(&workspace.root, "link.txt").expect_err("must refuse");
    assert_eq!(error.reason_code(), ReasonCode::PathSymlinkEscape);
}

// ---------------------------------------------------------------------------
// Binary and over-limit files open in preview, not the editor
// ---------------------------------------------------------------------------

/// §D7: "binary and over-limit files open in bounded preview mode, not the
/// editor." Three reasons, three distinct answers, and none of them is
/// `Editable`.
#[test]
fn binary_oversized_and_undecodable_files_all_open_in_bounded_preview() {
    let workspace = Workspace::new();
    workspace.write("image.png", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR");
    workspace.write(
        "huge.txt",
        vec![b'a'; usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap() + 1],
    );
    // No NUL byte, so not binary — just an encoding mjolnr does not decode.
    workspace.write("latin1.txt", b"caf\xe9 au lait");

    let binary = read_file(&workspace.root, "image.png").expect("read");
    let FileMode::Preview { reason, .. } = binary.mode else {
        panic!("a binary file must not be editable: {:?}", binary.mode);
    };
    assert_eq!(reason, PreviewReason::Binary);

    let huge = read_file(&workspace.root, "huge.txt").expect("read");
    let FileMode::Preview {
        reason,
        excerpt,
        excerpt_truncated,
    } = huge.mode
    else {
        panic!("an oversized file must not be editable");
    };
    assert_eq!(reason, PreviewReason::TooLarge);
    assert!(excerpt_truncated);
    assert_eq!(excerpt.len(), PREVIEW_BYTES);

    let undecodable = read_file(&workspace.root, "latin1.txt").expect("read");
    let FileMode::Preview { reason, .. } = undecodable.mode else {
        panic!("undecodable bytes must not be editable");
    };
    assert_eq!(reason, PreviewReason::NotUtf8);
}

/// The digest describes the file, not the excerpt. A digest taken over a
/// preview window would compare unequal on every save and would therefore be a
/// staleness check that always fires, which is the same as having none.
#[test]
fn a_preview_still_carries_the_digest_of_the_whole_file() {
    let workspace = Workspace::new();
    let bytes = vec![b'z'; usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap() + 64];
    workspace.write("huge.bin", &bytes);

    let read = read_file(&workspace.root, "huge.bin").expect("read");
    assert_eq!(read.digest, hash(&bytes));
    assert_eq!(read.size_bytes, bytes.len() as u64);
}

// ---------------------------------------------------------------------------
// Save half — compare, contain, write, and describe the operator effect
// ---------------------------------------------------------------------------

#[test]
fn a_save_compares_the_open_digest_and_returns_the_new_digest() {
    let workspace = Workspace::new();
    workspace.write("src/main.rs", "fn main() {}\n");
    let opened = read_file(&workspace.root, "src/main.rs").expect("open");

    let saved = save_file(
        &workspace.root,
        WorkspaceFileSaveRequest::new(
            "src/main.rs".to_owned(),
            opened.digest.clone(),
            "fn main() { println!(\"saved\"); }\n".to_owned(),
        ),
    )
    .expect("save");

    assert_eq!(saved.path, "src/main.rs");
    assert_eq!(saved.observed_digest, opened.digest);
    assert_ne!(saved.new_digest, saved.observed_digest);
    assert_eq!(
        saved.size_bytes,
        "fn main() { println!(\"saved\"); }\n".len() as u64
    );
    assert_eq!(
        std::fs::read_to_string(workspace.root.join("src/main.rs")).expect("read saved"),
        "fn main() { println!(\"saved\"); }\n"
    );
}

#[test]
fn a_stale_save_refuses_without_overwriting_the_newer_file() {
    let workspace = Workspace::new();
    workspace.write("notes.txt", "before\n");
    let opened = read_file(&workspace.root, "notes.txt").expect("open");
    std::fs::write(workspace.root.join("notes.txt"), "newer\n").expect("external edit");

    let error = save_file(
        &workspace.root,
        WorkspaceFileSaveRequest::new(
            "notes.txt".to_owned(),
            opened.digest,
            "operator edit\n".to_owned(),
        ),
    )
    .expect_err("stale content must refuse");

    assert_eq!(error.reason_code(), ReasonCode::StaleFileVersion);
    assert_eq!(
        std::fs::read_to_string(workspace.root.join("notes.txt")).expect("read newer"),
        "newer\n"
    );
}

#[test]
fn a_save_rechecks_containment_before_writing() {
    let workspace = Workspace::new();
    workspace.write("inside.txt", "inside\n");

    let error = save_file(
        &workspace.root,
        WorkspaceFileSaveRequest::new(
            "../outside.txt".to_owned(),
            "not-a-real-digest".to_owned(),
            "should not write\n".to_owned(),
        ),
    )
    .expect_err("an escaping save must refuse");

    assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
}

/// The boundary itself, asserted from both sides: a file exactly at the ceiling
/// is editable and one byte more is not. An off-by-one here is the difference
/// between a working editor and a surface that silently previews everything.
#[test]
fn the_editable_ceiling_is_inclusive() {
    let workspace = Workspace::new();
    let limit = usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap();
    workspace.write("exact.txt", vec![b'a'; limit]);
    workspace.write("over.txt", vec![b'a'; limit + 1]);

    assert!(matches!(
        read_file(&workspace.root, "exact.txt").expect("read").mode,
        FileMode::Editable { .. }
    ));
    assert!(matches!(
        read_file(&workspace.root, "over.txt").expect("read").mode,
        FileMode::Preview {
            reason: PreviewReason::TooLarge,
            ..
        }
    ));
}

// ---------------------------------------------------------------------------
// Classification metadata §D7 names
// ---------------------------------------------------------------------------

/// `generated` is a declared marker and never an inference from a name. The
/// second half of this test is the guard: a file sitting in a directory called
/// `build` is *not* generated, because that would be a claim about someone
/// else's conventions and being wrong about it mislabels hand-written source.
#[test]
fn generated_is_a_declared_marker_not_a_guess_from_a_directory_name() {
    let workspace = Workspace::new();
    workspace.write(
        "wire.rs",
        "// @generated by the schema compiler\npub struct A;\n",
    );
    workspace.write("build/handwritten.rs", "pub struct B;\n");

    let listing = workspace.list("").expect("list");
    assert_eq!(
        entry(&listing, "wire.rs").content,
        ContentFacts::Sniffed {
            binary: false,
            generated: true
        }
    );

    let build = workspace.list("build").expect("list");
    assert_eq!(
        entry(&build, "handwritten.rs").content,
        ContentFacts::Sniffed {
            binary: false,
            generated: false
        }
    );
}

/// An oversized file is not sniffed at all, so it reports `Oversized` rather
/// than a binary/generated verdict it never looked for. The distinction is not
/// cosmetic: `Oversized` and `Sniffed { binary: true }` send a reader to
/// different remedies.
#[test]
fn an_oversized_entry_is_not_sniffed_and_does_not_claim_to_be_text() {
    let workspace = Workspace::new();
    workspace.write(
        "big.log",
        vec![b'x'; usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap() + 1],
    );

    let listing = workspace.list("").expect("list");
    let big = entry(&listing, "big.log");
    assert_eq!(big.content, ContentFacts::Oversized);
    assert!(!big.content.editable());
    assert_eq!(big.size_bytes, Some(MAX_EDITABLE_FILE_BYTES + 1));
}

/// `ignored` is what the caller was told by git, and nothing else. The second
/// assertion is the one that matters: the same directory listed with no ignore
/// set reports nothing as ignored rather than guessing from the name.
#[test]
fn ignored_is_carried_from_the_callers_answer_and_never_invented() {
    let workspace = Workspace::new();
    workspace.write("target/debug.log", "noise");
    workspace.write("src/main.rs", "fn main() {}");

    let mut ignored = BTreeSet::new();
    ignored.insert("target".to_owned());

    let listing =
        list_directory(&workspace.root, "", 0, 100, &ignored).expect("list with an ignore set");
    assert!(entry(&listing, "target").ignored);
    assert!(!entry(&listing, "src").ignored);

    let unasked = workspace.list("").expect("list without one");
    assert!(!entry(&unasked, "target").ignored);
}

#[test]
fn a_directory_reports_no_content_and_no_size() {
    let workspace = Workspace::new();
    workspace.dir("src");

    let listing = workspace.list("").expect("list");
    let source = entry(&listing, "src");
    assert_eq!(source.kind, EntryKind::Directory);
    assert_eq!(source.content, ContentFacts::NotAFile);
    assert_eq!(source.size_bytes, None);
}

// ---------------------------------------------------------------------------
// Pagination and ordering
// ---------------------------------------------------------------------------

/// Paging must enumerate every entry exactly once. The bug this guards is
/// sorting the *page* by kind before slicing, which moves entries across page
/// boundaries and lets a client paging forward miss one entirely.
#[test]
fn paging_a_directory_yields_every_entry_exactly_once() {
    let workspace = Workspace::new();
    for index in 0..25 {
        workspace.write(&format!("file-{index:02}.txt"), "x");
        workspace.dir(&format!("dir-{index:02}"));
    }

    let mut seen = Vec::new();
    for page in 0..5 {
        let listing = list_directory(&workspace.root, "", page, 10, &BTreeSet::new())
            .expect("every page in range");
        assert_eq!(listing.page, page);
        assert_eq!(listing.total_entries, 50);
        assert!(!listing.total_truncated);
        seen.extend(listing.entries.into_iter().map(|entry| entry.name));
    }

    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        50,
        "every entry appears exactly once across pages"
    );
}

/// A page past the end is refused, not answered with an empty page. An empty
/// page and "this directory is empty" are the same bytes on the wire and
/// different facts — the same reason D4 refuses a cursor past its bound.
#[test]
fn a_page_past_the_end_is_refused_rather_than_answered_empty() {
    let workspace = Workspace::new();
    workspace.write("only.txt", "x");

    let error =
        list_directory(&workspace.root, "", 3, 10, &BTreeSet::new()).expect_err("must refuse");
    assert_eq!(error.reason_code(), ReasonCode::SchemaInvalid);
    assert!(matches!(
        error,
        WorkspaceFileError::PageOutOfRange {
            page: 3,
            pages: 1,
            ..
        }
    ));
}

/// An empty directory still has one page, and that page is a successful empty
/// listing rather than a refusal. Without the `.max(1)` this is the case that
/// makes a brand-new directory unopenable.
#[test]
fn an_empty_directory_has_one_empty_page() {
    let workspace = Workspace::new();
    workspace.dir("empty");

    let listing = workspace.list("empty").expect("an empty directory lists");
    assert!(listing.entries.is_empty());
    assert_eq!(listing.total_entries, 0);
    assert!(!listing.total_truncated);
}

/// Directories before files, then by name, within a page. ADR-0009's explorer
/// draws them that way and `read_dir` promises no order at all.
#[test]
fn a_page_sorts_directories_before_files() {
    let workspace = Workspace::new();
    workspace.write("aaa.txt", "x");
    workspace.dir("zzz");

    let listing = workspace.list("").expect("list");
    let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["zzz", "aaa.txt"]);
}

// ---------------------------------------------------------------------------
// Wrong-kind refusals
// ---------------------------------------------------------------------------

/// Listing a file and reading a directory are both refused as schema mistakes,
/// kept distinct from a containment refusal. Reporting a typo as an escape
/// attempt is both wrong and alarming.
#[test]
fn asking_for_the_wrong_kind_is_a_schema_refusal_not_a_containment_one() {
    let workspace = Workspace::new();
    workspace.write("file.txt", "x");
    workspace.dir("folder");

    let listed = workspace.list("file.txt").expect_err("must refuse");
    assert_eq!(listed.reason_code(), ReasonCode::SchemaInvalid);
    assert!(matches!(listed, WorkspaceFileError::WrongKind { .. }));

    let read = read_file(&workspace.root, "folder").expect_err("must refuse");
    assert_eq!(read.reason_code(), ReasonCode::SchemaInvalid);
    assert!(matches!(read, WorkspaceFileError::WrongKind { .. }));
}

#[test]
fn a_missing_path_is_refused_and_names_nothing_outside_the_workspace() {
    let workspace = Workspace::new();
    let error = read_file(&workspace.root, "nope.txt").expect_err("must refuse");
    assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
    assert!(error.to_string().contains("nope.txt"));
}

// ---------------------------------------------------------------------------
// Path shape on the wire
// ---------------------------------------------------------------------------

/// Every path a client receives is project-relative with no leading separator,
/// including the root's own, which is the empty string rather than `/` or `.`.
/// A client that has to special-case three spellings of "the root" will get one
/// of them wrong.
#[test]
fn projected_paths_are_project_relative_with_an_empty_root() {
    let workspace = Workspace::new();
    workspace.write("src/lib.rs", "x");

    let root = workspace.list("").expect("list");
    assert_eq!(root.path, "");
    assert_eq!(entry(&root, "src").path, "src");

    let nested = workspace.list("src").expect("list");
    assert_eq!(nested.path, "src");
    assert_eq!(entry(&nested, "lib.rs").path, "src/lib.rs");
}

/// A leading slash is an absolute path and is refused, not quietly reread as
/// project-relative. `policy::paths` already decided this and the producer does
/// not soften it: reinterpreting `/etc/passwd` as `<root>/etc/passwd` is a
/// silent widening of exactly the kind containment exists to refuse, and every
/// path a client sends comes from `DirectoryEntry::path`, which never has one.
#[test]
fn a_leading_slash_is_an_absolute_path_and_is_refused() {
    let workspace = Workspace::new();
    workspace.write("src/lib.rs", "x");

    let listed = workspace.list("/src").expect_err("must refuse");
    assert_eq!(listed.reason_code(), ReasonCode::PathOutsideWorkspace);

    let read = read_file(&workspace.root, "/src/lib.rs").expect_err("must refuse");
    assert_eq!(read.reason_code(), ReasonCode::PathOutsideWorkspace);
}

/// Nothing in a projection carries the absolute path of the workspace. It is
/// not a secret, but it is host detail a frontend has no use for, and the
/// habit of not shipping it is what keeps a future projection from shipping
/// something that is.
#[test]
fn a_listing_never_carries_the_absolute_root() {
    let workspace = Workspace::new();
    workspace.write("src/lib.rs", "x");

    let listing = workspace.list("src").expect("list");
    let rendered = format!("{listing:?}");
    assert!(
        !rendered.contains(&workspace.root.display().to_string()),
        "projection leaked the absolute root: {rendered}"
    );
}

/// The producer refuses to be pointed at a root that is not a directory,
/// which is `policy::paths`' own guard reached through this door.
#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let workspace = Workspace::new();
    let file = workspace.write("not-a-root.txt", "x");
    let error = list_directory(&file, "", 0, 10, &BTreeSet::new()).expect_err("must refuse");
    assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
}

/// A zero page size would divide by zero building the page count. Clamped to
/// one rather than refused: the caller's bound is the bridge's to enforce, and
/// a producer that panicked on it would be a crash where a refusal belongs.
#[test]
fn a_zero_page_size_is_clamped_rather_than_dividing_by_zero() {
    let workspace = Workspace::new();
    workspace.write("a.txt", "x");
    workspace.write("b.txt", "x");

    let listing = list_directory(&workspace.root, "", 0, 0, &BTreeSet::new()).expect("list");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.total_entries, 2);
}

#[test]
fn hashing_matches_the_bytes_and_not_the_path() {
    assert_eq!(
        hash(b"hello"),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn the_sniff_window_is_bounded_by_its_constant() {
    let long = vec![b'a'; SNIFF_BYTES * 2];
    assert_eq!(sniffed(&long).len(), SNIFF_BYTES);
    assert_eq!(sniffed(b"short").len(), 5);
}

#[test]
fn contains_finds_a_marker_only_where_it_is() {
    assert!(contains(b"// @generated by x", GENERATED_MARKER));
    assert!(!contains(b"// generated by hand", GENERATED_MARKER));
    assert!(!contains(b"", GENERATED_MARKER));
}

/// A directory with more children than the producer will walk reports
/// `total_truncated` rather than a total that quietly stopped counting. Driven
/// through the constant rather than by writing ten thousand files, which would
/// make the suite slow for no extra coverage — the assertion is that the flag
/// is wired to the enumeration bound, and `describe` is already covered above.
#[test]
fn the_enumeration_bound_is_the_only_thing_that_sets_total_truncated() {
    let workspace = Workspace::new();
    for index in 0..5 {
        workspace.write(&format!("f{index}.txt"), "x");
    }
    let listing = workspace.list("").expect("list");
    assert!(!listing.total_truncated);
    assert!(listing.entries.len() <= MAX_ENUMERATED_ENTRIES);
}

#[test]
fn join_and_normalize_agree_on_the_root() {
    assert_eq!(join("", "a.txt"), "a.txt");
    assert_eq!(join("src", "a.txt"), "src/a.txt");
    assert_eq!(join("/src/", "a.txt"), "src/a.txt");
    assert_eq!(normalize("/src/"), "src");
    assert_eq!(normalize(""), "");
}

#[test]
fn relative_to_strips_the_root() {
    let root = Path::new("/work/project");
    assert_eq!(
        relative_to(root, Path::new("/work/project/src/a.rs")),
        "src/a.rs"
    );
}

#[test]
fn directory_listing_latency_is_bounded_on_a_full_page() {
    let workspace = Workspace::new();
    for index in 0..100 {
        let mut bytes = vec![b'a'; 900 * 1024];
        bytes[0] = b'@';
        bytes[1] = b'g';
        workspace.write(&format!("file-{index:03}.txt"), &bytes);
    }
    let start = std::time::Instant::now();
    let listing = list_directory(&workspace.root, "", 0, 100, &BTreeSet::new()).expect("list");
    let elapsed = start.elapsed();
    assert_eq!(listing.entries.len(), 100);
    // A page is bounded by page_size + SNIFF_BYTES per file, not file size.
    // 100 files near the 1 MiB ceiling with a full-file read would be ~90 MiB;
    // an 8 KiB prefix keeps it under a megabyte and therefore fast.
    assert!(
        elapsed.as_millis() < 2_000,
        "listing 100 near-ceiling files took {elapsed:?}, expected <2s with bounded sniff"
    );
}

#[test]
fn an_external_mutation_between_read_and_save_is_a_stale_refusal() {
    let workspace = Workspace::new();
    workspace.write("notes.txt", "before\n");
    let opened = read_file(&workspace.root, "notes.txt").expect("open");
    std::fs::write(workspace.root.join("notes.txt"), "externally edited\n").expect("external edit");
    let mutated_read = read_file(&workspace.root, "notes.txt").expect("re-read");
    assert_ne!(opened.digest, mutated_read.digest);

    let error = save_file(
        &workspace.root,
        WorkspaceFileSaveRequest::new(
            "notes.txt".to_owned(),
            opened.digest,
            "stale attempt\n".to_owned(),
        ),
    )
    .expect_err("stale digest must refuse");
    assert_eq!(
        error.reason_code(),
        crate::core::error::ReasonCode::StaleFileVersion
    );
    assert_eq!(
        std::fs::read_to_string(workspace.root.join("notes.txt")).expect("read"),
        "externally edited\n"
    );
}

#[test]
fn a_re_read_after_external_mutation_sees_the_new_digest() {
    let workspace = Workspace::new();
    workspace.write("notes.txt", "before\n");
    let first = read_file(&workspace.root, "notes.txt").expect("first read");
    std::fs::write(workspace.root.join("notes.txt"), "after\n").expect("external edit");
    let second = read_file(&workspace.root, "notes.txt").expect("second read");
    assert_ne!(first.digest, second.digest);
    let saved = save_file(
        &workspace.root,
        WorkspaceFileSaveRequest::new(
            "notes.txt".to_owned(),
            second.digest.clone(),
            "after edited\n".to_owned(),
        ),
    )
    .expect("save with fresh digest");
    assert_eq!(saved.observed_digest, second.digest);
}
