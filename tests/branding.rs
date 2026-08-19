//! Enforces that the product's *contract* surfaces name `mjolnr`, not `smed`
//! (ADR-0018).
//!
//! The rename is not one change, it is a long tail, and the tail splits in two:
//!
//! - **Prose** — "smed is Danish for smith", a paragraph in a design doc. Stale
//!   prose is embarrassing. It is not a defect, and rewriting it needs a human
//!   who can decide what the sentence should say instead.
//! - **Contracts** — the command a message tells you to run, the branch a child
//!   is committed to, the environment variable that redirects a provider, the
//!   binary a release workflow packages. A stale contract is *wrong*: it names
//!   something that does not exist.
//!
//! This test guards the second kind only. That boundary is the point. ADR-0018
//! plans the mechanical bulk of the rename for a cheap model with a human on the
//! trust-critical seams; a scan that fails the build on a wrong command or a
//! wrong branch prefix is what makes delegating the rest safe, because the
//! expensive review is replaced by a green test.
//!
//! Like `tests/architecture.rs` this is a source scan rather than a type-level
//! trick: approximate, legible, and its failure message names the rule.

#![allow(clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

/// The subcommands that make a bare `smed` token a command invocation rather
/// than a mention of the old name.
const SUBCOMMANDS: &[&str] = &[
    "init",
    "exec",
    "auth",
    "plugin",
    "sessions",
    "diagnostics",
    "onboard",
    "triggers",
    "--resume",
    "--data-dir",
];

/// Paths whose `smed` references are deliberate and must survive.
///
/// Every entry is a place where the old name is the *correct* answer: it names
/// history, or it names a legacy identifier the compat window still has to read.
/// A file earns a line here only with a reason someone can check.
const ALLOWED: &[(&str, &str)] = &[
    (
        "src/core/paths.rs",
        "owns the ADR-0018 compat shim: the legacy workspace and config namespaces are constants here",
    ),
    (
        "src/store/paths.rs",
        "the data directory and database file keep their pre-rename names until a migration ADR moves them",
    ),
    (
        "src/store/secrets.rs",
        "`dev.smed` is the abandoned keyring's service name, retained so the one-shot migration can still find old credentials",
    ),
    (
        "src/providers/openai_compat.rs",
        "reads the legacy `SMED_<ID>_BASE_URL` as a fallback for shells that already export it",
    ),
    (
        "src/cli/onboard.rs",
        "first-run detection must see a pre-rename workspace as already configured",
    ),
    (
        "src/context/harness.rs",
        "harness detection must recognise a pre-rename workspace",
    ),
    (
        "docs/adr/0018-rename-smed-to-mjolnr.md",
        "the decision record for the rename names what was renamed",
    ),
    (
        "docs/renaming-to-mjolnr.md",
        "the brainstorm that preceded the decision",
    ),
    (
        "tests/branding.rs",
        "this scanner names the pattern it forbids",
    ),
];

#[test]
fn no_message_tells_a_user_to_run_the_old_binary() {
    let mut wrong = Vec::new();
    for file in rust_sources() {
        let Some(text) = read(&file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if let Some(found) = command_invocation(line) {
                wrong.push(format!(
                    "{}:{}: `{found}` — the binary is `mjolnr`",
                    display(&file),
                    number + 1
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a message naming a command that does not exist is worse than no message:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn child_work_lands_on_a_branch_named_for_this_product() {
    // A branch prefix is the one part of the rename that writes itself into
    // somebody else's repository, where it outlives any release note.
    let mut wrong = Vec::new();
    for file in rust_sources() {
        let Some(text) = read(&file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if line.contains("smed/sub-") || line.contains("smed/ext-") {
                wrong.push(format!("{}:{}", display(&file), number + 1));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "subagent and external-agent branches must be `mjolnr/sub-*` and `mjolnr/ext-*`:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_canonical_environment_prefix_is_the_current_one() {
    // A `SMED_`-prefixed variable may only be *read* as a fallback, never
    // constructed as the name the product asks a user to set.
    let mut wrong = Vec::new();
    for file in rust_sources() {
        let Some(text) = read(&file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if line.contains("SMED_") {
                wrong.push(format!(
                    "{}:{}: {}",
                    display(&file),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the environment prefix is `MJOLNR_`; a legacy read belongs in an allowlisted file:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn release_surfaces_package_the_binary_that_is_built() {
    // The failure this guards is silent and total: `Cargo.toml` builds `mjolnr`
    // while the workflow copies `target/<t>/release/smed`, so every release job
    // fails on a path that no longer exists.
    let root = repository_root();
    let mut wrong = Vec::new();
    for relative in ["scripts/install.sh", ".github/workflows/release.yml"] {
        let path = root.join(relative);
        let Some(text) = read(&path) else {
            wrong.push(format!(
                "{relative}: missing — this guard has lost its subject"
            ));
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if line.contains("smed") {
                wrong.push(format!("{relative}:{}: {}", number + 1, line.trim()));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "install and release scripts must name the built binary:\n{}",
        wrong.join("\n")
    );
}

/// The scanner must catch the forms someone reaches for under a deadline, and
/// must not fire on the old name used as prose.
#[test]
fn the_scanner_detects_a_violation() {
    assert!(
        command_invocation("        println!(\"run `smed auth login {id}`\");").is_some(),
        "a command inside a user-facing message must be caught"
    );
    assert!(
        command_invocation("//! `smed init`: scaffold a starter routing config.").is_some(),
        "a doc comment teaching the wrong command must be caught"
    );
    assert!(
        command_invocation("    \"CLI fallback: `smed sessions list`\",").is_some(),
        "a bare string literal must be caught"
    );

    // Lookalikes that must NOT fire: the old name as prose, or as a path
    // component, is a cosmetic backlog item and not this test's business.
    assert!(
        command_invocation("//! smed is Danish for smith.").is_none(),
        "prose is not a command invocation"
    );
    assert!(
        command_invocation("let dir = root.join(\".smed\");").is_none(),
        "a legacy path is governed by the compat shim, not by this rule"
    );
    assert!(
        command_invocation("use smed::runtime::Runtime;").is_none(),
        "the internal crate name is deliberately unchanged (ADR-0018 §3)"
    );
    assert!(
        command_invocation("let x = smedinit();").is_none(),
        "the match must end on a word boundary"
    );
}

/// Whether the line invokes the old binary with one of its subcommands.
fn command_invocation(line: &str) -> Option<String> {
    let mut search = line;
    while let Some(at) = search.find("smed ") {
        let before_is_boundary = search[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        let rest = &search[at + "smed ".len()..];
        if before_is_boundary
            && let Some(subcommand) = SUBCOMMANDS
                .iter()
                .find(|candidate| starts_with_word(rest, candidate))
        {
            return Some(format!("smed {subcommand}"));
        }
        search = &search[at + "smed".len()..];
    }
    None
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric() && character != '-')
    })
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Read a file, or `None` when it cannot be read.
///
/// The callers decide what an unreadable file means. For a scanned source tree
/// it means "nothing to check here"; for a named release script it means the
/// guard has lost its subject, which is itself a failure.
fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn display(path: &Path) -> String {
    path.strip_prefix(repository_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn is_allowed(path: &Path) -> bool {
    let relative = display(path);
    ALLOWED
        .iter()
        .any(|(allowed, _)| relative == *allowed || relative.replace('\\', "/") == *allowed)
}

/// Every `.rs` file under `src/` and `tests/` that is not allowlisted.
fn rust_sources() -> Vec<PathBuf> {
    let root = repository_root();
    let mut found = Vec::new();
    for directory in ["src", "tests"] {
        collect(&root.join(directory), &mut found);
    }
    found.retain(|path| !is_allowed(path));
    found.sort();
    found
}

fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}
